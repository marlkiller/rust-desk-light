use super::event::{AdminEvent, AdminEventSink, AdminInput, ReconnectEndpoint};
use crate::runtime::{hostname, load_admin_identity, os_label, username, Config};
use rdl_protocol::{write_envelope_with_token, EnvelopeDecoder, FileTransferAction, Message, Role};
use std::collections::HashSet;
use std::io;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{mpsc::Receiver, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const NETWORK_POLL_INTERVAL_MS: u64 = 16;
const NETWORK_IDLE_SLEEP_MS: u64 = 4;
const CONNECT_TIMEOUT_MS: u64 = 5_000;
const HANDSHAKE_TIMEOUT_MS: u64 = 8_000;
const MAX_INPUTS_PER_NETWORK_POLL: usize = 64;
const MAX_MESSAGES_PER_NETWORK_POLL: usize = 512;

enum ConnectionEnd {
    ReconnectRequested(Option<ReconnectEndpoint>),
}

pub(super) fn admin_network_loop(
    mut config: Config,
    input_rx: Receiver<AdminInput>,
    event_sink: AdminEventSink,
    ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
) -> io::Result<()> {
    let mut first_attempt = true;
    let mut wait_for_user = false;
    let mut skip_reload_once = false;
    loop {
        if wait_for_user {
            if let Some(endpoint) = wait_for_reconnect_request(&input_rx) {
                apply_reconnect_endpoint(&mut config, endpoint);
                skip_reload_once = true;
            }
        }
        if first_attempt {
            first_attempt = false;
        } else if skip_reload_once {
            skip_reload_once = false;
        } else if let Ok(reloaded) = config.reload() {
            config = reloaded;
        }

        if config.auth_token.trim().is_empty() {
            event_sink.send(AdminEvent::AuthTokenRequired);
            event_sink.send(AdminEvent::Disconnected);
            wait_for_user = true;
            continue;
        }
        match admin_connection_once(&config, &input_rx, &event_sink, &ignored_file_transfers) {
            Ok(ConnectionEnd::ReconnectRequested(endpoint)) => {
                if let Some(endpoint) = endpoint {
                    apply_reconnect_endpoint(&mut config, endpoint);
                    skip_reload_once = true;
                }
                event_sink.send(AdminEvent::Disconnected);
                wait_for_user = false;
            }
            Err(error) => {
                let detail = connection_error_detail(&error);
                event_sink.send(AdminEvent::ConnectionFailed {
                    ip: config.ip.clone(),
                    port: config.port,
                    auth_token: config.auth_token.clone(),
                    detail: detail.clone(),
                });
                event_sink.send(AdminEvent::Log(format!("connect failed: {detail}")));
                event_sink.send(AdminEvent::Disconnected);
                wait_for_user = true;
            }
        }
    }
}

fn wait_for_reconnect_request(input_rx: &Receiver<AdminInput>) -> Option<ReconnectEndpoint> {
    while let Ok(input) = input_rx.recv() {
        if let AdminInput::Reconnect { reason, endpoint } = input {
            debug_log!("debug event=admin_reconnect_request reason={reason}");
            return endpoint;
        }
    }
    None
}

fn apply_reconnect_endpoint(config: &mut Config, endpoint: ReconnectEndpoint) {
    config.ip = endpoint.ip;
    config.port = endpoint.port;
    config.auth_token = endpoint.auth_token;
}

fn admin_connection_once(
    config: &Config,
    input_rx: &Receiver<AdminInput>,
    event_sink: &AdminEventSink,
    ignored_file_transfers: &Arc<Mutex<HashSet<(String, u64)>>>,
) -> io::Result<ConnectionEnd> {
    let identity = load_admin_identity();
    let mut stream = connect_to_server(&config.ip, config.port)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(NETWORK_POLL_INTERVAL_MS)))?;
    let mut next_message_id = 1u64;
    send(
        &mut stream,
        &mut next_message_id,
        "",
        Message::Hello {
            role: Role::Admin,
            auth_token: config.auth_token.clone(),
            id: identity.id,
            fingerprint: identity.fingerprint,
            hostname: hostname(),
            os: os_label(),
            username: username(),
            gui_available: true,
        },
    )?;
    let session_token = wait_for_session(&mut stream, event_sink)?;
    send(
        &mut stream,
        &mut next_message_id,
        &session_token,
        Message::ListClients,
    )?;
    event_sink.send(AdminEvent::Connected);
    let mut decoder = EnvelopeDecoder::new();

    loop {
        let mut processed_inputs = 0usize;
        while processed_inputs < MAX_INPUTS_PER_NETWORK_POLL {
            let Ok(input) = input_rx.try_recv() else {
                break;
            };
            processed_inputs += 1;
            let result = match input {
                AdminInput::Command {
                    target_id,
                    command,
                    payload,
                } => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::Command {
                        target_id,
                        command,
                        payload,
                    },
                ),
                AdminInput::DesktopControl { target_id, payload } => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::DesktopControl { target_id, payload },
                ),
                AdminInput::DesktopInput { target_id, payload } => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::DesktopInput { target_id, payload },
                ),
                AdminInput::VideoControl {
                    target_id,
                    source,
                    payload,
                } => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::VideoControl {
                        target_id,
                        source,
                        payload,
                    },
                ),
                AdminInput::AudioControl {
                    target_id,
                    source,
                    payload,
                } => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::AudioControl {
                        target_id,
                        source,
                        payload,
                    },
                ),
                AdminInput::FileTransfer(message) => {
                    send(&mut stream, &mut next_message_id, &session_token, message)
                }
                AdminInput::Proxy(message) => {
                    send(&mut stream, &mut next_message_id, &session_token, message)
                }
                AdminInput::Reconnect { reason, endpoint } => {
                    debug_log!("debug event=admin_reconnect_request reason={reason}");
                    let _ = stream.shutdown(Shutdown::Both);
                    return Ok(ConnectionEnd::ReconnectRequested(endpoint));
                }
            };
            result?;
        }

        let mut processed_messages = 0usize;
        while processed_messages < MAX_MESSAGES_PER_NETWORK_POLL {
            let message = match decoder.read_next(&mut stream) {
                Ok(Some(envelope)) => envelope.message,
                Ok(None) => {
                    if processed_messages == 0 {
                        thread::sleep(Duration::from_millis(NETWORK_IDLE_SLEEP_MS));
                    }
                    break;
                }
                Err(error) => {
                    event_sink.send(AdminEvent::Log(format!("network read failed: {error}")));
                    return Err(error);
                }
            };
            processed_messages += 1;

            match message {
                Message::Clients(clients) => {
                    event_sink.send(AdminEvent::Clients(clients));
                }
                Message::CommandAck {
                    client_id,
                    command,
                    accepted,
                    detail,
                } => {
                    event_sink.send(AdminEvent::Ack {
                        client_id,
                        command,
                        accepted,
                        detail,
                    });
                }
                Message::CommandOutput {
                    client_id,
                    command,
                    stream_id,
                    sequence,
                    stream,
                    chunk,
                    current_dir,
                    finished,
                    success,
                } => {
                    event_sink.send(AdminEvent::CommandOutput {
                        client_id,
                        command,
                        stream_id,
                        sequence,
                        stream,
                        chunk,
                        current_dir,
                        finished,
                        success,
                    });
                }
                Message::DesktopFrame { client_id, payload } => {
                    event_sink.send(AdminEvent::DesktopFrame { client_id, payload });
                }
                Message::VideoFrame {
                    client_id,
                    source,
                    seq,
                    source_width,
                    source_height,
                    image_width,
                    image_height,
                    format,
                    bytes,
                } => {
                    event_sink.send(AdminEvent::VideoFrame {
                        client_id,
                        source,
                        seq,
                        source_width,
                        source_height,
                        image_width,
                        image_height,
                        format,
                        bytes,
                    });
                }
                message @ Message::FileTransfer { .. } => {
                    if let Message::FileTransfer {
                        target_id,
                        transfer_id,
                        action,
                        ..
                    } = &message
                    {
                        if admin_network_should_ignore_file_transfer(
                            ignored_file_transfers,
                            target_id,
                            *transfer_id,
                            *action,
                        ) {
                            continue;
                        }
                    }
                    event_sink.send(AdminEvent::FileTransfer(message));
                }
                Message::ProxyOpenResult {
                    client_id,
                    stream_id,
                    accepted,
                    detail,
                } => {
                    event_sink.send(AdminEvent::ProxyOpenResult {
                        client_id,
                        stream_id,
                        accepted,
                        detail,
                    });
                }
                Message::ProxyData {
                    client_id,
                    stream_id,
                    bytes,
                } => {
                    event_sink.send(AdminEvent::ProxyData {
                        client_id,
                        stream_id,
                        bytes,
                    });
                }
                Message::ProxyClose {
                    client_id,
                    stream_id,
                    reason,
                } => {
                    event_sink.send(AdminEvent::ProxyClose {
                        client_id,
                        stream_id,
                        reason,
                    });
                }
                Message::Ping => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::Pong,
                )?,
                other => {
                    event_sink.send(AdminEvent::Log(format!("server: {other:?}")));
                }
            }
        }
    }
}

fn wait_for_session(stream: &mut TcpStream, event_sink: &AdminEventSink) -> io::Result<String> {
    let mut decoder = EnvelopeDecoder::new();
    let started_at = Instant::now();
    loop {
        if started_at.elapsed() >= Duration::from_millis(HANDSHAKE_TIMEOUT_MS) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "server handshake timed out",
            ));
        }
        let message = match decoder.read_next(stream) {
            Ok(Some(envelope)) => envelope.message,
            Ok(None) => {
                thread::sleep(Duration::from_millis(NETWORK_IDLE_SLEEP_MS));
                continue;
            }
            Err(error) => return Err(error),
        };

        match message {
            Message::Session { token } => return Ok(token),
            Message::Error { detail } if detail.starts_with("auth failed") => {
                event_sink.send(AdminEvent::AuthTokenRejected(detail.clone()));
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, detail));
            }
            other => {
                event_sink.send(AdminEvent::Log(format!("server before session: {other:?}")));
            }
        }
    }
}

fn connect_to_server(ip: &str, port: u16) -> io::Result<TcpStream> {
    let addr = format!("{ip}:{port}");
    let timeout = Duration::from_millis(CONNECT_TIMEOUT_MS);
    let addrs: Vec<_> = addr.to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("server address resolved to no socket addresses: {addr}"),
        ));
    }

    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotConnected,
            format!("could not connect to server: {addr}"),
        )
    }))
}

fn connection_error_detail(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => format!("Connection refused: {error}"),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted => {
            format!("Connection closed by server: {error}")
        }
        io::ErrorKind::NotFound | io::ErrorKind::AddrNotAvailable => {
            format!("Server address is not reachable: {error}")
        }
        io::ErrorKind::TimedOut => format!("Connection timed out: {error}"),
        io::ErrorKind::PermissionDenied => format!("Token rejected: {error}"),
        io::ErrorKind::InvalidInput => format!("Invalid server address: {error}"),
        _ => format!("{error}"),
    }
}

fn admin_network_should_ignore_file_transfer(
    ignored_file_transfers: &Arc<Mutex<HashSet<(String, u64)>>>,
    client_id: &str,
    transfer_id: u64,
    action: FileTransferAction,
) -> bool {
    let key = (client_id.to_string(), transfer_id);
    let Ok(mut ignored) = ignored_file_transfers.lock() else {
        return false;
    };
    if !ignored.contains(&key) {
        return false;
    }
    if !matches!(
        action,
        FileTransferAction::Directory | FileTransferAction::Chunk
    ) {
        debug_log!(
            "debug event=admin_file_transfer_ignore client={} id={} action={}",
            client_id,
            transfer_id,
            action.as_str()
        );
    }
    if matches!(
        action,
        FileTransferAction::Complete | FileTransferAction::Error
    ) {
        ignored.remove(&key);
    }
    true
}

fn send(
    writer: &mut TcpStream,
    next_message_id: &mut u64,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    let result = write_envelope_with_token(
        writer,
        Role::Admin,
        *next_message_id,
        None,
        session_token,
        message,
    );
    *next_message_id = next_message_id.saturating_add(1);
    result
}

use super::event::{AdminEvent, AdminEventSink, AdminInput};
use crate::runtime::{hostname, load_admin_identity, os_label, username, Config};
use rdl_protocol::{write_envelope_with_token, EnvelopeDecoder, FileTransferAction, Message, Role};
use std::collections::HashSet;
use std::io;
use std::net::{Shutdown, TcpStream};
use std::sync::{mpsc::Receiver, Arc, Mutex};
use std::thread;
use std::time::Duration;

const INITIAL_RECONNECT_DELAY_MS: u64 = 500;
const MAX_RECONNECT_DELAY_MS: u64 = 8_000;
const NETWORK_POLL_INTERVAL_MS: u64 = 16;
const NETWORK_IDLE_SLEEP_MS: u64 = 4;
const MAX_INPUTS_PER_NETWORK_POLL: usize = 64;
const MAX_MESSAGES_PER_NETWORK_POLL: usize = 512;

pub(super) fn admin_network_loop(
    config: Config,
    input_rx: Receiver<AdminInput>,
    event_sink: AdminEventSink,
    ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
) -> io::Result<()> {
    let mut delay = INITIAL_RECONNECT_DELAY_MS;
    loop {
        match admin_connection_once(&config, &input_rx, &event_sink, &ignored_file_transfers) {
            Ok(AdminConnectionExit::Quit) => return Ok(()),
            Ok(AdminConnectionExit::Disconnected) => delay = INITIAL_RECONNECT_DELAY_MS,
            Err(error) => {
                event_sink.send(AdminEvent::Log(format!(
                    "connect failed: {error}; retrying in {delay}ms"
                )));
            }
        }
        event_sink.send(AdminEvent::Disconnected);
        thread::sleep(Duration::from_millis(delay));
        delay = (delay * 2).min(MAX_RECONNECT_DELAY_MS);
    }
}

enum AdminConnectionExit {
    Disconnected,
    Quit,
}

fn admin_connection_once(
    config: &Config,
    input_rx: &Receiver<AdminInput>,
    event_sink: &AdminEventSink,
    ignored_file_transfers: &Arc<Mutex<HashSet<(String, u64)>>>,
) -> io::Result<AdminConnectionExit> {
    let identity = load_admin_identity();
    let mut stream = TcpStream::connect(format!("{}:{}", config.ip, config.port))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(NETWORK_POLL_INTERVAL_MS)))?;
    let mut next_message_id = 1u64;
    send(
        &mut stream,
        &mut next_message_id,
        "",
        Message::Hello {
            role: Role::Admin,
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
                AdminInput::List => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::ListClients,
                ),
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
                AdminInput::AudioFrame {
                    target_id,
                    source,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                } => send(
                    &mut stream,
                    &mut next_message_id,
                    &session_token,
                    Message::AudioFrame {
                        client_id: target_id,
                        source,
                        seq,
                        sample_rate,
                        channels,
                        format,
                        bytes,
                    },
                ),
                AdminInput::FileTransfer(message) => {
                    send(&mut stream, &mut next_message_id, &session_token, message)
                }
                AdminInput::Reconnect { reason } => {
                    eprintln!("debug event=admin_reconnect_request reason={reason}");
                    let _ = stream.shutdown(Shutdown::Both);
                    return Ok(AdminConnectionExit::Disconnected);
                }
                AdminInput::Quit => {
                    let _ = stream.shutdown(Shutdown::Both);
                    return Ok(AdminConnectionExit::Quit);
                }
            };
            if result.is_err() {
                return Ok(AdminConnectionExit::Disconnected);
            }
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
                    return Ok(AdminConnectionExit::Disconnected);
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
                Message::AudioFrame {
                    client_id,
                    source,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                } => {
                    event_sink.send(AdminEvent::AudioFrame {
                        client_id,
                        source,
                        seq,
                        sample_rate,
                        channels,
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
    loop {
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
            other => {
                event_sink.send(AdminEvent::Log(format!("server before session: {other:?}")));
            }
        }
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
        eprintln!(
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

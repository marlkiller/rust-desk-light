#[cfg(feature = "gui")]
use crate::user_interaction;
use crate::{
    commands,
    runtime::{
        gui_available, hostname, load_client_identity, os_label, username, Config, LocalIdentity,
    },
};
#[cfg(feature = "gui")]
use eframe::egui;
use rdl_protocol::{
    audio_udp, now_epoch_ms, write_envelope_with_token, AudioSource, CommandKind, EnvelopeDecoder,
    FileTransferAction, FileTransferDirection, Message, Role, VideoSource,
};
use std::io;
use std::net::{TcpStream, UdpSocket};
#[cfg(feature = "gui")]
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

const INITIAL_RECONNECT_DELAY_MS: u64 = 500;
const MAX_RECONNECT_DELAY_MS: u64 = 8_000;
const NETWORK_POLL_INTERVAL_MS: u64 = 16;
#[cfg(feature = "gui")]
const GUI_FRAME_INTERVAL_MS: u64 = 16;
const NETWORK_IDLE_SLEEP_MS: u64 = 4;
const CLIENT_OUTBOUND_QUEUE_CAPACITY: usize = 32;
const CLIENT_BULK_OUTBOUND_QUEUE_CAPACITY: usize = 2;
const CLIENT_BULK_POLL_MS: u64 = 2;
const AUDIO_CAPTURE_FRAME_MS: u32 = 10;
const AUDIO_CAPTURE_RECV_TIMEOUT_MS: u64 = 20;
const AUDIO_STREAM_STOP_SETTLE_MS: u64 = 180;
const AUDIO_STREAM_REPORT_INTERVAL_MS: u64 = 1_000;
const AUDIO_UDP_REGISTER_INTERVAL_MS: u64 = 250;
const AUDIO_UDP_RECV_TIMEOUT_MS: u64 = 20;
const AUDIO_UDP_MAX_PAYLOAD_BYTES: usize = 1_200;

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let process_lock = crate::runtime::acquire_client_process_lock()?;
    debug_log!(
        "debug event=client_process_lock path={}",
        process_lock.path().display()
    );
    if gui_available() {
        #[cfg(feature = "gui")]
        {
            match run_gui(config.clone()) {
                Ok(()) => return Ok(()),
                Err(error) => eprintln!("GUI startup failed: {error}; falling back to terminal"),
            }
        }
    }
    run_terminal(config)?;
    Ok(())
}

#[cfg(feature = "gui")]
fn run_gui(config: Config) -> eframe::Result {
    disable_macos_automatic_window_tabbing();

    let identity = load_client_identity();
    let (event_tx, event_rx) = mpsc::channel();
    let (input_tx, input_rx) = mpsc::channel();
    let app_config = config.clone();
    let network_identity = identity.clone();
    let repaint_handle = Arc::new(Mutex::new(None));
    let network_repaint_handle = repaint_handle.clone();

    thread::spawn(move || {
        let event_sink = ClientEventSink::new(event_tx, Some(network_repaint_handle));
        if let Err(error) = client_network_loop(
            app_config,
            network_identity,
            true,
            event_sink.clone(),
            input_rx,
        ) {
            event_sink.send(ClientEvent::Log(format!("network stopped: {error}")));
        }
    });

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([780.0, 520.0])
        .with_min_inner_size([680.0, 440.0]);
    let viewport = match rust_desk_light_assets::app_window_icon() {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let window_title = rdl_version::app_version("rust-desk-light client");

    eframe::run_native(
        &window_title,
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(ClientApp::new(
                cc,
                config,
                identity,
                event_rx,
                input_tx,
                repaint_handle,
            )))
        }),
    )
}

#[cfg(all(feature = "gui", target_os = "macos"))]
fn disable_macos_automatic_window_tabbing() {
    if let Some(main_thread) = objc2_foundation::MainThreadMarker::new() {
        objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, main_thread);
    }
}

#[cfg(all(feature = "gui", not(target_os = "macos")))]
fn disable_macos_automatic_window_tabbing() {}

fn run_terminal(config: Config) -> io::Result<()> {
    let identity = load_client_identity();
    let (event_tx, event_rx) = mpsc::channel();
    let (_input_tx, input_rx) = mpsc::channel();
    println!(
        "rust-desk-light client {} terminal fallback, server={}:{} config={}",
        rdl_version::display_version(),
        config.ip,
        config.port,
        config.config_path.display()
    );
    println!("client id: {}", identity.id);
    println!("fingerprint: {}", identity.fingerprint);
    println!("waiting for admin commands; press Ctrl+C to exit");

    thread::spawn(move || {
        let event_sink = ClientEventSink::new(event_tx, None);
        if let Err(error) =
            client_network_loop(config, identity, false, event_sink.clone(), input_rx)
        {
            event_sink.send(ClientEvent::Log(format!("network stopped: {error}")));
        }
    });

    for event in event_rx {
        match event {
            ClientEvent::Connected => println!("connected"),
            ClientEvent::Disconnected => println!("disconnected"),
            ClientEvent::Command { command, payload } => {
                println!("command={} payload={payload}", command.as_str());
            }
            ClientEvent::ChatMessage { text } => println!("text_chat={text}"),
            ClientEvent::VoiceChatInvite => println!("voice_chat=incoming"),
            ClientEvent::VoiceChatConnected => println!("voice_chat=connected"),
            ClientEvent::VoiceChatEnded { message } => println!("voice_chat=ended {message}"),
            ClientEvent::VoiceChatFailed { message } => println!("voice_chat=failed {message}"),
            ClientEvent::Log(line) => println!("{line}"),
        }
    }

    Ok(())
}

fn client_network_loop(
    config: Config,
    identity: LocalIdentity,
    gui_mode: bool,
    event_sink: ClientEventSink,
    input_rx: Receiver<ClientInput>,
) -> io::Result<()> {
    let mut delay = INITIAL_RECONNECT_DELAY_MS;
    let mut last_config = config.clone();
    loop {
        let active_config = match config.reload() {
            Ok(config) => {
                last_config = config.clone();
                config
            }
            Err(error) => {
                event_sink.send(ClientEvent::Log(format!(
                    "config reload failed: {error}; using last known endpoint {}:{}",
                    last_config.ip, last_config.port
                )));
                last_config.clone()
            }
        };
        match client_connection_once(
            active_config,
            identity.clone(),
            gui_mode,
            event_sink.clone(),
            &input_rx,
        ) {
            Ok(()) => delay = INITIAL_RECONNECT_DELAY_MS,
            Err(error) => {
                event_sink.send(ClientEvent::Log(format!(
                    "connect failed: {error}; retrying in {delay}ms"
                )));
            }
        }
        event_sink.send(ClientEvent::Disconnected);
        thread::sleep(Duration::from_millis(delay));
        delay = (delay * 2).min(MAX_RECONNECT_DELAY_MS);
    }
}

fn client_connection_once(
    config: Config,
    identity: LocalIdentity,
    gui_mode: bool,
    event_sink: ClientEventSink,
    input_rx: &Receiver<ClientInput>,
) -> io::Result<()> {
    let stream = TcpStream::connect(format!("{}:{}", config.ip, config.port))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(NETWORK_POLL_INTERVAL_MS)))?;
    let writer = stream.try_clone()?;
    let (out_tx, out_rx) = mpsc::sync_channel(CLIENT_OUTBOUND_QUEUE_CAPACITY);
    let (bulk_out_tx, bulk_out_rx) = mpsc::sync_channel(CLIENT_BULK_OUTBOUND_QUEUE_CAPACITY);
    thread::spawn(move || client_writer_loop(writer, out_rx, bulk_out_rx));
    queue_message(
        &out_tx,
        "",
        Message::Hello {
            role: Role::Client,
            auth_token: config.auth_token.clone(),
            id: identity.id.clone(),
            fingerprint: identity.fingerprint.clone(),
            hostname: hostname(),
            os: os_label(),
            username: username(),
            gui_available: gui_mode,
        },
    )?;

    let mut reader = stream;
    let mut decoder = EnvelopeDecoder::new();
    let mut session_token = String::new();
    let desktop_stream = Arc::new(DesktopStreamState {
        running: AtomicBool::new(false),
        generation: std::sync::atomic::AtomicU64::new(0),
    });
    let camera_stream = Arc::new(DesktopStreamState {
        running: AtomicBool::new(false),
        generation: std::sync::atomic::AtomicU64::new(0),
    });
    let audio_stream = Arc::new(DesktopStreamState {
        running: AtomicBool::new(false),
        generation: std::sync::atomic::AtomicU64::new(0),
    });
    let voice_chat_stream = Arc::new(DesktopStreamState {
        running: AtomicBool::new(false),
        generation: std::sync::atomic::AtomicU64::new(0),
    });
    let voice_chat_mic_muted = Arc::new(AtomicBool::new(false));
    let voice_chat_speaker_muted = Arc::new(AtomicBool::new(false));
    let mut voice_chat_player: Option<crate::live_control::AudioOutputPlayer> = None;
    let mut voice_chat_invite_udp_endpoint: Option<AudioUdpEndpoint> = None;
    let mut voice_chat_udp_stop: Option<Arc<AtomicBool>> = None;
    loop {
        while let Ok(input) = input_rx.try_recv() {
            match input {
                ClientInput::ChatReply { text } => queue_message(
                    &out_tx,
                    &session_token,
                    Message::CommandAck {
                        client_id: identity.id.clone(),
                        command: CommandKind::TextChat,
                        accepted: true,
                        detail: format!("chat_message:{text}"),
                    },
                )?,
                ClientInput::VoiceChatAccept => {
                    voice_chat_stream.running.store(false, Ordering::Relaxed);
                    if let Some(stop) = voice_chat_udp_stop.take() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    let generation = voice_chat_stream
                        .generation
                        .fetch_add(1, Ordering::Relaxed)
                        .saturating_add(1);
                    let endpoint = match voice_chat_invite_udp_endpoint.clone() {
                        Some(endpoint) => endpoint,
                        None => {
                            voice_chat_stream.running.store(false, Ordering::Relaxed);
                            let error = "voice chat udp transport unavailable".to_string();
                            let _ = queue_message(
                                &out_tx,
                                &session_token,
                                Message::CommandAck {
                                    client_id: identity.id.clone(),
                                    command: CommandKind::VoiceChat,
                                    accepted: false,
                                    detail: format!("voice_chat_error\nmessage={error}"),
                                },
                            );
                            event_sink.send(ClientEvent::VoiceChatFailed { message: error });
                            continue;
                        }
                    };
                    let player = match crate::live_control::AudioOutputPlayer::start() {
                        Ok(player) => player,
                        Err(error) => {
                            voice_chat_stream.running.store(false, Ordering::Relaxed);
                            let _ = queue_message(
                                &out_tx,
                                &session_token,
                                Message::CommandAck {
                                    client_id: identity.id.clone(),
                                    command: CommandKind::VoiceChat,
                                    accepted: false,
                                    detail: format!("voice_chat_error\nmessage={error}"),
                                },
                            );
                            event_sink.send(ClientEvent::VoiceChatFailed { message: error });
                            continue;
                        }
                    };
                    let udp_sender = match AudioUdpSender::connect(endpoint.clone()) {
                        Ok(sender) => sender,
                        Err(error) => {
                            voice_chat_stream.running.store(false, Ordering::Relaxed);
                            let _ = queue_message(
                                &out_tx,
                                &session_token,
                                Message::CommandAck {
                                    client_id: identity.id.clone(),
                                    command: CommandKind::VoiceChat,
                                    accepted: false,
                                    detail: format!("voice_chat_error\nmessage={error}"),
                                },
                            );
                            event_sink.send(ClientEvent::VoiceChatFailed { message: error });
                            continue;
                        }
                    };
                    let receive_stream_id = endpoint
                        .return_stream_id
                        .unwrap_or_else(|| new_audio_udp_stream_id(generation));
                    let socket = match UdpSocket::bind("0.0.0.0:0") {
                        Ok(socket) => socket,
                        Err(error) => {
                            voice_chat_stream.running.store(false, Ordering::Relaxed);
                            let message = format!("bind udp failed: {error}");
                            let _ = queue_message(
                                &out_tx,
                                &session_token,
                                Message::CommandAck {
                                    client_id: identity.id.clone(),
                                    command: CommandKind::VoiceChat,
                                    accepted: false,
                                    detail: format!("voice_chat_error\nmessage={message}"),
                                },
                            );
                            event_sink.send(ClientEvent::VoiceChatFailed { message });
                            continue;
                        }
                    };
                    if let Err(error) = socket
                        .set_read_timeout(Some(Duration::from_millis(AUDIO_UDP_RECV_TIMEOUT_MS)))
                    {
                        voice_chat_stream.running.store(false, Ordering::Relaxed);
                        let message = format!("udp timeout setup failed: {error}");
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::CommandAck {
                                client_id: identity.id.clone(),
                                command: CommandKind::VoiceChat,
                                accepted: false,
                                detail: format!("voice_chat_error\nmessage={message}"),
                            },
                        );
                        event_sink.send(ClientEvent::VoiceChatFailed { message });
                        continue;
                    }
                    let udp_stop = Arc::new(AtomicBool::new(false));
                    let sink = player.sink();
                    let speaker_muted = voice_chat_speaker_muted.clone();
                    let worker_stop = udp_stop.clone();
                    let worker_event_sink = event_sink.clone();
                    let worker_server_addr = endpoint.addr();
                    event_sink.send(ClientEvent::Log(format!(
                        "voice udp receiver ready stream={receive_stream_id} relay={worker_server_addr}"
                    )));
                    thread::spawn(move || {
                        audio_udp_receive_loop(
                            socket,
                            worker_server_addr,
                            receive_stream_id,
                            worker_stop,
                            sink,
                            speaker_muted,
                            worker_event_sink,
                        );
                    });
                    voice_chat_player = Some(player);
                    voice_chat_udp_stop = Some(udp_stop);
                    voice_chat_stream.running.store(true, Ordering::Relaxed);
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: identity.id.clone(),
                            command: CommandKind::VoiceChat,
                            accepted: true,
                            detail: format!(
                                "voice_chat_accepted\nmessage=accepted\ngeneration={generation}\ntransport=udp\nudp_host={}\nudp_port={}\nudp_stream={receive_stream_id}",
                                endpoint.host, endpoint.port
                            ),
                        },
                    );
                    event_sink.send(ClientEvent::VoiceChatConnected);
                    let worker_tx = out_tx.clone();
                    let worker_token = session_token.clone();
                    let stream_state = voice_chat_stream.clone();
                    let mic_muted = voice_chat_mic_muted.clone();
                    let worker_event_sink = event_sink.clone();
                    let client_id = identity.id.clone();
                    thread::spawn(move || {
                        voice_chat_capture_loop(
                            client_id,
                            udp_sender,
                            worker_tx,
                            worker_token,
                            stream_state,
                            generation,
                            mic_muted,
                            worker_event_sink,
                        );
                    });
                }
                ClientInput::VoiceChatDecline => {
                    voice_chat_stream.running.store(false, Ordering::Relaxed);
                    voice_chat_stream.generation.fetch_add(1, Ordering::Relaxed);
                    if let Some(stop) = voice_chat_udp_stop.take() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    let _ = voice_chat_player.take();
                    voice_chat_invite_udp_endpoint = None;
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: identity.id.clone(),
                            command: CommandKind::VoiceChat,
                            accepted: false,
                            detail: "voice_chat_declined\nmessage=declined".to_string(),
                        },
                    );
                    event_sink.send(ClientEvent::VoiceChatEnded {
                        message: "Declined".to_string(),
                    });
                }
                ClientInput::VoiceChatEnd => {
                    voice_chat_stream.running.store(false, Ordering::Relaxed);
                    voice_chat_stream.generation.fetch_add(1, Ordering::Relaxed);
                    if let Some(stop) = voice_chat_udp_stop.take() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    let _ = voice_chat_player.take();
                    voice_chat_invite_udp_endpoint = None;
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: identity.id.clone(),
                            command: CommandKind::VoiceChat,
                            accepted: true,
                            detail: "voice_chat_ended\nmessage=ended".to_string(),
                        },
                    );
                    event_sink.send(ClientEvent::VoiceChatEnded {
                        message: "Call ended".to_string(),
                    });
                }
                ClientInput::VoiceChatMicMuted { muted } => {
                    voice_chat_mic_muted.store(muted, Ordering::Relaxed);
                }
                ClientInput::VoiceChatSpeakerMuted { muted } => {
                    voice_chat_speaker_muted.store(muted, Ordering::Relaxed);
                }
            }
        }

        let Some(message) = (match decoder.read_next(&mut reader) {
            Ok(Some(envelope)) => Some(envelope.message),
            Ok(None) => {
                thread::sleep(Duration::from_millis(NETWORK_IDLE_SLEEP_MS));
                continue;
            }
            Err(error) => {
                event_sink.send(ClientEvent::Log(format!("network read failed: {error}")));
                break;
            }
        }) else {
            continue;
        };

        match message {
            Message::Session { token } => {
                session_token = token;
                event_sink.send(ClientEvent::Connected);
            }
            Message::Command {
                target_id,
                command,
                payload,
            } => {
                event_sink.send(ClientEvent::Command {
                    command: command.clone(),
                    payload: payload.clone(),
                });
                if command == CommandKind::ClientConfig {
                    let update = crate::runtime::update_client_config(&config, &payload);
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: target_id,
                            command: CommandKind::ClientConfig,
                            accepted: update.accepted,
                            detail: update.detail,
                        },
                    );
                    if update.reconnect {
                        break;
                    }
                    continue;
                }
                if command == CommandKind::TextChat && gui_mode {
                    event_sink.send(ClientEvent::ChatMessage {
                        text: payload.clone(),
                    });
                }
                if command == CommandKind::VoiceChat {
                    voice_chat_stream.running.store(false, Ordering::Relaxed);
                    voice_chat_stream.generation.fetch_add(1, Ordering::Relaxed);
                    voice_chat_mic_muted.store(false, Ordering::Relaxed);
                    voice_chat_speaker_muted.store(false, Ordering::Relaxed);
                    if let Some(stop) = voice_chat_udp_stop.take() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    let _ = voice_chat_player.take();
                    voice_chat_invite_udp_endpoint = match AudioUdpEndpoint::from_payload(&payload)
                    {
                        Ok(endpoint) => endpoint,
                        Err(error) => {
                            event_sink.send(ClientEvent::Log(format!(
                                "voice chat udp invite ignored: {error}"
                            )));
                            None
                        }
                    };
                    if gui_mode {
                        event_sink.send(ClientEvent::VoiceChatInvite);
                    } else {
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::CommandAck {
                                client_id: target_id,
                                command: CommandKind::VoiceChat,
                                accepted: false,
                                detail: commands::gui_disabled_detail(&CommandKind::VoiceChat),
                            },
                        );
                    }
                    continue;
                }
                if command == CommandKind::RemoteTerminal {
                    let worker_tx = out_tx.clone();
                    let worker_token = session_token.clone();
                    thread::spawn(move || {
                        let client_id = target_id;
                        let result = crate::remote_management::execute_terminal_streaming(
                            &payload,
                            |output| {
                                queue_message(
                                    &worker_tx,
                                    &worker_token,
                                    Message::CommandOutput {
                                        client_id: client_id.clone(),
                                        command: CommandKind::RemoteTerminal,
                                        stream_id: output.stream_id,
                                        sequence: output.sequence,
                                        stream: output.stream,
                                        chunk: output.chunk,
                                        current_dir: output.current_dir,
                                        finished: output.finished,
                                        success: output.success,
                                    },
                                )
                            },
                        );
                        if let Err(error) = result {
                            let _ = queue_message(
                                &worker_tx,
                                &worker_token,
                                Message::CommandAck {
                                    client_id,
                                    command: CommandKind::RemoteTerminal,
                                    accepted: false,
                                    detail: format!("remote terminal stream failed: {error}"),
                                },
                            );
                        }
                    });
                    continue;
                }
                let worker_tx = out_tx.clone();
                let worker_token = session_token.clone();
                thread::spawn(move || {
                    let reply = commands::handle_command(&command, &payload, gui_mode);
                    let _ = queue_message(
                        &worker_tx,
                        &worker_token,
                        Message::CommandAck {
                            client_id: target_id,
                            command,
                            accepted: reply.accepted,
                            detail: reply.detail,
                        },
                    );
                });
            }
            message @ Message::FileTransfer {
                direction: FileTransferDirection::Download,
                action: FileTransferAction::Start,
                ..
            } => {
                let worker_tx = out_tx.clone();
                let worker_bulk_tx = bulk_out_tx.clone();
                let worker_token = session_token.clone();
                thread::spawn(move || {
                    let result = crate::remote_management::handle_file_transfer(message, |reply| {
                        queue_file_transfer_reply(&worker_tx, &worker_bulk_tx, &worker_token, reply)
                    });
                    if let Err(error) = result {
                        eprintln!("file transfer failed: {error}");
                    }
                });
            }
            message @ Message::FileTransfer { .. } => {
                if let Err(error) =
                    crate::remote_management::handle_file_transfer(message, |reply| {
                        queue_file_transfer_reply(&out_tx, &bulk_out_tx, &session_token, reply)
                    })
                {
                    event_sink.send(ClientEvent::Log(format!("file transfer failed: {error}")));
                }
            }
            Message::DesktopControl { target_id, payload } => {
                if !gui_mode {
                    desktop_stream.running.store(false, Ordering::Relaxed);
                    desktop_stream.generation.fetch_add(1, Ordering::Relaxed);
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: target_id,
                            command: CommandKind::RemoteDesktop,
                            accepted: false,
                            detail: commands::gui_disabled_detail(&CommandKind::RemoteDesktop),
                        },
                    );
                    continue;
                }

                match remote_desktop_action(&payload).as_deref() {
                    Some("start") => {
                        desktop_stream.running.store(false, Ordering::Relaxed);
                        let generation = desktop_stream
                            .generation
                            .fetch_add(1, Ordering::Relaxed)
                            .saturating_add(1);
                        thread::sleep(Duration::from_millis(5));
                        desktop_stream.running.store(true, Ordering::Relaxed);
                        let worker_tx = out_tx.clone();
                        let worker_token = session_token.clone();
                        let stream_state = desktop_stream.clone();
                        thread::spawn(move || {
                            remote_desktop_stream_loop(
                                target_id,
                                payload,
                                worker_tx,
                                worker_token,
                                stream_state,
                                generation,
                            );
                        });
                    }
                    Some("stop") => {
                        desktop_stream.running.store(false, Ordering::Relaxed);
                        desktop_stream.generation.fetch_add(1, Ordering::Relaxed);
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::DesktopFrame {
                                client_id: target_id,
                                payload: "remote_desktop_stopped\nmessage=stopped".to_string(),
                            },
                        );
                    }
                    _ => {
                        let worker_tx = out_tx.clone();
                        let worker_token = session_token.clone();
                        thread::spawn(move || {
                            let payload =
                                crate::live_control::handle(&CommandKind::RemoteDesktop, &payload);
                            let _ = queue_message(
                                &worker_tx,
                                &worker_token,
                                Message::DesktopFrame {
                                    client_id: target_id,
                                    payload,
                                },
                            );
                        });
                    }
                }
            }
            Message::DesktopInput { target_id, payload } => {
                if !gui_mode {
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: target_id,
                            command: CommandKind::RemoteDesktop,
                            accepted: false,
                            detail: commands::gui_disabled_detail(&CommandKind::RemoteDesktop),
                        },
                    );
                    continue;
                }

                let worker_tx = out_tx.clone();
                let worker_token = session_token.clone();
                thread::spawn(move || {
                    let should_reply = !desktop_payload_is_move(&payload);
                    let result = crate::live_control::handle(&CommandKind::RemoteDesktop, &payload);
                    let input_failed = result.starts_with("remote_desktop_error\n");
                    if should_reply || input_failed {
                        let result = desktop_input_reply_payload(result);
                        let _ = queue_message(
                            &worker_tx,
                            &worker_token,
                            Message::DesktopFrame {
                                client_id: target_id,
                                payload: result,
                            },
                        );
                    }
                });
            }
            Message::VideoControl {
                target_id,
                source,
                payload,
            } => match video_control_action(&payload).as_deref() {
                _ if !gui_mode => {
                    let stream_state = match &source {
                        VideoSource::RemoteDesktop => desktop_stream.clone(),
                        VideoSource::Camera => camera_stream.clone(),
                    };
                    stream_state.running.store(false, Ordering::Relaxed);
                    stream_state.generation.fetch_add(1, Ordering::Relaxed);
                    let command = video_source_command(&source);
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: target_id,
                            command: command.clone(),
                            accepted: false,
                            detail: commands::gui_disabled_detail(&command),
                        },
                    );
                }
                Some("start") => {
                    let stream_state = match &source {
                        VideoSource::RemoteDesktop => desktop_stream.clone(),
                        VideoSource::Camera => camera_stream.clone(),
                    };
                    stream_state.running.store(false, Ordering::Relaxed);
                    let generation = stream_state
                        .generation
                        .fetch_add(1, Ordering::Relaxed)
                        .saturating_add(1);
                    thread::sleep(Duration::from_millis(5));
                    stream_state.running.store(true, Ordering::Relaxed);
                    let worker_tx = out_tx.clone();
                    let worker_realtime_tx = bulk_out_tx.clone();
                    let worker_token = session_token.clone();
                    thread::spawn(move || {
                        video_stream_loop(
                            target_id,
                            source,
                            payload,
                            worker_realtime_tx,
                            worker_tx,
                            worker_token,
                            stream_state,
                            generation,
                        );
                    });
                }
                Some("stop") => {
                    let stream_state = match &source {
                        VideoSource::RemoteDesktop => desktop_stream.clone(),
                        VideoSource::Camera => camera_stream.clone(),
                    };
                    stream_state.running.store(false, Ordering::Relaxed);
                    stream_state.generation.fetch_add(1, Ordering::Relaxed);
                    if source == VideoSource::Camera {
                        let _ = crate::live_control::handle(&CommandKind::Camera, "action=stop");
                    }
                    let _ = target_id;
                }
                _ => {}
            },
            Message::AudioControl {
                target_id,
                source,
                payload,
            } => match source {
                AudioSource::AudioListen => match video_control_action(&payload).as_deref() {
                    _ if !gui_mode => {
                        audio_stream.running.store(false, Ordering::Relaxed);
                        audio_stream.generation.fetch_add(1, Ordering::Relaxed);
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::CommandAck {
                                client_id: target_id,
                                command: CommandKind::AudioListen,
                                accepted: false,
                                detail: commands::gui_disabled_detail(&CommandKind::AudioListen),
                            },
                        );
                    }
                    Some("start") => {
                        audio_stream.running.store(false, Ordering::Relaxed);
                        let generation = audio_stream
                            .generation
                            .fetch_add(1, Ordering::Relaxed)
                            .saturating_add(1);
                        thread::sleep(Duration::from_millis(AUDIO_STREAM_STOP_SETTLE_MS));
                        audio_stream.running.store(true, Ordering::Relaxed);
                        let worker_tx = out_tx.clone();
                        let worker_token = session_token.clone();
                        let stream_state = audio_stream.clone();
                        thread::spawn(move || {
                            audio_stream_loop(
                                target_id,
                                payload,
                                worker_tx,
                                worker_token,
                                stream_state,
                                generation,
                            );
                        });
                    }
                    Some("stop") => {
                        audio_stream.running.store(false, Ordering::Relaxed);
                        audio_stream.generation.fetch_add(1, Ordering::Relaxed);
                        thread::sleep(Duration::from_millis(AUDIO_STREAM_STOP_SETTLE_MS));
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::CommandAck {
                                client_id: target_id,
                                command: CommandKind::AudioListen,
                                accepted: true,
                                detail: "audio_listen_stopped\nmessage=stopped".to_string(),
                            },
                        );
                    }
                    _ => {
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::CommandAck {
                                client_id: target_id,
                                command: CommandKind::AudioListen,
                                accepted: false,
                                detail:
                                    "audio_listen_error\nmessage=unsupported audio control action"
                                        .to_string(),
                            },
                        );
                    }
                },
                AudioSource::VoiceChat => match video_control_action(&payload).as_deref() {
                    Some("stop") | Some("end") => {
                        voice_chat_stream.running.store(false, Ordering::Relaxed);
                        voice_chat_stream.generation.fetch_add(1, Ordering::Relaxed);
                        if let Some(stop) = voice_chat_udp_stop.take() {
                            stop.store(true, Ordering::Relaxed);
                        }
                        let _ = voice_chat_player.take();
                        voice_chat_invite_udp_endpoint = None;
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::CommandAck {
                                client_id: target_id,
                                command: CommandKind::VoiceChat,
                                accepted: true,
                                detail: "voice_chat_ended\nmessage=ended".to_string(),
                            },
                        );
                        event_sink.send(ClientEvent::VoiceChatEnded {
                            message: "Call ended".to_string(),
                        });
                    }
                    _ => {}
                },
            },
            Message::Ping => queue_message(&out_tx, &session_token, Message::Pong)?,
            Message::Error { detail } if detail.starts_with("auth failed") => {
                event_sink.send(ClientEvent::Log(format!("server: {detail}")));
                break;
            }
            other => {
                event_sink.send(ClientEvent::Log(format!("server: {other:?}")));
            }
        }
    }

    audio_stream.running.store(false, Ordering::Relaxed);
    voice_chat_stream.running.store(false, Ordering::Relaxed);
    if let Some(stop) = voice_chat_udp_stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    let _ = voice_chat_player.take();

    Ok(())
}

#[cfg(feature = "gui")]
struct ClientApp {
    config: Config,
    identity: LocalIdentity,
    input_tx: Sender<ClientInput>,
    event_rx: Receiver<ClientEvent>,
    connected: bool,
    log_lines: Vec<String>,
    chat_window: Option<user_interaction::text_chat::ChatWindow>,
    voice_chat_window: Option<user_interaction::voice_chat::VoiceChatWindow>,
}

#[cfg(feature = "gui")]
impl ClientApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        identity: LocalIdentity,
        event_rx: Receiver<ClientEvent>,
        input_tx: Sender<ClientInput>,
        repaint_handle: Arc<Mutex<Option<egui::Context>>>,
    ) -> Self {
        apply_client_theme(&cc.egui_ctx);
        if let Ok(mut handle) = repaint_handle.lock() {
            *handle = Some(cc.egui_ctx.clone());
        }
        Self {
            config,
            identity,
            input_tx,
            event_rx,
            connected: false,
            log_lines: vec![timestamped_log(format!(
                "client gui started version={}",
                rdl_version::display_version()
            ))],
            chat_window: None,
            voice_chat_window: None,
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.event_rx.try_recv() {
            changed = true;
            match event {
                ClientEvent::Connected => {
                    self.connected = true;
                    self.push_log("connected to server");
                }
                ClientEvent::Disconnected => {
                    self.connected = false;
                    self.push_log("disconnected from server");
                }
                ClientEvent::Command { command, payload } => {
                    self.push_log(format!(
                        "received command={} payload={payload}",
                        command.as_str()
                    ));
                }
                ClientEvent::ChatMessage { text } => {
                    user_interaction::text_chat::receive_admin_message(&mut self.chat_window, text);
                }
                ClientEvent::VoiceChatInvite => {
                    user_interaction::voice_chat::receive_invite(&mut self.voice_chat_window);
                    self.push_log("incoming voice_chat invite");
                }
                ClientEvent::VoiceChatConnected => {
                    user_interaction::voice_chat::mark_live(&mut self.voice_chat_window);
                    self.push_log("voice_chat connected");
                }
                ClientEvent::VoiceChatEnded { message } => {
                    user_interaction::voice_chat::mark_ended(
                        &mut self.voice_chat_window,
                        message.clone(),
                    );
                    self.push_log(format!("voice_chat ended: {message}"));
                }
                ClientEvent::VoiceChatFailed { message } => {
                    user_interaction::voice_chat::mark_failed(
                        &mut self.voice_chat_window,
                        message.clone(),
                    );
                    self.push_log(format!("voice_chat failed: {message}"));
                }
                ClientEvent::Log(line) => self.push_log(line),
            }
        }
        changed
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let line = timestamped_log(line);
        eprintln!("{line}");
        self.log_lines.push(line);
        prune_activity_logs(&mut self.log_lines);
    }

    fn render_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Rust Desk Light")
                        .size(22.0)
                        .color(COLOR_TEXT)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "Client Agent | {}",
                        rdl_version::display_version()
                    ))
                    .size(13.0)
                    .color(COLOR_MUTED),
                );
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_pill(ui, self.connected);
            });
        });
    }

    fn render_status(&self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Status");
            ui.add_space(10.0);
            egui::Grid::new("client_status_grid")
                .num_columns(2)
                .spacing([18.0, 10.0])
                .show(ui, |ui| {
                    detail_row(
                        ui,
                        "Connection",
                        if self.connected {
                            "Online"
                        } else {
                            "Connecting / Offline"
                        },
                    );
                    detail_row(ui, "Client ID", &self.identity.id);
                    detail_row(ui, "Fingerprint", &self.identity.fingerprint);
                    detail_row(
                        ui,
                        "Server",
                        &format!("{}:{}", self.config.ip, self.config.port),
                    );
                    detail_row(ui, "Version", &rdl_version::display_version());
                    detail_row(ui, "Host", &hostname());
                    detail_row(
                        ui,
                        "Runtime",
                        &format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
                    );
                    detail_row(ui, "User", &username());
                });
        });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Activity");
            ui.add_space(8.0);
            let output = egui::ScrollArea::vertical()
                .id_salt("client_activity_scroll_area")
                .stick_to_bottom(true)
                .max_height(180.0)
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(ui.available_width());
                        for line in &self.log_lines {
                            ui.label(egui::RichText::new(line).size(12.0).color(COLOR_MUTED));
                        }
                    });
                });
            activity_context_menu(ui, output.inner_rect, output.id, &mut self.log_lines);
        });
    }
}

#[cfg(feature = "gui")]
impl eframe::App for ClientApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let changed = self.drain_events();

        ui.painter().rect_filled(ui.max_rect(), 0.0, COLOR_BG);
        ui.add_space(18.0);
        ui.vertical_centered_justified(|ui| {
            ui.set_max_width(700.0);
            self.render_header(ui);
            ui.add_space(14.0);
            self.render_status(ui);
            ui.add_space(12.0);
            self.render_activity(ui);
        });
        for text in user_interaction::text_chat::render_window(ui.ctx(), &mut self.chat_window) {
            let _ = self.input_tx.send(ClientInput::ChatReply { text });
        }
        for action in
            user_interaction::voice_chat::render_window(ui.ctx(), &mut self.voice_chat_window)
        {
            match action {
                user_interaction::voice_chat::VoiceChatAction::Accept => {
                    user_interaction::voice_chat::mark_connecting(&mut self.voice_chat_window);
                    let _ = self.input_tx.send(ClientInput::VoiceChatAccept);
                }
                user_interaction::voice_chat::VoiceChatAction::Decline => {
                    let _ = self.input_tx.send(ClientInput::VoiceChatDecline);
                }
                user_interaction::voice_chat::VoiceChatAction::End => {
                    let _ = self.input_tx.send(ClientInput::VoiceChatEnd);
                }
                user_interaction::voice_chat::VoiceChatAction::MicMuted(muted) => {
                    let _ = self.input_tx.send(ClientInput::VoiceChatMicMuted { muted });
                }
                user_interaction::voice_chat::VoiceChatAction::SpeakerMuted(muted) => {
                    let _ = self
                        .input_tx
                        .send(ClientInput::VoiceChatSpeakerMuted { muted });
                }
            }
        }

        if changed {
            ui.ctx().request_repaint();
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(GUI_FRAME_INTERVAL_MS));
        }
    }
}

#[cfg(feature = "gui")]
const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
#[cfg(feature = "gui")]
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
#[cfg(feature = "gui")]
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
#[cfg(feature = "gui")]
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
#[cfg(feature = "gui")]
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
#[cfg(feature = "gui")]
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
#[cfg(feature = "gui")]
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
#[cfg(feature = "gui")]
const ACTIVITY_LOG_LIMIT: usize = 300;

#[cfg(feature = "gui")]
fn apply_client_theme(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.visuals = egui::Visuals::light();
    style.visuals.window_fill = COLOR_PANEL;
    style.visuals.panel_fill = COLOR_BG;
    style.visuals.widgets.noninteractive.fg_stroke.color = COLOR_TEXT;
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(238, 242, 247);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(226, 234, 244);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(216, 228, 242);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(216, 232, 252);
    ctx.set_global_style(style);
}

#[cfg(feature = "gui")]
fn panel(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(8.0)
        .inner_margin(14.0)
        .show(ui, add_contents);
}

#[cfg(feature = "gui")]
fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(14.0)
            .color(COLOR_TEXT)
            .strong(),
    );
}

#[cfg(feature = "gui")]
fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).color(COLOR_MUTED));
    ui.label(egui::RichText::new(value).color(COLOR_TEXT).strong());
    ui.end_row();
}

#[cfg(feature = "gui")]
fn timestamped_log(line: impl Into<String>) -> String {
    format!("[{}] {}", activity_time_label(), line.into())
}

#[cfg(feature = "gui")]
fn prune_activity_logs(log_lines: &mut Vec<String>) {
    if log_lines.len() > ACTIVITY_LOG_LIMIT {
        log_lines.drain(0..log_lines.len() - ACTIVITY_LOG_LIMIT);
    }
}

#[cfg(feature = "gui")]
fn activity_context_menu(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    log_lines: &mut Vec<String>,
) {
    ui.interact(rect, id.with("activity_context_menu"), egui::Sense::click())
        .context_menu(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(log_lines.join("\n"));
                ui.close();
            }
            if ui.button("Clear").clicked() {
                log_lines.clear();
                ui.close();
            }
        });
}

#[cfg(feature = "gui")]
fn activity_time_label() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let china_time = now + 8 * 60 * 60;
    let seconds_today = china_time % (24 * 60 * 60);
    let hour = seconds_today / 3600;
    let minute = (seconds_today % 3600) / 60;
    let second = seconds_today % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

#[cfg(feature = "gui")]
fn status_pill(ui: &mut egui::Ui, connected: bool) {
    let (text, color) = if connected {
        ("Online", COLOR_GOOD)
    } else {
        ("Offline", COLOR_BAD)
    };
    egui::Frame::default()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(999.0)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(color).strong());
        });
}

#[derive(Debug)]
enum ClientEvent {
    Connected,
    Disconnected,
    Command {
        command: CommandKind,
        payload: String,
    },
    ChatMessage {
        text: String,
    },
    VoiceChatInvite,
    VoiceChatConnected,
    VoiceChatEnded {
        message: String,
    },
    VoiceChatFailed {
        message: String,
    },
    Log(String),
}

#[derive(Clone)]
struct ClientEventSink {
    tx: Sender<ClientEvent>,
    #[cfg(feature = "gui")]
    repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
}

impl ClientEventSink {
    #[cfg(feature = "gui")]
    fn new(
        tx: Sender<ClientEvent>,
        repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
    ) -> Self {
        Self { tx, repaint_handle }
    }

    #[cfg(not(feature = "gui"))]
    fn new(tx: Sender<ClientEvent>, _repaint_handle: Option<()>) -> Self {
        Self { tx }
    }

    fn send(&self, event: ClientEvent) {
        let _ = self.tx.send(event);
        #[cfg(feature = "gui")]
        if let Some(ctx) = self
            .repaint_handle
            .as_ref()
            .and_then(|handle| handle.lock().ok().and_then(|ctx| ctx.clone()))
        {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

#[cfg_attr(not(feature = "gui"), allow(dead_code))]
enum ClientInput {
    ChatReply { text: String },
    VoiceChatAccept,
    VoiceChatDecline,
    VoiceChatEnd,
    VoiceChatMicMuted { muted: bool },
    VoiceChatSpeakerMuted { muted: bool },
}

struct ClientOutbound {
    session_token: String,
    message: Message,
}

struct DesktopStreamState {
    running: AtomicBool,
    generation: std::sync::atomic::AtomicU64,
}

#[derive(Default)]
struct AudioFramePacketizer {
    sample_rate: u32,
    channels: u16,
    format: String,
    frame_bytes: usize,
    pending: Vec<u8>,
}

impl AudioFramePacketizer {
    fn clear_pending(&mut self) {
        self.pending.clear();
    }

    fn push(
        &mut self,
        frame: crate::live_control::CapturedAudioFrame,
    ) -> Vec<crate::live_control::CapturedAudioFrame> {
        if frame.bytes.is_empty() {
            return Vec::new();
        }
        if self.sample_rate != frame.sample_rate
            || self.channels != frame.channels
            || self.format != frame.format
        {
            self.sample_rate = frame.sample_rate;
            self.channels = frame.channels;
            self.format = frame.format.clone();
            self.frame_bytes = audio_capture_frame_bytes(frame.sample_rate, frame.channels);
            self.pending.clear();
        }
        self.pending.extend(frame.bytes);

        let mut frames = Vec::new();
        while self.pending.len() >= self.frame_bytes {
            let bytes: Vec<u8> = self.pending.drain(..self.frame_bytes).collect();
            frames.push(crate::live_control::CapturedAudioFrame {
                sample_rate: self.sample_rate,
                channels: self.channels,
                format: self.format.clone(),
                bytes,
            });
        }
        frames
    }
}

fn audio_capture_frame_bytes(sample_rate: u32, channels: u16) -> usize {
    let samples_per_channel =
        ((sample_rate.max(1) as u64 * AUDIO_CAPTURE_FRAME_MS as u64) / 1000).max(1) as usize;
    let target_bytes = samples_per_channel * channels.max(1) as usize * 2;
    target_bytes.min(max_pcm_s16le_udp_payload_bytes(channels))
}

fn max_pcm_s16le_udp_payload_bytes(channels: u16) -> usize {
    let sample_frame_bytes = channels.max(1) as usize * 2;
    (AUDIO_UDP_MAX_PAYLOAD_BYTES / sample_frame_bytes).max(1) * sample_frame_bytes
}

struct AudioUdpSender {
    socket: UdpSocket,
    stream_id: u64,
    packet: Vec<u8>,
}

#[derive(Clone)]
struct AudioUdpEndpoint {
    host: String,
    port: u16,
    stream_id: u64,
    return_stream_id: Option<u64>,
}

impl AudioUdpSender {
    fn from_payload(payload: &str) -> Result<Option<Self>, String> {
        AudioUdpEndpoint::from_payload(payload)
            .map(|endpoint| endpoint.map(Self::connect))
            .and_then(|sender| sender.transpose())
    }

    fn connect(endpoint: AudioUdpEndpoint) -> Result<Self, String> {
        let socket =
            UdpSocket::bind("0.0.0.0:0").map_err(|error| format!("bind udp failed: {error}"))?;
        socket
            .connect(endpoint.addr())
            .map_err(|error| format!("connect udp relay failed: {error}"))?;
        Ok(Self {
            socket,
            stream_id: endpoint.stream_id,
            packet: Vec::with_capacity(audio_udp::MAX_PACKET_BYTES),
        })
    }

    fn send_frame(
        &mut self,
        seq: u64,
        frame: &crate::live_control::CapturedAudioFrame,
    ) -> io::Result<()> {
        audio_udp::encode_audio(
            self.stream_id,
            seq,
            now_epoch_ms() as u64,
            frame.sample_rate,
            frame.channels,
            &frame.format,
            &frame.bytes,
            &mut self.packet,
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.socket.send(&self.packet).map(|_| ())
    }
}

impl AudioUdpEndpoint {
    fn from_payload(payload: &str) -> Result<Option<Self>, String> {
        if video_control_value(payload, "transport").as_deref() != Some("udp") {
            return Ok(None);
        }
        let host = video_control_value(payload, "udp_host")
            .ok_or_else(|| "missing audio udp host".to_string())?;
        let port = video_control_value(payload, "udp_port")
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| "missing audio udp port".to_string())?;
        let stream_id = video_control_value(payload, "udp_stream")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| "missing audio udp stream".to_string())?;
        let return_stream_id =
            video_control_value(payload, "udp_return_stream").and_then(|value| value.parse().ok());
        Ok(Some(Self {
            host,
            port,
            stream_id,
            return_stream_id,
        }))
    }

    fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn new_audio_udp_stream_id(tag: u64) -> u64 {
    ((now_epoch_ms() as u64).saturating_mul(1024))
        .saturating_add(tag)
        .max(1)
}

fn audio_udp_receive_loop(
    socket: UdpSocket,
    server_addr: String,
    stream_id: u64,
    stop: Arc<AtomicBool>,
    sink: crate::live_control::AudioOutputSink,
    speaker_muted: Arc<AtomicBool>,
    event_sink: ClientEventSink,
) {
    let mut register_packet = Vec::new();
    let mut unregister_packet = Vec::new();
    audio_udp::encode_register(stream_id, &mut register_packet);
    audio_udp::encode_unregister(stream_id, &mut unregister_packet);
    let mut last_register = Instant::now() - Duration::from_millis(AUDIO_UDP_REGISTER_INTERVAL_MS);
    let mut buf = [0_u8; audio_udp::MAX_PACKET_BYTES];
    let mut last_seq = 0_u64;
    let mut received_packets = 0_u64;
    let mut received_bytes = 0_u64;
    let mut duplicate_drops = 0_u64;
    let mut muted_drops = 0_u64;
    let mut playback_errors = 0_u64;
    let mut last_report = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        if last_register.elapsed() >= Duration::from_millis(AUDIO_UDP_REGISTER_INTERVAL_MS) {
            if let Err(error) = socket.send_to(&register_packet, &server_addr) {
                event_sink.send(ClientEvent::Log(format!(
                    "voice udp register failed: {error}"
                )));
                break;
            }
            last_register = Instant::now();
        }

        match socket.recv_from(&mut buf) {
            Ok((len, _)) => match audio_udp::decode(&buf[..len]) {
                Ok(audio_udp::Packet::Audio {
                    stream_id: packet_stream_id,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                    ..
                }) if packet_stream_id == stream_id => {
                    if seq <= last_seq {
                        duplicate_drops = duplicate_drops.saturating_add(1);
                        continue;
                    }
                    last_seq = seq;
                    received_packets = received_packets.saturating_add(1);
                    received_bytes = received_bytes.saturating_add(bytes.len() as u64);
                    if speaker_muted.load(Ordering::Relaxed) {
                        muted_drops = muted_drops.saturating_add(1);
                        continue;
                    }
                    if let Err(error) = sink.push_frame(sample_rate, channels, format, bytes) {
                        playback_errors = playback_errors.saturating_add(1);
                        event_sink.send(ClientEvent::Log(format!(
                            "voice udp playback failed: {error}"
                        )));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    event_sink.send(ClientEvent::Log(format!(
                        "voice udp packet ignored: {error}"
                    )));
                }
            },
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => {
                event_sink.send(ClientEvent::Log(format!(
                    "voice udp receive failed: {error}"
                )));
                break;
            }
        }

        if last_report.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
            debug_log!(
                "debug event=voice_chat_rx transport=udp stream={} packets={} bytes={} muted_drops={} duplicate_drops={} playback_errors={} last_seq={}",
                stream_id,
                received_packets,
                received_bytes,
                muted_drops,
                duplicate_drops,
                playback_errors,
                last_seq
            );
            last_report = Instant::now();
        }
    }

    let _ = socket.send_to(&unregister_packet, server_addr);
}

fn client_writer_loop(
    mut writer: TcpStream,
    out_rx: Receiver<ClientOutbound>,
    bulk_out_rx: Receiver<ClientOutbound>,
) {
    let mut next_message_id = 1u64;
    let mut out_open = true;
    let mut bulk_open = true;
    while out_open || bulk_open {
        loop {
            match out_rx.try_recv() {
                Ok(outbound) => {
                    if !write_client_outbound(&mut writer, &mut next_message_id, outbound) {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    out_open = false;
                    break;
                }
            }
        }

        if !bulk_open {
            match out_rx.recv_timeout(Duration::from_millis(CLIENT_BULK_POLL_MS)) {
                Ok(outbound) => {
                    out_open = true;
                    if !write_client_outbound(&mut writer, &mut next_message_id, outbound) {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => out_open = false,
            }
            continue;
        }

        match bulk_out_rx.recv_timeout(Duration::from_millis(CLIENT_BULK_POLL_MS)) {
            Ok(outbound) => {
                if !write_client_outbound(&mut writer, &mut next_message_id, outbound) {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !out_open && !bulk_open {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bulk_open = false;
            }
        }
    }
}

fn write_client_outbound(
    writer: &mut TcpStream,
    next_message_id: &mut u64,
    outbound: ClientOutbound,
) -> bool {
    let fallback = command_ack_send_failure(&outbound.message);
    if let Err(error) = send(
        writer,
        next_message_id,
        &outbound.session_token,
        outbound.message,
    ) {
        if let Some(message) = fallback {
            eprintln!("client write failed, sending command error ack: {error}");
            if let Err(fallback_error) = send(
                writer,
                next_message_id,
                &outbound.session_token,
                message(error),
            ) {
                eprintln!("client fallback write failed: {fallback_error}");
                return false;
            }
            return true;
        }
        eprintln!("client write failed: {error}");
        return false;
    }
    true
}

fn command_ack_send_failure(
    message: &Message,
) -> Option<Box<dyn FnOnce(io::Error) -> Message + Send + 'static>> {
    let Message::CommandAck {
        client_id,
        command,
        accepted: _,
        detail: _,
    } = message
    else {
        return None;
    };
    let client_id = client_id.clone();
    let command = command.clone();
    Some(Box::new(move |error| Message::CommandAck {
        client_id,
        command,
        accepted: false,
        detail: format!("client failed to send command result: {error}"),
    }))
}

fn desktop_payload_is_move(payload: &str) -> bool {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| action.trim() == "move")
        .unwrap_or(false)
}

fn desktop_input_reply_payload(result: String) -> String {
    let Some(message) = remote_desktop_error_message(&result) else {
        return result;
    };
    format!("remote_desktop_input\nmessage=input failed: {message}")
}

fn remote_desktop_error_message(detail: &str) -> Option<String> {
    let mut lines = detail.lines();
    if lines.next().unwrap_or_default().trim() != "remote_desktop_error" {
        return None;
    }
    let message = detail
        .lines()
        .find_map(|line| line.strip_prefix("message="))
        .unwrap_or("remote desktop input failed")
        .replace(['\t', '\r', '\n'], " ");
    Some(message)
}

fn remote_desktop_action(payload: &str) -> Option<String> {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| action.trim().to_ascii_lowercase())
}

fn remote_desktop_value(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

fn remote_desktop_stream_loop(
    client_id: String,
    start_payload: String,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
) {
    let screen = remote_desktop_value(&start_payload, "screen")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let quality =
        remote_desktop_value(&start_payload, "quality").unwrap_or_else(|| "medium".to_string());
    let fps = quality_fps(&quality);
    let interval = Duration::from_millis((1000 / fps).max(1));
    while stream_state.running.load(Ordering::Relaxed)
        && stream_state.generation.load(Ordering::Relaxed) == generation
    {
        let started = std::time::Instant::now();
        let payload = crate::live_control::handle(
            &CommandKind::RemoteDesktop,
            &format!("action=screenshot\nscreen={screen}\nquality={quality}"),
        );
        if queue_message(
            &out_tx,
            &session_token,
            Message::DesktopFrame {
                client_id: client_id.clone(),
                payload,
            },
        )
        .is_err()
        {
            stream_state.running.store(false, Ordering::Relaxed);
            break;
        }
        let elapsed = started.elapsed();
        if elapsed < interval {
            thread::sleep(interval - elapsed);
        }
    }
}

fn video_stream_loop(
    client_id: String,
    source: VideoSource,
    start_payload: String,
    realtime_tx: SyncSender<ClientOutbound>,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
) {
    let quality = remote_desktop_value(&start_payload, "quality")
        .or_else(|| video_control_value(&start_payload, "quality"))
        .unwrap_or_else(|| "medium".to_string());
    let fps = quality_fps(&quality);
    let interval = Duration::from_millis((1000 / fps).max(1));
    let mut seq = stream_sequence_base(generation);
    while stream_state.running.load(Ordering::Relaxed)
        && stream_state.generation.load(Ordering::Relaxed) == generation
    {
        let started = std::time::Instant::now();
        let frame = match &source {
            VideoSource::RemoteDesktop => {
                let screen = video_control_value(&start_payload, "screen")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or_default();
                crate::live_control::capture_remote_desktop_video_frame(screen, &quality).map(
                    |frame| Message::VideoFrame {
                        client_id: client_id.clone(),
                        source: VideoSource::RemoteDesktop,
                        seq,
                        source_width: frame.source_width,
                        source_height: frame.source_height,
                        image_width: frame.image_width,
                        image_height: frame.image_height,
                        format: frame.format,
                        bytes: frame.bytes,
                    },
                )
            }
            VideoSource::Camera => {
                let device = video_control_value(&start_payload, "device")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or_default();
                crate::live_control::capture_camera_video_frame(device, &quality).map(|frame| {
                    Message::VideoFrame {
                        client_id: client_id.clone(),
                        source: VideoSource::Camera,
                        seq,
                        source_width: frame.width,
                        source_height: frame.height,
                        image_width: frame.width,
                        image_height: frame.height,
                        format: frame.format,
                        bytes: frame.bytes,
                    }
                })
            }
        };
        match frame {
            Ok(message) => {
                if try_queue_realtime_message(&realtime_tx, &session_token, message).is_err() {
                    stream_state.running.store(false, Ordering::Relaxed);
                    break;
                }
                seq = seq.saturating_add(1);
            }
            Err(error) => {
                let _ = queue_message(
                    &out_tx,
                    &session_token,
                    Message::CommandAck {
                        client_id: client_id.clone(),
                        command: video_source_command(&source),
                        accepted: false,
                        detail: error,
                    },
                );
                stream_state.running.store(false, Ordering::Relaxed);
                break;
            }
        }
        let elapsed = started.elapsed();
        if elapsed < interval {
            thread::sleep(interval - elapsed);
        }
    }
}

fn audio_stream_loop(
    client_id: String,
    start_payload: String,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
) {
    let mut udp_sender = match AudioUdpSender::from_payload(&start_payload) {
        Ok(Some(sender)) => sender,
        Ok(None) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::AudioListen,
                    accepted: false,
                    detail: "audio_listen_error\nmessage=udp transport required".to_string(),
                },
            );
            return;
        }
        Err(error) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::AudioListen,
                    accepted: false,
                    detail: format!("audio_listen_error\nmessage={error}"),
                },
            );
            return;
        }
    };

    if let Err(error) = crate::live_control::confirm_audio_listen() {
        stream_state.running.store(false, Ordering::Relaxed);
        let _ = queue_message(
            &out_tx,
            &session_token,
            Message::CommandAck {
                client_id,
                command: CommandKind::AudioListen,
                accepted: false,
                detail: format!("audio_listen_error\nmessage={error}"),
            },
        );
        return;
    }

    let device = video_control_value(&start_payload, "device")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let (frame_tx, frame_rx) = mpsc::sync_channel(8);
    let input_stream = match crate::live_control::start_audio_input_stream(device, frame_tx) {
        Ok(stream) => stream,
        Err(error) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::AudioListen,
                    accepted: false,
                    detail: format!("audio_listen_error\nmessage={error}"),
                },
            );
            return;
        }
    };
    let _ = queue_message(
        &out_tx,
        &session_token,
        Message::CommandAck {
            client_id: client_id.clone(),
            command: CommandKind::AudioListen,
            accepted: true,
            detail: format!(
                "audio_listen_started\nsample_rate={}\nchannels={}\nformat={}\ngeneration={generation}\ntransport=udp",
                input_stream.sample_rate, input_stream.channels, input_stream.format
            ),
        },
    );

    let mut seq = stream_sequence_base(generation);
    let mut packetizer = AudioFramePacketizer::default();
    let mut sent_packets = 0_u64;
    let mut sent_bytes = 0_u64;
    let mut last_report = Instant::now();
    while stream_state.running.load(Ordering::Relaxed)
        && stream_state.generation.load(Ordering::Relaxed) == generation
    {
        let frame = match frame_rx
            .recv_timeout(Duration::from_millis(AUDIO_CAPTURE_RECV_TIMEOUT_MS))
        {
            Ok(frame) => frame,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let detail = "audio_listen_error\nmessage=audio input stream stopped unexpectedly"
                    .to_string();
                let _ = queue_message(
                    &out_tx,
                    &session_token,
                    Message::CommandAck {
                        client_id: client_id.clone(),
                        command: CommandKind::AudioListen,
                        accepted: false,
                        detail,
                    },
                );
                break;
            }
        };
        for frame in packetizer.push(frame) {
            let frame_bytes = frame.bytes.len() as u64;
            match udp_sender.send_frame(seq, &frame) {
                Ok(()) => {
                    sent_packets = sent_packets.saturating_add(1);
                    sent_bytes = sent_bytes.saturating_add(frame_bytes);
                }
                Err(error) => {
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: client_id.clone(),
                            command: CommandKind::AudioListen,
                            accepted: false,
                            detail: format!("audio_listen_error\nmessage=udp send failed: {error}"),
                        },
                    );
                    stream_state.running.store(false, Ordering::Relaxed);
                    break;
                }
            }
            seq = seq.saturating_add(1);
        }
        if last_report.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
            debug_log!(
                "debug event=audio_listen_tx client={} transport={} packets={} bytes={} queue_drops={} capture_drops={} pending_bytes={}",
                client_id,
                "udp",
                sent_packets,
                sent_bytes,
                0,
                input_stream.dropped_callbacks.load(Ordering::Relaxed),
                packetizer.pending.len()
            );
            last_report = Instant::now();
        }
    }
    drop(input_stream);
}

fn voice_chat_capture_loop(
    client_id: String,
    mut udp_sender: AudioUdpSender,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
    mic_muted: Arc<AtomicBool>,
    event_sink: ClientEventSink,
) {
    let (frame_tx, frame_rx) = mpsc::sync_channel(8);
    let input_stream = match crate::live_control::start_audio_input_stream(0, frame_tx) {
        Ok(stream) => stream,
        Err(error) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::VoiceChat,
                    accepted: false,
                    detail: format!("voice_chat_error\nmessage={error}"),
                },
            );
            event_sink.send(ClientEvent::VoiceChatFailed { message: error });
            return;
        }
    };

    let mut seq = stream_sequence_base(generation);
    let mut packetizer = AudioFramePacketizer::default();
    let mut sent_packets = 0_u64;
    let mut sent_bytes = 0_u64;
    let mut last_report = Instant::now();
    while stream_state.running.load(Ordering::Relaxed)
        && stream_state.generation.load(Ordering::Relaxed) == generation
    {
        let frame =
            match frame_rx.recv_timeout(Duration::from_millis(AUDIO_CAPTURE_RECV_TIMEOUT_MS)) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let message = "audio input stream stopped unexpectedly".to_string();
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: client_id.clone(),
                            command: CommandKind::VoiceChat,
                            accepted: false,
                            detail: format!("voice_chat_error\nmessage={message}"),
                        },
                    );
                    event_sink.send(ClientEvent::VoiceChatFailed { message });
                    break;
                }
            };
        if mic_muted.load(Ordering::Relaxed) {
            packetizer.clear_pending();
            continue;
        }
        for frame in packetizer.push(frame) {
            let frame_bytes = frame.bytes.len() as u64;
            match udp_sender.send_frame(seq, &frame) {
                Ok(()) => {
                    sent_packets = sent_packets.saturating_add(1);
                    sent_bytes = sent_bytes.saturating_add(frame_bytes);
                }
                Err(error) => {
                    let message = format!("udp send failed: {error}");
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: client_id.clone(),
                            command: CommandKind::VoiceChat,
                            accepted: false,
                            detail: format!("voice_chat_error\nmessage={message}"),
                        },
                    );
                    event_sink.send(ClientEvent::VoiceChatFailed { message });
                    stream_state.running.store(false, Ordering::Relaxed);
                    break;
                }
            }
            seq = seq.saturating_add(1);
        }
        if last_report.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
            debug_log!(
                "debug event=voice_chat_tx client={} transport=udp packets={} bytes={} capture_drops={} pending_bytes={}",
                client_id,
                sent_packets,
                sent_bytes,
                input_stream.dropped_callbacks.load(Ordering::Relaxed),
                packetizer.pending.len()
            );
            last_report = Instant::now();
        }
    }
    drop(input_stream);
}

fn quality_fps(value: &str) -> u64 {
    match value {
        "low" => 10,
        "high" => 2,
        _ => 5,
    }
}

fn video_control_action(payload: &str) -> Option<String> {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| action.trim().to_ascii_lowercase())
}

fn video_control_value(payload: &str, key: &str) -> Option<String> {
    remote_desktop_value(payload, key)
}

fn video_source_command(source: &VideoSource) -> CommandKind {
    match source {
        VideoSource::RemoteDesktop => CommandKind::RemoteDesktop,
        VideoSource::Camera => CommandKind::Camera,
    }
}

fn stream_sequence_base(generation: u64) -> u64 {
    generation.saturating_mul(1_u64 << 32).max(1)
}

fn queue_message(
    out_tx: &SyncSender<ClientOutbound>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    out_tx
        .send(ClientOutbound {
            session_token: session_token.to_string(),
            message,
        })
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
}

fn try_queue_realtime_message(
    out_tx: &SyncSender<ClientOutbound>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    match out_tx.try_send(ClientOutbound {
        session_token: session_token.to_string(),
        message,
    }) {
        Ok(()) | Err(mpsc::TrySendError::Full(_)) => Ok(()),
        Err(mpsc::TrySendError::Disconnected(_)) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "outbound queue disconnected",
        )),
    }
}

fn queue_file_transfer_reply(
    out_tx: &SyncSender<ClientOutbound>,
    bulk_out_tx: &SyncSender<ClientOutbound>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    if file_transfer_reply_is_bulk(&message) {
        log_client_file_transfer_queue("bulk", &message);
        queue_message(bulk_out_tx, session_token, message)
    } else {
        log_client_file_transfer_queue("high", &message);
        queue_message(out_tx, session_token, message)
    }
}

fn file_transfer_reply_is_bulk(message: &Message) -> bool {
    matches!(
        message,
        Message::FileTransfer {
            direction: FileTransferDirection::Download,
            action: FileTransferAction::Directory
                | FileTransferAction::Chunk
                | FileTransferAction::Complete,
            ..
        }
    )
}

fn log_client_file_transfer_queue(queue: &str, message: &Message) {
    let Message::FileTransfer {
        target_id,
        transfer_id,
        direction,
        action,
        total_bytes,
        transferred_bytes,
        message,
        ..
    } = message
    else {
        return;
    };
    if matches!(
        action,
        FileTransferAction::Directory | FileTransferAction::Chunk
    ) && message.trim().is_empty()
    {
        return;
    }
    debug_log!(
        "debug event=client_file_transfer_queue queue={} client={} id={} direction={} action={} bytes={}/{} message={}",
        queue,
        target_id,
        transfer_id,
        direction.as_str(),
        action.as_str(),
        transferred_bytes,
        total_bytes,
        sanitize_log_value(message)
    );
}

fn sanitize_log_value(value: &str) -> String {
    let mut value = value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    const MAX_LOG_VALUE_LEN: usize = 180;
    if value.len() > MAX_LOG_VALUE_LEN {
        value.truncate(MAX_LOG_VALUE_LEN);
        value.push_str("...");
    }
    value
}

fn send(
    writer: &mut TcpStream,
    next_message_id: &mut u64,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    let result = write_envelope_with_token(
        writer,
        Role::Client,
        *next_message_id,
        None,
        session_token,
        message,
    );
    *next_message_id = next_message_id.saturating_add(1);
    result
}

#[cfg(test)]
mod tests {
    use super::desktop_input_reply_payload;

    #[test]
    fn desktop_input_reply_payload_wraps_errors_as_input_status() {
        let payload = desktop_input_reply_payload(
            "remote_desktop_error\nmessage=macOS input requires Accessibility permission"
                .to_string(),
        );

        assert_eq!(
            payload,
            "remote_desktop_input\nmessage=input failed: macOS input requires Accessibility permission"
        );
    }

    #[test]
    fn desktop_input_reply_payload_keeps_success_payloads() {
        let payload = desktop_input_reply_payload(
            "remote_desktop_input\nmessage=click left 10 20".to_string(),
        );

        assert_eq!(payload, "remote_desktop_input\nmessage=click left 10 20");
    }
}

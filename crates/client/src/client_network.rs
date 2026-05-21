use crate::app_event::{ClientEvent, ClientEventSink, ClientInput};
use crate::audio_stream::{
    audio_stream_loop, audio_udp_receive_loop, new_audio_udp_stream_id, voice_chat_capture_loop,
    AudioUdpEndpoint, AudioUdpSender, AUDIO_STREAM_STOP_SETTLE_MS, AUDIO_UDP_RECV_TIMEOUT_MS,
};
use crate::live_control::realtime_video::latest_video_channel;
use crate::live_video_stream::video_stream_loop;
use crate::outbound::{self, queue_file_transfer_reply, queue_message};
use crate::payload::{
    desktop_input_reply_payload, desktop_payload_is_transient_input, detail_value,
    remote_desktop_action, video_control_action, video_source_command,
};
use crate::reverse_proxy::{client_proxy_stream_loop, ClientProxyStream};
use crate::stream_state::DesktopStreamState;
use crate::{
    commands,
    runtime::{hostname, os_label, username, Config, LocalIdentity},
};
use rdl_protocol::{
    AudioSource, CommandKind, EnvelopeDecoder, FileTransferAction, FileTransferDirection, Message,
    P2pAction, Role, VideoSource,
};
use std::collections::HashMap;
use std::io;
use std::net::{TcpStream, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

const INITIAL_RECONNECT_DELAY_MS: u64 = 500;
const MAX_RECONNECT_DELAY_MS: u64 = 8_000;
const NETWORK_POLL_INTERVAL_MS: u64 = 16;
const NETWORK_IDLE_SLEEP_MS: u64 = 4;
const CLIENT_OUTBOUND_QUEUE_CAPACITY: usize = 32;
const CLIENT_BULK_OUTBOUND_QUEUE_CAPACITY: usize = 2;
pub(crate) fn client_network_loop(
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
    let (video_out_tx, video_out_rx) = latest_video_channel();
    let (bulk_out_tx, bulk_out_rx) = mpsc::sync_channel(CLIENT_BULK_OUTBOUND_QUEUE_CAPACITY);
    thread::spawn(move || outbound::writer_loop(writer, out_rx, video_out_rx, bulk_out_rx));
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
    let desktop_input_lock = Arc::new(Mutex::new(()));
    let mut voice_chat_player: Option<crate::live_control::AudioOutputPlayer> = None;
    let mut voice_chat_invite_udp_endpoint: Option<AudioUdpEndpoint> = None;
    let mut voice_chat_udp_stop: Option<Arc<AtomicBool>> = None;
    let mut proxy_streams = HashMap::<u64, ClientProxyStream>::new();
    let mut p2p_sessions = HashMap::<u64, crate::p2p::P2pTestSession>::new();
    let (proxy_done_tx, proxy_done_rx) = mpsc::channel::<u64>();
    loop {
        while let Ok(stream_id) = proxy_done_rx.try_recv() {
            proxy_streams.remove(&stream_id);
        }

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
                    let mut update = crate::runtime::update_client_config(&config, &payload);
                    if update.restart {
                        match crate::session::schedule_config_file_restart(
                            &update.restart_config_path,
                        ) {
                            Ok(path) => {
                                update.detail.push_str(&format!(
                                    "\nrestart_scheduled=true\nrestart_path={}\nrestart_config_path={}",
                                    detail_value(&path.display().to_string()),
                                    detail_value(&update.restart_config_path.display().to_string())
                                ));
                            }
                            Err(error) => {
                                update.accepted = false;
                                update.restart = false;
                                update.detail.push_str(&format!(
                                    "\nrestart_scheduled=false\nrestart_error={}",
                                    detail_value(&error.to_string())
                                ));
                            }
                        }
                    }
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
                    if update.restart {
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
                if !crate::live_control::command_available(&CommandKind::RemoteDesktop) {
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
                        let _ = queue_message(
                            &out_tx,
                            &session_token,
                            Message::DesktopFrame {
                                client_id: target_id,
                                payload: concat!(
                                    "remote_desktop_error\n",
                                    "message=remote desktop streaming requires video control"
                                )
                                .to_string(),
                            },
                        );
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
                if !crate::live_control::command_available(&CommandKind::RemoteDesktop) {
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
                let input_lock = desktop_input_lock.clone();
                thread::spawn(move || {
                    let _input_guard = match input_lock.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    let should_reply = !desktop_payload_is_transient_input(&payload);
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
                _ if !crate::live_control::video_source_available(&source) => {
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
                    let worker_realtime_tx = video_out_tx.clone();
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
                    _ if !crate::live_control::audio_control_available() => {
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
            Message::ProxyOpen {
                target_id,
                stream_id,
                host,
                port,
            } => {
                if target_id != identity.id {
                    continue;
                }
                if let Some(existing) = proxy_streams.remove(&stream_id) {
                    existing.stop.store(true, Ordering::Relaxed);
                }
                let (data_tx, data_rx) = mpsc::channel::<Vec<u8>>();
                let stop = Arc::new(AtomicBool::new(false));
                proxy_streams.insert(
                    stream_id,
                    ClientProxyStream {
                        data_tx,
                        stop: stop.clone(),
                    },
                );
                let worker_high_tx = out_tx.clone();
                let worker_bulk_tx = bulk_out_tx.clone();
                let worker_token = session_token.clone();
                let worker_done_tx = proxy_done_tx.clone();
                thread::spawn(move || {
                    client_proxy_stream_loop(
                        target_id,
                        stream_id,
                        host,
                        port,
                        data_rx,
                        worker_high_tx,
                        worker_bulk_tx,
                        worker_token,
                        stop,
                        worker_done_tx,
                    );
                });
            }
            Message::ProxyData {
                client_id,
                stream_id,
                bytes,
            } => {
                if client_id != identity.id {
                    continue;
                }
                let Some(stream) = proxy_streams.get(&stream_id) else {
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::ProxyClose {
                            client_id,
                            stream_id,
                            reason: "proxy stream is not open".to_string(),
                        },
                    );
                    continue;
                };
                if stream.data_tx.send(bytes).is_err() {
                    proxy_streams.remove(&stream_id);
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::ProxyClose {
                            client_id,
                            stream_id,
                            reason: "proxy target writer stopped".to_string(),
                        },
                    );
                }
            }
            Message::ProxyClose {
                client_id,
                stream_id,
                reason: _,
            } => {
                if client_id != identity.id {
                    continue;
                }
                if let Some(stream) = proxy_streams.remove(&stream_id) {
                    stream.stop.store(true, Ordering::Relaxed);
                }
            }
            Message::P2pControl {
                target_id,
                session_id,
                nonce,
                action,
                server_udp_addr,
                peer_udp_addr,
                detail,
            } => {
                if target_id != identity.id {
                    continue;
                }
                match action {
                    P2pAction::Start => {
                        if let Some(session) = p2p_sessions.remove(&session_id) {
                            session.stop();
                        }
                        let fallback_server_udp_addr = format!("{}:{}", config.ip, config.port);
                        let session = crate::p2p::start_test(
                            identity.id.clone(),
                            session_id,
                            nonce,
                            server_udp_addr,
                            fallback_server_udp_addr,
                            out_tx.clone(),
                            session_token.clone(),
                            event_sink.clone(),
                        );
                        p2p_sessions.insert(session_id, session);
                    }
                    P2pAction::Stop => {
                        if let Some(session) = p2p_sessions.remove(&session_id) {
                            session.stop();
                        }
                        event_sink.send(ClientEvent::Log(format!(
                            "p2p test stopped by admin session={session_id}"
                        )));
                    }
                    P2pAction::PeerReady => match peer_udp_addr.parse() {
                        Ok(addr) => {
                            if let Some(session) = p2p_sessions.get(&session_id) {
                                session.set_peer_addr(addr);
                            }
                        }
                        Err(error) => event_sink.send(ClientEvent::Log(format!(
                            "p2p peer endpoint invalid session={session_id}: {error}"
                        ))),
                    },
                    P2pAction::Error => {
                        if let Some(session) = p2p_sessions.remove(&session_id) {
                            session.stop();
                        }
                        event_sink.send(ClientEvent::Log(format!(
                            "p2p test error session={session_id}: {detail}"
                        )));
                    }
                    P2pAction::ServerReady => {}
                }
            }
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
    for session in p2p_sessions.values() {
        session.stop();
    }
    if let Some(stop) = voice_chat_udp_stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    let _ = voice_chat_player.take();

    Ok(())
}

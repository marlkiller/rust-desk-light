use crate::{
    command_menu, live_control, remote_management,
    runtime::{hostname, load_admin_identity, os_label, terminal_mode, username, Config},
    user_interaction, windowing,
};
use base64::Engine;
use eframe::egui;
use rdl_protocol::{
    write_envelope_with_token, ClientInfo, CommandKind, CommandOutputStream, EnvelopeDecoder,
    FileTransferAction, FileTransferDirection, Message, Role, VideoSource,
};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, Read};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender, TrySendError},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const INITIAL_RECONNECT_DELAY_MS: u64 = 500;
const MAX_RECONNECT_DELAY_MS: u64 = 8_000;
const NETWORK_POLL_INTERVAL_MS: u64 = 16;
const GUI_FRAME_INTERVAL_MS: u64 = 250;
const NETWORK_IDLE_SLEEP_MS: u64 = 4;
const FILE_TRANSFER_CHUNK_SIZE: usize = 512 * 1024;
const ADMIN_INPUT_QUEUE_CAPACITY: usize = 8;
const MAX_GUI_EVENTS_PER_FRAME: usize = 512;
const PERFORMANCE_AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env();
    if terminal_mode() {
        run_terminal(config)?;
    } else {
        run_gui(config)?;
    }
    Ok(())
}

fn run_gui(config: Config) -> eframe::Result {
    disable_macos_automatic_window_tabbing();

    let (input_tx, input_rx) = mpsc::sync_channel(ADMIN_INPUT_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel();
    let ui_event_tx = event_tx.clone();
    let network_config = config.clone();
    let repaint_handle = Arc::new(Mutex::new(None));
    let network_repaint_handle = repaint_handle.clone();
    let ignored_file_transfers = Arc::new(Mutex::new(HashSet::new()));
    let network_ignored_file_transfers = ignored_file_transfers.clone();

    thread::spawn(move || {
        let event_sink = AdminEventSink::new(event_tx, Some(network_repaint_handle));
        if let Err(error) = admin_network_loop(
            network_config,
            input_rx,
            event_sink.clone(),
            network_ignored_file_transfers,
        ) {
            event_sink.send(AdminEvent::Log(format!("network stopped: {error}")));
        }
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([980.0, 620.0]),
        ..Default::default()
    };
    let window_title = rdl_version::app_version("rust-desk-light admin");

    eframe::run_native(
        &window_title,
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(AdminApp::new(
                cc,
                config,
                input_tx,
                event_rx,
                ui_event_tx,
                repaint_handle,
                ignored_file_transfers,
            )))
        }),
    )
}

#[cfg(target_os = "macos")]
fn disable_macos_automatic_window_tabbing() {
    if let Some(main_thread) = objc2_foundation::MainThreadMarker::new() {
        objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, main_thread);
    }
}

#[cfg(not(target_os = "macos"))]
fn disable_macos_automatic_window_tabbing() {}

fn run_terminal(config: Config) -> io::Result<()> {
    println!(
        "rust-desk-light admin {} terminal mode, server={}:{}",
        rdl_version::display_version(),
        config.ip,
        config.port
    );

    let (input_tx, input_rx) = mpsc::sync_channel(ADMIN_INPUT_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel();
    let ignored_file_transfers = Arc::new(Mutex::new(HashSet::new()));
    thread::spawn(move || {
        let event_sink = AdminEventSink::new(event_tx, None);
        if let Err(error) =
            admin_network_loop(config, input_rx, event_sink.clone(), ignored_file_transfers)
        {
            event_sink.send(AdminEvent::Log(format!("network stopped: {error}")));
        }
    });
    thread::spawn(move || terminal_input_loop(input_tx));

    for event in event_rx {
        match event {
            AdminEvent::Clients(clients) => {
                println!("online clients: {}", clients.len());
                for client in clients {
                    println!(
                        "- {} | fp={} | host={} os={} user={} gui={}",
                        client.id,
                        client.fingerprint,
                        client.hostname,
                        client.os,
                        client.username,
                        client.gui_available
                    );
                }
            }
            AdminEvent::Ack {
                client_id,
                command,
                accepted,
                detail,
            } => println!(
                "ack client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                detail
            ),
            AdminEvent::CommandOutput {
                client_id,
                command,
                stream,
                chunk,
                finished,
                success,
                ..
            } => println!(
                "command_output client={} command={} stream={} finished={} success={} chunk={}",
                client_id,
                command.as_str(),
                stream.as_str(),
                finished,
                success,
                chunk
            ),
            AdminEvent::DesktopFrame { client_id, payload } => {
                println!("desktop_frame client={} bytes={}", client_id, payload.len());
            }
            AdminEvent::DecodedDesktopFrame { client_id, result } => match result {
                Ok(_) => println!("decoded_desktop_frame client={client_id}"),
                Err(error) => println!("decoded_desktop_frame client={client_id} error={error}"),
            },
            AdminEvent::DecodedCameraFrame { client_id, result } => match result {
                Ok(_) => println!("decoded_camera_frame client={client_id}"),
                Err(error) => println!("decoded_camera_frame client={client_id} error={error}"),
            },
            AdminEvent::VideoFrame {
                client_id,
                source,
                bytes,
                ..
            } => {
                println!(
                    "video_frame client={} source={} bytes={}",
                    client_id,
                    source.as_str(),
                    bytes.len()
                );
            }
            AdminEvent::FileTransfer(message) => {
                if let Message::FileTransfer {
                    target_id,
                    transfer_id,
                    direction,
                    action,
                    total_bytes,
                    transferred_bytes,
                    message,
                    ..
                } = message
                {
                    println!(
                        "file_transfer client={} id={} direction={} action={} bytes={}/{} message={}",
                        target_id,
                        transfer_id,
                        direction.as_str(),
                        action.as_str(),
                        transferred_bytes,
                        total_bytes,
                        message
                    );
                }
            }
            AdminEvent::Log(line) => println!("{line}"),
            AdminEvent::Connected => println!("connected"),
            AdminEvent::Disconnected => println!("disconnected"),
        }
    }

    Ok(())
}

fn admin_network_loop(
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
        while let Ok(input) = input_rx.try_recv() {
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

        let Some(message) = (match decoder.read_next(&mut stream) {
            Ok(Some(envelope)) => Some(envelope.message),
            Ok(None) => {
                thread::sleep(Duration::from_millis(NETWORK_IDLE_SLEEP_MS));
                continue;
            }
            Err(error) => {
                event_sink.send(AdminEvent::Log(format!("network read failed: {error}")));
                return Ok(AdminConnectionExit::Disconnected);
            }
        }) else {
            continue;
        };

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

fn should_log_admin_file_transfer_event(action: FileTransferAction, message: &str) -> bool {
    matches!(
        action,
        FileTransferAction::Start
            | FileTransferAction::Cancel
            | FileTransferAction::Complete
            | FileTransferAction::Error
    ) || !message.trim().is_empty()
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

struct AdminApp {
    config: Config,
    input_tx: SyncSender<AdminInput>,
    event_rx: Receiver<AdminEvent>,
    event_tx: Sender<AdminEvent>,
    repaint_handle: Arc<Mutex<Option<egui::Context>>>,
    connected: bool,
    clients: Vec<ClientRow>,
    client_filter: String,
    selected_client_id: Option<String>,
    command_windows: Vec<CommandResultWindow>,
    file_manager_windows: Vec<remote_management::file_manager::FileManagerWindow>,
    desktop_windows: Vec<live_control::remote_desktop::RemoteDesktopWindow>,
    camera_windows: Vec<live_control::camera::CameraWindow>,
    terminal_windows: Vec<remote_management::remote_terminal::TerminalWindow>,
    chat_windows: Vec<user_interaction::text_chat::ChatWindow>,
    interaction_command_windows: Vec<user_interaction::commands::InteractionCommandWindow>,
    session_command_windows: Vec<crate::session::SessionCommandWindow>,
    execute_windows: Vec<crate::execute::ExecuteWindow>,
    file_transfer_cancel_flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
    log_lines: Vec<String>,
}

struct CommandResultWindow {
    id: u64,
    client_id: String,
    hostname: String,
    username: String,
    command: CommandKind,
    status: CommandResultStatus,
    detail: String,
    open: bool,
    close_requested: Arc<AtomicBool>,
    refresh_requested: Arc<AtomicBool>,
    auto_refresh_enabled: Arc<AtomicBool>,
    last_auto_refresh_at: Option<Instant>,
    process_kill_requested: Arc<Mutex<Option<String>>>,
    table_filter: Arc<Mutex<String>>,
    table_sort: Arc<Mutex<Option<TableSort>>>,
    table_selected_row: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Copy)]
enum CommandResultStatus {
    Pending,
    Accepted,
    Failed,
}

#[derive(Clone, Copy)]
struct TableSort {
    column: usize,
    ascending: bool,
}

#[derive(Clone)]
struct ClientRow {
    info: ClientInfo,
    status: ClientStatus,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ClientStatus {
    Online,
    Stale,
    Offline,
}

impl AdminApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        input_tx: SyncSender<AdminInput>,
        event_rx: Receiver<AdminEvent>,
        event_tx: Sender<AdminEvent>,
        repaint_handle: Arc<Mutex<Option<egui::Context>>>,
        ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
    ) -> Self {
        apply_admin_theme(&cc.egui_ctx);
        if let Ok(mut handle) = repaint_handle.lock() {
            *handle = Some(cc.egui_ctx.clone());
        }
        Self {
            config,
            input_tx,
            event_rx,
            event_tx,
            repaint_handle,
            connected: false,
            clients: Vec::new(),
            client_filter: String::new(),
            selected_client_id: None,
            command_windows: Vec::new(),
            file_manager_windows: Vec::new(),
            desktop_windows: Vec::new(),
            camera_windows: Vec::new(),
            terminal_windows: Vec::new(),
            chat_windows: Vec::new(),
            interaction_command_windows: Vec::new(),
            session_command_windows: Vec::new(),
            execute_windows: Vec::new(),
            file_transfer_cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            ignored_file_transfers,
            log_lines: vec![timestamped_log(format!(
                "admin gui started version={}",
                rdl_version::display_version()
            ))],
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut changed = false;
        let mut latest_desktop_frames = HashMap::<String, String>::new();
        let mut latest_camera_frames = HashMap::<String, String>::new();
        let mut processed_events = 0usize;
        while processed_events < MAX_GUI_EVENTS_PER_FRAME {
            let Ok(event) = self.event_rx.try_recv() else {
                break;
            };
            processed_events += 1;
            changed = true;
            match event {
                AdminEvent::Connected => {
                    self.connected = true;
                    self.push_log("connected to server");
                }
                AdminEvent::Disconnected => {
                    self.connected = false;
                    self.push_log("disconnected from server");
                    for client in &mut self.clients {
                        client.status = ClientStatus::Offline;
                    }
                }
                AdminEvent::Clients(clients) => {
                    self.merge_clients(clients);
                    if self.selected_client_id.is_none() {
                        self.selected_client_id =
                            self.clients.first().map(|client| client.info.id.clone());
                    }
                }
                AdminEvent::Ack {
                    client_id,
                    command,
                    accepted,
                    detail,
                } => {
                    if accepted
                        && command == CommandKind::Camera
                        && detail.starts_with("camera_frame\n")
                    {
                        latest_camera_frames.insert(client_id, detail);
                    } else {
                        self.handle_command_ack(client_id, command, accepted, detail);
                    }
                }
                AdminEvent::DesktopFrame { client_id, payload } => {
                    latest_desktop_frames.insert(client_id, payload);
                }
                AdminEvent::DecodedDesktopFrame { client_id, result } => match result {
                    Ok(frame) => live_control::remote_desktop::handle_decoded_frame(
                        &mut self.desktop_windows,
                        &client_id,
                        frame,
                    ),
                    Err(message) => self.handle_desktop_ack(
                        &client_id,
                        true,
                        format!("remote_desktop_error\nmessage={message}"),
                    ),
                },
                AdminEvent::DecodedCameraFrame { client_id, result } => match result {
                    Ok(frame) => live_control::camera::handle_decoded_frame(
                        &mut self.camera_windows,
                        &client_id,
                        frame,
                    ),
                    Err(message) => self.handle_camera_ack(
                        &client_id,
                        true,
                        format!("camera_error\nmessage={message}"),
                    ),
                },
                AdminEvent::VideoFrame {
                    client_id,
                    source,
                    seq,
                    source_width,
                    source_height,
                    image_width,
                    image_height,
                    format,
                    bytes,
                } => self.spawn_video_frame_decode(
                    client_id,
                    source,
                    seq,
                    source_width,
                    source_height,
                    image_width,
                    image_height,
                    format,
                    bytes,
                ),
                AdminEvent::CommandOutput {
                    client_id,
                    command,
                    stream_id,
                    sequence,
                    stream,
                    chunk,
                    current_dir,
                    finished,
                    success,
                } => self.handle_command_output(
                    client_id,
                    command,
                    stream_id,
                    sequence,
                    stream,
                    chunk,
                    current_dir,
                    finished,
                    success,
                ),
                AdminEvent::FileTransfer(message) => {
                    if let Message::FileTransfer {
                        target_id,
                        transfer_id,
                        direction,
                        action,
                        total_bytes,
                        transferred_bytes,
                        message: status_message,
                        ..
                    } = &message
                    {
                        if should_log_admin_file_transfer_event(*action, status_message) {
                            eprintln!(
                                "debug event=admin_file_transfer_recv client={} id={} direction={} action={} bytes={}/{} message={}",
                                target_id,
                                transfer_id,
                                direction.as_str(),
                                action.as_str(),
                                transferred_bytes,
                                total_bytes,
                                sanitize_log_value(status_message)
                            );
                        }
                        if *action == FileTransferAction::Error {
                            if let Ok(flags) = self.file_transfer_cancel_flags.lock() {
                                if let Some(flag) = flags.get(transfer_id) {
                                    flag.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                        if self.should_ignore_file_transfer(target_id, *transfer_id) {
                            if matches!(
                                *action,
                                FileTransferAction::Complete | FileTransferAction::Error
                            ) {
                                self.unignore_file_transfer(target_id, *transfer_id);
                            }
                            continue;
                        }
                        let client_id = target_id.clone();
                        let (hostname, username) = self.client_window_identity(&client_id);
                        remote_management::file_manager::handle_transfer(
                            &mut self.file_manager_windows,
                            &client_id,
                            hostname,
                            username,
                            message,
                        );
                    }
                }
                AdminEvent::Log(line) => self.push_log(line),
            }
        }
        for (client_id, payload) in latest_desktop_frames {
            if payload.starts_with("remote_desktop_frame\n") {
                self.spawn_desktop_frame_decode(client_id, payload);
            } else {
                self.handle_desktop_ack(&client_id, true, payload);
            }
        }
        for (client_id, payload) in latest_camera_frames {
            self.spawn_camera_frame_decode(client_id, payload);
        }
        changed
    }

    fn ignore_file_transfer(&self, client_id: &str, transfer_id: u64) {
        if let Ok(mut ignored) = self.ignored_file_transfers.lock() {
            ignored.insert((client_id.to_string(), transfer_id));
        }
        eprintln!("debug event=admin_file_transfer_ignore_add client={client_id} id={transfer_id}");
    }

    fn unignore_file_transfer(&self, client_id: &str, transfer_id: u64) {
        if let Ok(mut ignored) = self.ignored_file_transfers.lock() {
            ignored.remove(&(client_id.to_string(), transfer_id));
        }
        eprintln!(
            "debug event=admin_file_transfer_ignore_remove client={client_id} id={transfer_id}"
        );
    }

    fn should_ignore_file_transfer(&self, client_id: &str, transfer_id: u64) -> bool {
        self.ignored_file_transfers
            .lock()
            .map(|ignored| ignored.contains(&(client_id.to_string(), transfer_id)))
            .unwrap_or(false)
    }

    fn spawn_desktop_frame_decode(&self, client_id: String, payload: String) {
        let sink = AdminEventSink::new(self.event_tx.clone(), Some(self.repaint_handle.clone()));
        thread::spawn(move || {
            let result = live_control::remote_desktop::decode_frame_payload(&payload);
            sink.send(AdminEvent::DecodedDesktopFrame { client_id, result });
        });
    }

    fn spawn_camera_frame_decode(&self, client_id: String, payload: String) {
        let sink = AdminEventSink::new(self.event_tx.clone(), Some(self.repaint_handle.clone()));
        thread::spawn(move || {
            let result = live_control::camera::decode_frame_payload(&payload);
            sink.send(AdminEvent::DecodedCameraFrame { client_id, result });
        });
    }

    fn spawn_video_frame_decode(
        &self,
        client_id: String,
        source: VideoSource,
        seq: u64,
        source_width: u32,
        source_height: u32,
        image_width: u32,
        image_height: u32,
        format: String,
        bytes: Vec<u8>,
    ) {
        let sink = AdminEventSink::new(self.event_tx.clone(), Some(self.repaint_handle.clone()));
        thread::spawn(move || match source {
            VideoSource::RemoteDesktop => {
                let result = live_control::remote_desktop::decode_video_frame(
                    seq,
                    source_width,
                    source_height,
                    image_width,
                    image_height,
                    format,
                    bytes,
                );
                sink.send(AdminEvent::DecodedDesktopFrame { client_id, result });
            }
            VideoSource::Camera => {
                let result = live_control::camera::decode_video_frame(
                    seq,
                    image_width,
                    image_height,
                    format,
                    bytes,
                );
                sink.send(AdminEvent::DecodedCameraFrame { client_id, result });
            }
        });
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let line = timestamped_log(line);
        eprintln!("{line}");
        self.log_lines.push(line);
        prune_activity_logs(&mut self.log_lines);
    }

    fn merge_clients(&mut self, clients: Vec<ClientInfo>) {
        let online_ids: HashSet<String> = clients.iter().map(|client| client.id.clone()).collect();
        for client in clients {
            if let Some(existing) = self.clients.iter_mut().find(|row| row.info.id == client.id) {
                existing.info = client;
                existing.status = ClientStatus::Online;
            } else {
                self.clients.push(ClientRow {
                    info: client,
                    status: ClientStatus::Online,
                });
            }
        }

        for row in &mut self.clients {
            if !online_ids.contains(&row.info.id) && row.status != ClientStatus::Stale {
                row.status = ClientStatus::Offline;
            }
        }
    }

    fn filtered_clients(&self) -> Vec<ClientRow> {
        let filter = self.client_filter.trim().to_ascii_lowercase();
        self.clients
            .iter()
            .filter(|row| {
                if filter.is_empty() {
                    return true;
                }
                row.info.id.to_ascii_lowercase().contains(&filter)
                    || row.info.fingerprint.to_ascii_lowercase().contains(&filter)
                    || row.info.hostname.to_ascii_lowercase().contains(&filter)
                    || row.info.username.to_ascii_lowercase().contains(&filter)
                    || row.info.os.to_ascii_lowercase().contains(&filter)
            })
            .cloned()
            .collect()
    }

    fn online_client_count(&self) -> usize {
        self.clients
            .iter()
            .filter(|row| row.status == ClientStatus::Online)
            .count()
    }

    fn send_command(&mut self, client_id: &str, command: CommandKind) {
        if session_command_requires_confirmation(&command) {
            self.open_session_command_window(client_id, command);
            return;
        }
        if command == CommandKind::TextChat {
            self.open_chat_window(client_id);
            return;
        }
        if command == CommandKind::FileManager {
            self.open_file_manager_window(client_id);
            return;
        }
        if command == CommandKind::RemoteTerminal {
            self.open_terminal_window(client_id);
            return;
        }
        if command == CommandKind::RemoteDesktop {
            self.open_desktop_window(client_id);
            return;
        }
        if command == CommandKind::Camera {
            self.open_camera_window(client_id);
            return;
        }
        if matches!(
            command,
            CommandKind::ExecuteFile | CommandKind::ExecuteCode | CommandKind::ExecuteStaticCommand
        ) {
            self.open_execute_window(client_id, command);
            return;
        }
        if matches!(
            command,
            CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::OpenTextInNotepad
        ) {
            self.open_interaction_command_window(client_id, command);
            return;
        }
        let _ = self.input_tx.send(AdminInput::Command {
            target_id: client_id.to_string(),
            command: command.clone(),
            payload: String::new(),
        });
        self.open_command_window(client_id, command.clone());
        self.push_log(format!(
            "sent command={} to {}",
            command.as_str(),
            client_id
        ));
    }

    fn open_chat_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::text_chat::open_window(
            &mut self.chat_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_file_manager_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        remote_management::file_manager::open_window(
            &mut self.file_manager_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_terminal_window(&mut self, client_id: &str) {
        let (hostname, username, os) = self.client_window_environment(client_id);
        remote_management::remote_terminal::open_window(
            &mut self.terminal_windows,
            client_id,
            hostname,
            username,
            os,
        );
    }

    fn open_desktop_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::remote_desktop::open_window(
            &mut self.desktop_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_camera_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::camera::open_window(&mut self.camera_windows, client_id, hostname, username);
    }

    fn open_session_command_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        crate::session::open_window(
            &mut self.session_command_windows,
            client_id,
            hostname,
            username,
            command,
        );
    }

    fn open_interaction_command_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::commands::open_window(
            &mut self.interaction_command_windows,
            client_id,
            hostname,
            username,
            command,
        );
    }

    fn open_execute_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        crate::execute::open_window(
            &mut self.execute_windows,
            client_id,
            hostname,
            username,
            command,
        );
    }

    fn open_command_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        self.command_windows.push(CommandResultWindow {
            id: self.next_command_window_id(),
            client_id: client_id.to_string(),
            hostname,
            username,
            command,
            status: CommandResultStatus::Pending,
            detail: "Waiting for client result...".to_string(),
            open: true,
            close_requested: Arc::new(AtomicBool::new(false)),
            refresh_requested: Arc::new(AtomicBool::new(false)),
            auto_refresh_enabled: Arc::new(AtomicBool::new(false)),
            last_auto_refresh_at: None,
            process_kill_requested: Arc::new(Mutex::new(None)),
            table_filter: Arc::new(Mutex::new(String::new())),
            table_sort: Arc::new(Mutex::new(None)),
            table_selected_row: Arc::new(Mutex::new(None)),
        });
    }

    fn next_command_window_id(&self) -> u64 {
        self.command_windows
            .iter()
            .map(|window| window.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    fn client_window_identity(&self, client_id: &str) -> (String, String) {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| (row.info.hostname.clone(), row.info.username.clone()))
            .unwrap_or_else(|| ("unknown-host".to_string(), "unknown-user".to_string()))
    }

    fn client_window_environment(&self, client_id: &str) -> (String, String, String) {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| {
                (
                    row.info.hostname.clone(),
                    row.info.username.clone(),
                    row.info.os.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    "unknown-host".to_string(),
                    "unknown-user".to_string(),
                    "unknown-os".to_string(),
                )
            })
    }

    fn handle_command_ack(
        &mut self,
        client_id: String,
        command: CommandKind,
        accepted: bool,
        detail: String,
    ) {
        self.push_log(format!(
            "ack client={} command={} accepted={}",
            client_id,
            command.as_str(),
            accepted
        ));

        if accepted && detail == "forwarded" {
            return;
        }

        if command == CommandKind::TextChat {
            self.handle_chat_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::FileManager {
            self.handle_file_manager_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::RemoteTerminal {
            self.handle_terminal_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::RemoteDesktop {
            self.handle_desktop_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::Camera {
            self.handle_camera_ack(&client_id, accepted, detail);
            return;
        }
        if crate::execute::handle_ack(
            &mut self.execute_windows,
            &client_id,
            &command,
            accepted,
            &detail,
        ) {
            return;
        }
        if session_command_requires_confirmation(&command) {
            self.push_log(format!(
                "result client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                sanitize_log_value(&detail)
            ));
            if accepted
                && command == CommandKind::DeleteClient
                && detail_status(&detail).as_deref() == Some("scheduled")
            {
                self.remove_client_row(&client_id);
            }
            return;
        }
        if quiet_user_interaction_command(&command) {
            self.push_log(format!(
                "result client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                sanitize_log_value(&detail)
            ));
            return;
        }

        let (hostname, username) = self.client_window_identity(&client_id);
        if let Some(window) = self.command_windows.iter_mut().rev().find(|window| {
            window.client_id == client_id
                && window.command == command
                && matches!(window.status, CommandResultStatus::Pending)
        }) {
            if window.command == CommandKind::PerformanceMonitor
                && window.close_requested.load(Ordering::Relaxed)
            {
                window.status = if accepted {
                    CommandResultStatus::Accepted
                } else {
                    CommandResultStatus::Failed
                };
                window.detail = detail;
                window.hostname = hostname;
                window.username = username;
                window.open = false;
                window.auto_refresh_enabled.store(false, Ordering::Relaxed);
                window.last_auto_refresh_at = None;
                return;
            }
            update_command_window(window, accepted, detail, hostname, username);
            return;
        }

        if let Some(window) = self
            .command_windows
            .iter_mut()
            .rev()
            .find(|window| window.client_id == client_id && window.command == command)
        {
            update_command_window(window, accepted, detail, hostname, username);
            return;
        }

        if command == CommandKind::KillTargetProcess {
            self.push_log(format!(
                "kill target process result client={} accepted={} detail={}",
                client_id, accepted, detail
            ));
            if accepted && kill_target_process_succeeded(&detail) {
                self.refresh_process_window(&client_id);
                self.refresh_window_manager_window(&client_id);
            }
            return;
        }

        self.command_windows.push(CommandResultWindow {
            id: self.next_command_window_id(),
            client_id,
            hostname,
            username,
            command,
            status: if accepted {
                CommandResultStatus::Accepted
            } else {
                CommandResultStatus::Failed
            },
            detail,
            open: true,
            close_requested: Arc::new(AtomicBool::new(false)),
            refresh_requested: Arc::new(AtomicBool::new(false)),
            auto_refresh_enabled: Arc::new(AtomicBool::new(false)),
            last_auto_refresh_at: None,
            process_kill_requested: Arc::new(Mutex::new(None)),
            table_filter: Arc::new(Mutex::new(String::new())),
            table_sort: Arc::new(Mutex::new(None)),
            table_selected_row: Arc::new(Mutex::new(None)),
        });
    }

    fn remove_client_row(&mut self, client_id: &str) {
        self.clients.retain(|row| row.info.id != client_id);
        if self.selected_client_id.as_deref() == Some(client_id) {
            self.selected_client_id = self.clients.first().map(|row| row.info.id.clone());
        }
    }

    fn handle_chat_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::text_chat::handle_ack(
            &mut self.chat_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn handle_file_manager_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        remote_management::file_manager::handle_ack(
            &mut self.file_manager_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn handle_terminal_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username, os) = self.client_window_environment(client_id);
        remote_management::remote_terminal::handle_ack(
            &mut self.terminal_windows,
            client_id,
            hostname,
            username,
            os,
            accepted,
            detail,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_command_output(
        &mut self,
        client_id: String,
        command: CommandKind,
        stream_id: u64,
        sequence: u64,
        stream: CommandOutputStream,
        chunk: String,
        current_dir: String,
        finished: bool,
        success: bool,
    ) {
        if command != CommandKind::RemoteTerminal {
            self.push_log(format!(
                "ignored command output client={} command={}",
                client_id,
                command.as_str()
            ));
            return;
        }
        let (hostname, username, os) = self.client_window_environment(&client_id);
        remote_management::remote_terminal::handle_output(
            &mut self.terminal_windows,
            &client_id,
            hostname,
            username,
            os,
            stream_id,
            sequence,
            stream,
            chunk,
            current_dir,
            finished,
            success,
        );
    }

    fn handle_desktop_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::remote_desktop::handle_ack(
            &mut self.desktop_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn handle_camera_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::camera::handle_ack(
            &mut self.camera_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn refresh_process_window(&mut self, client_id: &str) {
        self.refresh_command_result_window(
            client_id,
            CommandKind::ProcessManager,
            "Refreshing process list...",
        );
    }

    fn refresh_window_manager_window(&mut self, client_id: &str) {
        self.refresh_command_result_window(
            client_id,
            CommandKind::WindowManager,
            "Refreshing window list...",
        );
    }

    fn refresh_command_result_window(
        &mut self,
        client_id: &str,
        command: CommandKind,
        pending_detail: &str,
    ) {
        let Some(window) = self
            .command_windows
            .iter_mut()
            .rev()
            .find(|window| window.client_id == client_id && window.command == command)
        else {
            return;
        };

        let _ = self.input_tx.send(AdminInput::Command {
            target_id: client_id.to_string(),
            command: command.clone(),
            payload: String::new(),
        });
        window.status = CommandResultStatus::Pending;
        window.detail = pending_detail.to_string();
        window.open = true;
        self.push_log(format!(
            "refresh command={} to {client_id}",
            command.as_str()
        ));
    }

    fn render_menu_bar(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            ui.horizontal(|ui| {
                section_title(ui, "Commands");
                ui.separator();
                if let Some(client_id) = self.selected_client_id.clone() {
                    command_menu::render_context_menu(ui, &client_id, &mut |client_id, command| {
                        self.send_command(client_id, command);
                    });
                } else {
                    ui.label(
                        egui::RichText::new("Select a client to enable command menus")
                            .color(COLOR_MUTED),
                    );
                }
                ui.menu_button("中文测试", |ui| {
                    if ui.button("输出中文日志").clicked() {
                        self.push_log("中文日志测试：菜单和日志应正常显示，不应乱码。");
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    connection_status_pill(ui, self.connected);
                    ui.label(
                        egui::RichText::new(format!("{}:{}", self.config.ip, self.config.port))
                            .color(COLOR_MUTED),
                    );
                });
            });
        });
    }

    fn render_overview(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Overview");
            ui.add_space(8.0);
            ui.columns(4, |columns| {
                metric(
                    &mut columns[0],
                    "Online clients",
                    self.online_client_count().to_string(),
                );
                metric(
                    &mut columns[1],
                    "Known clients",
                    self.clients.len().to_string(),
                );
                metric(
                    &mut columns[2],
                    "Selected",
                    self.selected_client_id
                        .as_deref()
                        .unwrap_or("None")
                        .to_string(),
                );
                metric(&mut columns[3], "Version", rdl_version::display_version());
            });
        });
    }

    fn render_clients(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            ui.horizontal(|ui| {
                section_title(ui, "Clients");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("Right click a row for commands")
                            .size(12.0)
                            .color(COLOR_MUTED),
                    );
                });
            });
            ui.add_space(8.0);
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
                ui.add_sized(
                    [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
                    egui::TextEdit::singleline(&mut self.client_filter)
                        .hint_text("Search by id, fingerprint, host, user, or OS")
                        .vertical_align(egui::Align::Center),
                );
            });
            ui.add_space(10.0);

            let clients = self.filtered_clients();
            if clients.is_empty() {
                empty_state(ui);
                return;
            }

            let ctx = ui.ctx().clone();
            egui::ScrollArea::horizontal()
                .id_salt("admin_clients_horizontal_scroll")
                .show(ui, |ui| {
                    egui_extras::TableBuilder::new(ui)
                        .id_salt("admin_clients_table")
                        .striped(true)
                        .sense(egui::Sense::click())
                        .column(egui_extras::Column::exact(82.0))
                        .column(egui_extras::Column::exact(180.0))
                        .column(egui_extras::Column::exact(150.0))
                        .column(egui_extras::Column::exact(170.0))
                        .column(egui_extras::Column::exact(150.0))
                        .column(egui_extras::Column::exact(120.0))
                        .column(egui_extras::Column::exact(220.0))
                        .column(egui_extras::Column::exact(70.0))
                        .column(egui_extras::Column::exact(130.0))
                        .header(24.0, |mut header| {
                            header.col(|ui| table_header(ui, "Status"));
                            header.col(|ui| table_header(ui, "Client ID"));
                            header.col(|ui| table_header(ui, "IP"));
                            header.col(|ui| table_header(ui, "Fingerprint"));
                            header.col(|ui| table_header(ui, "Host"));
                            header.col(|ui| table_header(ui, "User"));
                            header.col(|ui| table_header(ui, "OS Version"));
                            header.col(|ui| table_header(ui, "GUI"));
                            header.col(|ui| table_header(ui, "Last Heartbeat"));
                        })
                        .body(|body| {
                            body.rows(28.0, clients.len(), |mut row| {
                                let row_data = &clients[row.index()];
                                let client = &row_data.info;
                                let selected =
                                    self.selected_client_id.as_deref() == Some(client.id.as_str());
                                row.set_selected(selected);
                                row.col(|ui| {
                                    centered_cell(ui, |ui| client_status_text(ui, row_data.status))
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(ui, compact_id(&client.id));
                                    });
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(ui, &client.peer_addr);
                                    });
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(ui, compact_id(&client.fingerprint));
                                    });
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(ui, &client.hostname);
                                    });
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(ui, &client.username);
                                    });
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(ui, &client.os);
                                    });
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(
                                            ui,
                                            if client.gui_available { "Yes" } else { "No" },
                                        );
                                    });
                                });
                                row.col(|ui| {
                                    centered_cell(ui, |ui| {
                                        cell_label(ui, last_seen_label(client.last_seen_epoch_ms));
                                    });
                                });
                                let response = row.response();
                                if response.hovered() {
                                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if response.clicked() {
                                    self.selected_client_id = Some(client.id.clone());
                                }
                                response.context_menu(|ui| {
                                    command_menu::render_context_menu(
                                        ui,
                                        &client.id,
                                        &mut |client_id, command| {
                                            self.send_command(client_id, command);
                                        },
                                    );
                                });
                            });
                        });
                });
        });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Activity");
            ui.add_space(8.0);
            let output = egui::ScrollArea::vertical()
                .id_salt("admin_activity_scroll_area")
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

    fn render_command_windows(&mut self, ctx: &egui::Context) {
        let mut pending_logs = Vec::new();
        let now = Instant::now();
        for window in &mut self.command_windows {
            if window.close_requested.load(Ordering::Relaxed) {
                window.open = false;
                window.auto_refresh_enabled.store(false, Ordering::Relaxed);
                window.last_auto_refresh_at = None;
            }
            if window.refresh_requested.swap(false, Ordering::Relaxed) {
                refresh_command_window(
                    &self.input_tx,
                    window,
                    "Refreshing command result...",
                    "refresh",
                    now,
                    &mut pending_logs,
                );
            }
            if performance_auto_refresh_due(window, now) {
                refresh_command_window(
                    &self.input_tx,
                    window,
                    "Auto refreshing performance monitor...",
                    "auto_refresh",
                    now,
                    &mut pending_logs,
                );
            }
            let process_id = window
                .process_kill_requested
                .lock()
                .ok()
                .and_then(|mut value| value.take());
            if let Some(process_id) = process_id {
                let _ = self.input_tx.send(AdminInput::Command {
                    target_id: window.client_id.clone(),
                    command: CommandKind::KillTargetProcess,
                    payload: process_id.clone(),
                });
                pending_logs.push(format!(
                    "kill target process pid={} on {}",
                    process_id, window.client_id
                ));
            }
        }
        for line in pending_logs {
            self.push_log(line);
        }

        for window in &mut self.command_windows {
            if !window.open {
                continue;
            }
            let title = format!(
                "{} - {}",
                command_title(&window.command),
                command_window_identity_title(&window.hostname, &window.username)
            );
            let viewport_id = egui::ViewportId::from_hash_of(("command_result", window.id));
            let builder = windowing::child_viewport_builder(title, [760.0, 460.0], [260.0, 180.0]);

            let command = window.command.clone();
            let status = window.status;
            let detail = window.detail.clone();
            let close_requested = window.close_requested.clone();
            let refresh_requested = window.refresh_requested.clone();
            let auto_refresh_enabled = window.auto_refresh_enabled.clone();
            let process_kill_requested = window.process_kill_requested.clone();
            let table_filter = window.table_filter.clone();
            let table_sort = window.table_sort.clone();
            let table_selected_row = window.table_selected_row.clone();
            let status_notice = command_status_notice(&command, status, &detail);

            ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
                if ui.ctx().input(|input| input.viewport().close_requested()) {
                    close_requested.store(true, Ordering::Relaxed);
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
                    .show_inside(ui, |ui| {
                        windowing::render_child_window_controls(ui);
                        let status_bar_height = 44.0;
                        let content_height =
                            (ui.available_height() - status_bar_height - 8.0).max(0.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), content_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::ScrollArea::both()
                                    .id_salt(("command_result_scroll", viewport_id))
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let mut detail = detail.clone();
                                        render_command_result(
                                            ui,
                                            &command,
                                            &mut detail,
                                            &table_filter,
                                            &table_sort,
                                            &table_selected_row,
                                            &refresh_requested,
                                            &auto_refresh_enabled,
                                            matches!(status, CommandResultStatus::Pending),
                                            &process_kill_requested,
                                        );
                                    });
                            },
                        );
                        ui.add_space(8.0);
                        render_command_window_status_bar(ui, &status, status_notice.as_deref());
                    });
            });
        }
        self.command_windows
            .retain(|window| window.open || matches!(window.status, CommandResultStatus::Pending));
    }

    fn render_chat_windows(&mut self, ctx: &egui::Context) {
        for outbound in user_interaction::text_chat::render_windows(ctx, &mut self.chat_windows) {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: CommandKind::TextChat,
                payload: outbound.text,
            });
            self.push_log(format!("sent text_chat to {}", outbound.client_id));
        }
    }

    fn render_interaction_command_windows(&mut self, ctx: &egui::Context) {
        for outbound in
            user_interaction::commands::render_windows(ctx, &mut self.interaction_command_windows)
        {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: outbound.command.clone(),
                payload: outbound.payload,
            });
            self.push_log(format!(
                "sent command={} to {}",
                outbound.command.as_str(),
                outbound.client_id
            ));
        }
    }

    fn render_session_command_windows(&mut self, ctx: &egui::Context) {
        for outbound in crate::session::render_windows(ctx, &mut self.session_command_windows) {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: outbound.command.clone(),
                payload: outbound.payload,
            });
            self.push_log(format!(
                "sent command={} to {}",
                outbound.command.as_str(),
                outbound.client_id
            ));
        }
    }

    fn render_execute_windows(&mut self, ctx: &egui::Context) {
        for outbound in crate::execute::render_windows(ctx, &mut self.execute_windows) {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: outbound.command.clone(),
                payload: outbound.payload,
            });
            self.push_log(format!(
                "sent command={} to {}",
                outbound.command.as_str(),
                outbound.client_id
            ));
        }
    }

    fn render_file_manager_windows(&mut self, ctx: &egui::Context) {
        for outbound in
            remote_management::file_manager::render_windows(ctx, &mut self.file_manager_windows)
        {
            if let Some(request) =
                remote_management::file_manager::parse_transfer_request(&outbound.payload)
            {
                self.handle_file_transfer_request(outbound.client_id.clone(), request);
            } else {
                let input_tx = self.input_tx.clone();
                let client_id = outbound.client_id.clone();
                let action = payload_field(&outbound.payload, "action")
                    .unwrap_or_else(|| "list".to_string());
                let path = payload_field(&outbound.payload, "path").unwrap_or_default();
                eprintln!(
                    "debug event=admin_file_manager_send client={} action={} path={}",
                    outbound.client_id, action, path
                );
                thread::spawn(move || {
                    let _ = input_tx.send(AdminInput::Command {
                        target_id: client_id,
                        command: CommandKind::FileManager,
                        payload: outbound.payload,
                    });
                });
                self.push_log(format!("sent file_manager to {}", outbound.client_id));
            }
        }
    }

    fn handle_file_transfer_request(
        &mut self,
        client_id: String,
        request: remote_management::file_manager::FileTransferRequest,
    ) {
        match request {
            remote_management::file_manager::FileTransferRequest::Upload {
                transfer_id,
                local_path,
                remote_path,
            } => {
                self.unignore_file_transfer(&client_id, transfer_id);
                eprintln!(
                    "debug event=admin_file_transfer_request client={} id={} direction=upload local_path={} remote_path={}",
                    client_id, transfer_id, local_path, remote_path
                );
                let cancel_flag = Arc::new(AtomicBool::new(false));
                if let Ok(mut flags) = self.file_transfer_cancel_flags.lock() {
                    flags.insert(transfer_id, cancel_flag.clone());
                }
                let input_tx = self.input_tx.clone();
                let flags = self.file_transfer_cancel_flags.clone();
                let sink =
                    AdminEventSink::new(self.event_tx.clone(), Some(self.repaint_handle.clone()));
                let worker_client_id = client_id.clone();
                thread::spawn(move || {
                    let result = run_file_upload_transfer(
                        &input_tx,
                        &worker_client_id,
                        transfer_id,
                        &local_path,
                        &remote_path,
                        cancel_flag,
                    );
                    if let Ok(mut flags) = flags.lock() {
                        flags.remove(&transfer_id);
                    }
                    if let Err(error) = result {
                        let _ = send_upload_cancel(
                            &input_tx,
                            &worker_client_id,
                            transfer_id,
                            &remote_path,
                        );
                        if error.kind() == io::ErrorKind::Interrupted {
                            return;
                        }
                        sink.send(AdminEvent::FileTransfer(file_transfer_message(
                            worker_client_id,
                            transfer_id,
                            FileTransferDirection::Upload,
                            FileTransferAction::Error,
                            remote_path.clone(),
                            String::new(),
                            0,
                            0,
                            0,
                            0,
                            Vec::new(),
                            format!("upload failed: {error}"),
                        )));
                    }
                });
                self.push_log(format!(
                    "queued file upload id={transfer_id} to {client_id}"
                ));
            }
            remote_management::file_manager::FileTransferRequest::Download {
                transfer_id,
                remote_path,
                local_dir,
            } => {
                self.unignore_file_transfer(&client_id, transfer_id);
                eprintln!(
                    "debug event=admin_file_transfer_request client={} id={} direction=download remote_path={} local_dir={}",
                    client_id, transfer_id, remote_path, local_dir
                );
                let input_tx = self.input_tx.clone();
                let download_message = file_transfer_message(
                    client_id.clone(),
                    transfer_id,
                    FileTransferDirection::Download,
                    FileTransferAction::Start,
                    remote_path,
                    String::new(),
                    0,
                    0,
                    0,
                    0,
                    Vec::new(),
                    local_dir,
                );
                thread::spawn(move || {
                    let _ = send_file_transfer_input(&input_tx, download_message);
                });
                self.push_log(format!(
                    "queued file download id={transfer_id} from {client_id}"
                ));
            }
            remote_management::file_manager::FileTransferRequest::Cancel {
                transfer_id,
                direction,
                remote_path,
            } => {
                let should_reconnect_after_cancel = direction == FileTransferDirection::Download;
                self.ignore_file_transfer(&client_id, transfer_id);
                eprintln!(
                    "debug event=admin_file_transfer_request client={} id={} direction={} action=cancel remote_path={}",
                    client_id,
                    transfer_id,
                    direction.as_str(),
                    remote_path
                );
                if let Ok(flags) = self.file_transfer_cancel_flags.lock() {
                    if let Some(flag) = flags.get(&transfer_id) {
                        flag.store(true, Ordering::Relaxed);
                    }
                }
                let input_tx = self.input_tx.clone();
                let cancel_message = file_transfer_message(
                    client_id.clone(),
                    transfer_id,
                    direction,
                    FileTransferAction::Cancel,
                    remote_path,
                    String::new(),
                    0,
                    0,
                    0,
                    0,
                    Vec::new(),
                    "cancel requested".to_string(),
                );
                thread::spawn(move || {
                    let _ = send_file_transfer_input(&input_tx, cancel_message);
                    if should_reconnect_after_cancel {
                        let _ = input_tx.send(AdminInput::Reconnect {
                            reason: format!("cancelled download transfer id={transfer_id}"),
                        });
                    }
                });
                self.push_log(format!(
                    "cancel file transfer id={transfer_id} on {client_id}"
                ));
            }
        }
    }

    fn render_desktop_windows(&mut self, ctx: &egui::Context) {
        for outbound in live_control::remote_desktop::render_windows(ctx, &mut self.desktop_windows)
        {
            let message = if outbound.input {
                AdminInput::DesktopInput {
                    target_id: outbound.client_id.clone(),
                    payload: outbound.payload,
                }
            } else if video_stream_payload(&outbound.payload) {
                AdminInput::VideoControl {
                    target_id: outbound.client_id.clone(),
                    source: VideoSource::RemoteDesktop,
                    payload: outbound.payload,
                }
            } else {
                AdminInput::DesktopControl {
                    target_id: outbound.client_id.clone(),
                    payload: outbound.payload,
                }
            };
            let _ = self.input_tx.send(message);
        }
    }

    fn render_camera_windows(&mut self, ctx: &egui::Context) {
        for outbound in live_control::camera::render_windows(ctx, &mut self.camera_windows) {
            let message = if video_stream_payload(&outbound.payload) {
                AdminInput::VideoControl {
                    target_id: outbound.client_id.clone(),
                    source: VideoSource::Camera,
                    payload: outbound.payload,
                }
            } else {
                AdminInput::Command {
                    target_id: outbound.client_id.clone(),
                    command: CommandKind::Camera,
                    payload: outbound.payload,
                }
            };
            let _ = self.input_tx.send(message);
        }
    }

    fn render_terminal_windows(&mut self, ctx: &egui::Context) {
        for outbound in
            remote_management::remote_terminal::render_windows(ctx, &mut self.terminal_windows)
        {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: CommandKind::RemoteTerminal,
                payload: outbound.command,
            });
            self.push_log(format!("sent remote_terminal to {}", outbound.client_id));
        }
    }
}

impl eframe::App for AdminApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let changed = self.drain_events();

        ui.painter().rect_filled(ui.max_rect(), 0.0, COLOR_BG);
        ui.add_space(18.0);
        ui.vertical_centered_justified(|ui| {
            ui.set_max_width(1120.0);
            self.render_menu_bar(ui);
            ui.add_space(12.0);
            self.render_overview(ui);
            ui.add_space(12.0);
            self.render_clients(ui);
            ui.add_space(12.0);
            self.render_activity(ui);
        });
        self.render_command_windows(ui.ctx());
        self.render_file_manager_windows(ui.ctx());
        self.render_desktop_windows(ui.ctx());
        self.render_camera_windows(ui.ctx());
        self.render_terminal_windows(ui.ctx());
        self.render_chat_windows(ui.ctx());
        self.render_interaction_command_windows(ui.ctx());
        self.render_session_command_windows(ui.ctx());
        self.render_execute_windows(ui.ctx());

        if changed {
            ui.ctx().request_repaint();
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(GUI_FRAME_INTERVAL_MS));
        }
    }
}

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const COLOR_ACCENT: egui::Color32 = egui::Color32::from_rgb(35, 99, 188);
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);
const TABLE_BODY_TEXT_SIZE: f32 = 11.5;
const TABLE_HEADER_TEXT_SIZE: f32 = 11.5;
const TABLE_BODY_CELL_HEIGHT: f32 = 16.0;
const TABLE_HEADER_CELL_HEIGHT: f32 = 17.0;
const TABLE_SORT_MARKER_WIDTH: f32 = 12.0;
const TABLE_WIDTH_SAMPLE_ROWS: usize = 200;
const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;
const ACTIVITY_LOG_LIMIT: usize = 300;

fn apply_admin_theme(ctx: &egui::Context) {
    install_cjk_font(ctx);

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
    style.visuals.selection.stroke.color = COLOR_ACCENT;
    ctx.set_global_style(style);
}

fn install_cjk_font(ctx: &egui::Context) {
    let Some(font_bytes) = load_system_cjk_font() else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    let font_name = "rdl_cjk_fallback".to_string();
    fonts.font_data.insert(
        font_name.clone(),
        Arc::new(egui::FontData::from_owned(font_bytes)),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, font_name.clone());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push(font_name);
    ctx.set_fonts(fonts);
}

fn load_system_cjk_font() -> Option<Vec<u8>> {
    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];

    candidates.iter().find_map(|path| std::fs::read(path).ok())
}

fn panel(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(8.0)
        .inner_margin(14.0)
        .show(ui, add_contents);
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(14.0)
            .color(COLOR_TEXT)
            .strong(),
    );
}

fn table_header(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(12.0)
            .color(COLOR_MUTED)
            .strong(),
    );
}

fn centered_cell(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(
        egui::Layout::left_to_right(egui::Align::Center),
        add_contents,
    );
}

fn cell_label(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(
        egui::Label::new(egui::RichText::new(text).size(12.0))
            .selectable(false)
            .sense(egui::Sense::hover()),
    );
}

fn connection_status_pill(ui: &mut egui::Ui, connected: bool) {
    let (text, color) = if connected {
        ("Online", COLOR_GOOD)
    } else {
        ("Reconnecting", COLOR_BAD)
    };
    status_badge(ui, text, color);
}

fn client_status_text(ui: &mut egui::Ui, status: ClientStatus) {
    let (text, color) = match status {
        ClientStatus::Online => ("Online", COLOR_GOOD),
        ClientStatus::Stale => ("Stale", COLOR_WARN),
        ClientStatus::Offline => ("Offline", COLOR_BAD),
    };
    ui.add(
        egui::Label::new(egui::RichText::new(text).size(12.0).color(color).strong())
            .selectable(false)
            .sense(egui::Sense::hover()),
    );
}

fn timestamped_log(line: impl Into<String>) -> String {
    format!("[{}] {}", activity_time_label(), line.into())
}

fn prune_activity_logs(log_lines: &mut Vec<String>) {
    if log_lines.len() > ACTIVITY_LOG_LIMIT {
        log_lines.drain(0..log_lines.len() - ACTIVITY_LOG_LIMIT);
    }
}

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

fn update_command_window(
    window: &mut CommandResultWindow,
    accepted: bool,
    detail: String,
    hostname: String,
    username: String,
) {
    window.status = if accepted {
        CommandResultStatus::Accepted
    } else {
        CommandResultStatus::Failed
    };
    window.detail = detail;
    window.hostname = hostname;
    window.username = username;
    window.open = true;
}

fn refresh_command_window(
    input_tx: &SyncSender<AdminInput>,
    window: &mut CommandResultWindow,
    detail: &str,
    log_prefix: &str,
    now: Instant,
    pending_logs: &mut Vec<String>,
) {
    let _ = input_tx.send(AdminInput::Command {
        target_id: window.client_id.clone(),
        command: window.command.clone(),
        payload: String::new(),
    });
    window.status = CommandResultStatus::Pending;
    window.detail = detail.to_string();
    window.open = true;
    if window.command == CommandKind::PerformanceMonitor {
        window.last_auto_refresh_at = Some(now);
    }
    pending_logs.push(format!(
        "{log_prefix} command={} to {}",
        window.command.as_str(),
        window.client_id
    ));
}

fn performance_auto_refresh_due(window: &mut CommandResultWindow, now: Instant) -> bool {
    if window.command != CommandKind::PerformanceMonitor || !window.open {
        window.last_auto_refresh_at = None;
        return false;
    }
    if !window.auto_refresh_enabled.load(Ordering::Relaxed) {
        window.last_auto_refresh_at = None;
        return false;
    }
    if matches!(window.status, CommandResultStatus::Pending) {
        return false;
    }

    let Some(last_refresh) = window.last_auto_refresh_at else {
        window.last_auto_refresh_at = Some(now);
        return false;
    };
    now.duration_since(last_refresh) >= PERFORMANCE_AUTO_REFRESH_INTERVAL
}

fn render_command_window_status_bar(
    ui: &mut egui::Ui,
    status: &CommandResultStatus,
    notice: Option<&str>,
) {
    let (status_text, default_progress_text, color) = command_window_status(status);
    let progress_text = notice.unwrap_or(default_progress_text);
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_min_height(26.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(
                    egui::RichText::new(status_text)
                        .size(12.0)
                        .color(COLOR_TEXT)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(progress_text)
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
            });
        });
}

fn command_window_status(
    status: &CommandResultStatus,
) -> (&'static str, &'static str, egui::Color32) {
    match status {
        CommandResultStatus::Pending => ("Pending", "Waiting for client result", COLOR_WARN),
        CommandResultStatus::Accepted => ("Done", "Result received", COLOR_GOOD),
        CommandResultStatus::Failed => ("Failed", "Command failed", COLOR_BAD),
    }
}

fn command_window_identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

fn command_status_notice(
    command: &CommandKind,
    status: CommandResultStatus,
    detail: &str,
) -> Option<String> {
    if matches!(status, CommandResultStatus::Accepted) {
        if command_expects_result_table(command) && parse_result_table(detail).is_none() {
            return Some("Table data could not be parsed".to_string());
        }
        if matches!(command, CommandKind::PerformanceMonitor)
            && !detail.trim().is_empty()
            && parse_performance_metrics(detail).is_empty()
        {
            return Some("Performance metrics could not be parsed".to_string());
        }
        if command_allows_plain_detail(command, detail) {
            return Some("Result received".to_string());
        }
    }
    None
}

fn command_expects_result_table(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::ProcessManager
            | CommandKind::WindowManager
            | CommandKind::StartupManager
            | CommandKind::RegistryManager
            | CommandKind::DriverManager
            | CommandKind::EventLog
            | CommandKind::ActiveConnections
    )
}

fn command_allows_plain_detail(command: &CommandKind, detail: &str) -> bool {
    if detail.trim().is_empty() || command_expects_result_table(command) {
        return false;
    }
    if matches!(command, CommandKind::Camera) && parse_camera_frame(detail).is_some() {
        return false;
    }
    true
}

fn kill_target_process_succeeded(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("ok")
        && !detail.contains("refused")
        && !detail.contains("requires")
        && !detail.contains("failed")
        && !detail.contains("exited with error")
}

fn quiet_user_interaction_command(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::OpenTextInNotepad
    )
}

fn session_command_requires_confirmation(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::UpdateClient
            | CommandKind::UninstallClient
            | CommandKind::KillClientProcess
            | CommandKind::Shutdown
            | CommandKind::Reboot
            | CommandKind::DeleteClient
    )
}

fn detail_status(detail: &str) -> Option<String> {
    payload_field(detail, "status")
}

fn render_command_result(
    ui: &mut egui::Ui,
    command: &CommandKind,
    detail: &mut String,
    table_filter: &Arc<Mutex<String>>,
    table_sort: &Arc<Mutex<Option<TableSort>>>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    refresh_requested: &Arc<AtomicBool>,
    auto_refresh_enabled: &Arc<AtomicBool>,
    refresh_in_flight: bool,
    process_kill_requested: &Arc<Mutex<Option<String>>>,
) {
    if command_expects_result_table(command) {
        render_table_toolbar(ui, table_filter, refresh_requested, refresh_in_flight);
        ui.add_space(8.0);
        if let Some(table) = parse_result_table(detail) {
            render_result_table(
                ui,
                command,
                &table,
                table_filter,
                table_sort,
                table_selected_row,
                process_kill_requested,
            );
            return;
        }
        return;
    }
    if matches!(command, CommandKind::PerformanceMonitor) {
        render_performance_monitor_toolbar(
            ui,
            refresh_requested,
            auto_refresh_enabled,
            refresh_in_flight,
        );
        ui.add_space(8.0);
        render_performance_monitor_detail(ui, detail);
        return;
    }
    if matches!(command, CommandKind::Camera) {
        if render_camera_result(ui, detail) {
            ui.add_space(8.0);
        } else {
            render_plain_command_detail(ui, detail);
        }
        return;
    }
    render_plain_command_detail(ui, detail);
}

struct PerformanceMetrics {
    cpu: Option<PerformanceMetric>,
    memory: Option<PerformanceMetric>,
    disk: Option<PerformanceMetric>,
}

impl PerformanceMetrics {
    fn is_empty(&self) -> bool {
        self.cpu.is_none() && self.memory.is_none() && self.disk.is_none()
    }

    fn bars(&self) -> Vec<&PerformanceMetric> {
        [&self.cpu, &self.memory, &self.disk]
            .into_iter()
            .flatten()
            .collect()
    }
}

struct PerformanceMetric {
    label: &'static str,
    percent: f32,
    value: String,
    color: egui::Color32,
}

fn render_performance_monitor_detail(ui: &mut egui::Ui, detail: &mut String) {
    let metrics = parse_performance_metrics(detail);
    if !metrics.is_empty() {
        render_performance_metric_bars(ui, &metrics);
        ui.add_space(10.0);
    }
    render_plain_command_detail(ui, detail);
}

fn render_performance_metric_bars(ui: &mut egui::Ui, metrics: &PerformanceMetrics) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("Resource usage")
                    .size(12.0)
                    .color(COLOR_TEXT)
                    .strong(),
            );
            ui.add_space(8.0);
            for metric in metrics.bars() {
                render_performance_metric_bar(ui, metric);
                ui.add_space(6.0);
            }
        });
}

fn render_performance_metric_bar(ui: &mut egui::Ui, metric: &PerformanceMetric) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [82.0, 20.0],
            egui::Label::new(
                egui::RichText::new(metric.label)
                    .size(12.0)
                    .color(COLOR_MUTED),
            )
            .truncate(),
        );
        let bar_width = (ui.available_width() - 4.0).max(120.0);
        ui.add_sized(
            [bar_width, 20.0],
            egui::ProgressBar::new((metric.percent / 100.0).clamp(0.0, 1.0))
                .fill(metric.color)
                .text(metric.value.clone()),
        );
    });
}

fn render_plain_command_detail(ui: &mut egui::Ui, detail: &mut String) {
    if detail.trim().is_empty() {
        return;
    }
    ui.add(
        egui::TextEdit::multiline(detail)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .desired_rows(18)
            .interactive(true),
    );
}

fn parse_performance_metrics(detail: &str) -> PerformanceMetrics {
    PerformanceMetrics {
        cpu: parse_cpu_metric(detail),
        memory: parse_memory_metric(detail),
        disk: parse_disk_metric(detail),
    }
}

fn parse_cpu_metric(detail: &str) -> Option<PerformanceMetric> {
    parse_named_number(
        detail,
        &["cpu_percent", "cpupercent", "LoadPercent", "LoadPercentage"],
    )
    .map(|value| percent_metric("CPU", value, egui::Color32::from_rgb(35, 99, 188)))
    .or_else(|| {
        let load = parse_load_average(detail)?;
        let cores = std::thread::available_parallelism()
            .map(|value| value.get() as f32)
            .unwrap_or(1.0)
            .max(1.0);
        Some(PerformanceMetric {
            label: "CPU Load",
            percent: clamp_percent(load * 100.0 / cores),
            value: format!("{load:.2} load"),
            color: egui::Color32::from_rgb(35, 99, 188),
        })
    })
}

fn parse_memory_metric(detail: &str) -> Option<PerformanceMetric> {
    parse_named_number(
        detail,
        &["memory_percent", "memorypercent", "MemoryPercent"],
    )
    .or_else(|| parse_windows_memory_percent(detail))
    .or_else(|| parse_linux_memory_percent(detail))
    .or_else(|| parse_macos_memory_percent(detail))
    .map(|value| percent_metric("Memory", value, egui::Color32::from_rgb(24, 135, 84)))
}

fn parse_disk_metric(detail: &str) -> Option<PerformanceMetric> {
    parse_named_number(detail, &["disk_percent", "diskpercent", "DiskPercent"])
        .or_else(|| parse_df_disk_percent(detail))
        .map(|value| percent_metric("Disk", value, egui::Color32::from_rgb(179, 116, 28)))
}

fn percent_metric(label: &'static str, value: f32, color: egui::Color32) -> PerformanceMetric {
    let percent = clamp_percent(value);
    PerformanceMetric {
        label,
        percent,
        value: format!("{percent:.1}%"),
        color,
    }
}

fn clamp_percent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn parse_named_number(detail: &str, keys: &[&str]) -> Option<f32> {
    detail.lines().find_map(|line| {
        keys.iter()
            .find_map(|key| parse_number_after_key(line.trim(), key))
    })
}

fn parse_number_after_key(line: &str, key: &str) -> Option<f32> {
    let key_len = key.len();
    let prefix = line.get(..key_len)?;
    if !prefix.eq_ignore_ascii_case(key) {
        return None;
    }
    let rest = line[key_len..]
        .trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '='));
    first_number(rest)
}

fn parse_load_average(detail: &str) -> Option<f32> {
    detail.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        ["load average:", "load averages:"]
            .into_iter()
            .find_map(|marker| {
                let index = lower.find(marker)?;
                first_number(&line[index + marker.len()..])
            })
    })
}

fn parse_windows_memory_percent(detail: &str) -> Option<f32> {
    let total = parse_named_number(detail, &["TotalMemoryMB"])?;
    let free = parse_named_number(detail, &["FreeMemoryMB"])?;
    (total > 0.0).then_some((total - free) * 100.0 / total)
}

fn parse_linux_memory_percent(detail: &str) -> Option<f32> {
    detail.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("Mem:")?;
        let values = rest
            .split_whitespace()
            .filter_map(first_number)
            .collect::<Vec<_>>();
        let total = *values.first()?;
        let used = *values.get(1)?;
        (total > 0.0).then_some(used * 100.0 / total)
    })
}

fn parse_macos_memory_percent(detail: &str) -> Option<f32> {
    let free = parse_vm_stat_pages(detail, "Pages free")?;
    let active = parse_vm_stat_pages(detail, "Pages active").unwrap_or(0.0);
    let inactive = parse_vm_stat_pages(detail, "Pages inactive").unwrap_or(0.0);
    let speculative = parse_vm_stat_pages(detail, "Pages speculative").unwrap_or(0.0);
    let wired = parse_vm_stat_pages(detail, "Pages wired down").unwrap_or(0.0);
    let compressed = parse_vm_stat_pages(detail, "Pages occupied by compressor").unwrap_or(0.0);
    let used = active + inactive + wired + compressed;
    let total = used + free + speculative;
    (total > 0.0).then_some(used * 100.0 / total)
}

fn parse_vm_stat_pages(detail: &str, label: &str) -> Option<f32> {
    detail.lines().find_map(|line| {
        let trimmed = line.trim_start();
        let prefix = trimmed.get(..label.len())?;
        if !prefix.eq_ignore_ascii_case(label) {
            return None;
        }
        first_number(&trimmed[label.len()..])
    })
}

fn parse_df_disk_percent(detail: &str) -> Option<f32> {
    let mut lines = detail.lines();
    while lines
        .next()
        .is_some_and(|line| !line.split_whitespace().any(|cell| cell == "Filesystem"))
    {}
    lines.find_map(|line| {
        line.split_whitespace()
            .find_map(|cell| cell.strip_suffix('%').and_then(first_number))
    })
}

fn first_number(value: &str) -> Option<f32> {
    let mut number = String::new();
    let mut started = false;
    for ch in value.chars() {
        if ch.is_ascii_digit() || matches!(ch, '.' | '-') {
            number.push(ch);
            started = true;
        } else if ch == ',' && started {
            continue;
        } else if started {
            break;
        }
    }
    if number.is_empty() || number == "." || number == "-" {
        return None;
    }
    number.parse::<f32>().ok()
}

fn render_camera_result(ui: &mut egui::Ui, detail: &str) -> bool {
    let Some(frame) = parse_camera_frame(detail) else {
        return false;
    };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(frame.image_base64) {
        Ok(bytes) => bytes,
        Err(error) => {
            ui.label(
                egui::RichText::new(format!("decode camera frame failed: {error}"))
                    .color(COLOR_BAD),
            );
            return true;
        }
    };
    let image = match image::load_from_memory(&bytes) {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            ui.label(
                egui::RichText::new(format!("load camera frame failed: {error}")).color(COLOR_BAD),
            );
            return true;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    let texture = ui.ctx().load_texture(
        format!("camera_frame:{}", stable_hash(detail)),
        color_image,
        egui::TextureOptions::LINEAR,
    );
    let available_width = ui.available_width().max(1.0);
    let scale = (available_width / size[0] as f32).min(1.0);
    let display_size = egui::vec2(size[0] as f32 * scale, size[1] as f32 * scale);
    ui.add(egui::Image::new(&texture).fit_to_exact_size(display_size));
    true
}

struct CameraFrame<'a> {
    image_base64: &'a str,
}

fn parse_camera_frame(detail: &str) -> Option<CameraFrame<'_>> {
    let mut lines = detail.lines();
    if lines.next()?.trim() != "camera_frame" {
        return None;
    }
    let image_base64 = lines.find_map(|line| line.strip_prefix("image_base64="))?;
    Some(CameraFrame { image_base64 })
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn render_table_toolbar(
    ui: &mut egui::Ui,
    table_filter: &Arc<Mutex<String>>,
    refresh_requested: &Arc<AtomicBool>,
    refresh_in_flight: bool,
) {
    let mut filter = table_filter
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();

    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        ui.allocate_ui_with_layout(
            egui::vec2(38.0, TOOLBAR_CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new("Filter").size(12.0).color(COLOR_MUTED));
            },
        );
        let response = ui.add_sized(
            [240.0, TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut filter)
                .hint_text("Filter table content")
                .vertical_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(mut value) = table_filter.lock() {
                *value = filter.clone();
            }
        }
        let label = if refresh_in_flight {
            "Refreshing..."
        } else {
            "Refresh"
        };
        if ui
            .add_enabled(!refresh_in_flight, egui::Button::new(label))
            .clicked()
        {
            refresh_requested.store(true, Ordering::Relaxed);
        }
    });
}

fn render_performance_monitor_toolbar(
    ui: &mut egui::Ui,
    refresh_requested: &Arc<AtomicBool>,
    auto_refresh_enabled: &Arc<AtomicBool>,
    refresh_in_flight: bool,
) {
    let mut auto_refresh = auto_refresh_enabled.load(Ordering::Relaxed);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        let label = if refresh_in_flight {
            "Refreshing..."
        } else {
            "Refresh"
        };
        if ui
            .add_enabled(!refresh_in_flight, egui::Button::new(label))
            .clicked()
        {
            refresh_requested.store(true, Ordering::Relaxed);
        }
        if ui
            .checkbox(&mut auto_refresh, "Auto refresh (5s)")
            .changed()
        {
            auto_refresh_enabled.store(auto_refresh, Ordering::Relaxed);
        }
    });
}

struct ResultTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Clone)]
struct DisplayTableRow {
    source_index: usize,
    cells: Vec<String>,
}

fn parse_result_table(detail: &str) -> Option<ResultTable> {
    let normalized = detail.replace("`t", "\t");
    let body = normalized
        .lines()
        .skip_while(|line| line.trim().is_empty() || line.trim_end().ends_with(':'))
        .collect::<Vec<_>>();
    if body.len() < 2 {
        return None;
    }

    if body.iter().any(|line| line.contains('\t')) {
        return parse_tab_table(&body);
    }

    parse_whitespace_table(&body)
}

fn parse_tab_table(lines: &[&str]) -> Option<ResultTable> {
    let headers = split_tab_row(lines.first()?)
        .into_iter()
        .map(clean_cell)
        .collect();
    let rows = lines
        .iter()
        .skip(1)
        .map(|line| {
            split_tab_row(line)
                .into_iter()
                .map(clean_cell)
                .collect::<Vec<_>>()
        })
        .filter(|row| row.len() >= 2)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        None
    } else {
        Some(ResultTable { headers, rows })
    }
}

fn parse_whitespace_table(lines: &[&str]) -> Option<ResultTable> {
    let headers = split_ws_row(lines.first()?);
    if headers.len() < 2 {
        return None;
    }
    let rows = lines
        .iter()
        .skip(1)
        .map(|line| split_ws_row(line))
        .filter(|row| row.len() >= 2)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        None
    } else {
        Some(ResultTable { headers, rows })
    }
}

fn split_tab_row(line: &str) -> Vec<&str> {
    line.split('\t')
        .filter(|cell| !cell.trim().is_empty())
        .collect()
}

fn split_ws_row(line: &str) -> Vec<String> {
    line.split_whitespace().map(clean_cell).collect()
}

fn clean_cell(value: impl AsRef<str>) -> String {
    value.as_ref().trim().to_string()
}

fn render_result_table(
    ui: &mut egui::Ui,
    command: &CommandKind,
    table: &ResultTable,
    table_filter: &Arc<Mutex<String>>,
    table_sort: &Arc<Mutex<Option<TableSort>>>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    process_kill_requested: &Arc<Mutex<Option<String>>>,
) {
    let filter = table_filter
        .lock()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let mut sort = table_sort.lock().map(|value| *value).unwrap_or(None);
    let selected_row = table_selected_row
        .lock()
        .map(|value| value.clone())
        .unwrap_or(None);
    let mut rows = filtered_table_rows(table, &filter);
    sort_table_rows(&mut rows, sort);
    let row_cells = rows.iter().map(|row| row.cells.clone()).collect::<Vec<_>>();
    let widths = table_column_widths(command, &table.headers, &row_cells, ui.available_width());
    let alignments = table_column_alignments(command, &table.headers);

    egui::Frame::default()
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(6.0)
        .show(ui, |ui| {
            table_header_row(ui, &table.headers, &widths, &alignments, &mut sort);
            for (row_index, row) in rows.iter().enumerate() {
                let row_key = table_row_key(row);
                table_row(
                    ui,
                    &row.cells,
                    &widths,
                    &alignments,
                    false,
                    row_index,
                    selected_row.as_deref() == Some(row_key.as_str()),
                    &row_key,
                    table_selected_row,
                    process_row_pid(command, &table.headers, &row.cells),
                    process_kill_requested,
                );
            }
        });

    if let Ok(mut value) = table_sort.lock() {
        *value = sort;
    }
    if rows.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("No rows match the current filter.")
                .size(12.0)
                .color(COLOR_MUTED),
        );
    }
}

fn filtered_table_rows(table: &ResultTable, filter: &str) -> Vec<DisplayTableRow> {
    table
        .rows
        .iter()
        .enumerate()
        .filter(|row| {
            let row = row.1;
            filter.is_empty()
                || row
                    .iter()
                    .any(|cell| cell.to_ascii_lowercase().contains(filter))
        })
        .map(|(source_index, cells)| DisplayTableRow {
            source_index,
            cells: cells.clone(),
        })
        .collect()
}

fn sort_table_rows(rows: &mut [DisplayTableRow], sort: Option<TableSort>) {
    let Some(sort) = sort else {
        return;
    };
    rows.sort_by(|left, right| {
        let left_cell = left
            .cells
            .get(sort.column)
            .map(String::as_str)
            .unwrap_or("");
        let right_cell = right
            .cells
            .get(sort.column)
            .map(String::as_str)
            .unwrap_or("");
        let ordering = compare_table_cells(left_cell, right_cell)
            .then_with(|| left.source_index.cmp(&right.source_index));
        if sort.ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn compare_table_cells(left: &str, right: &str) -> std::cmp::Ordering {
    match (left.trim().parse::<f64>(), right.trim().parse::<f64>()) {
        (Ok(left), Ok(right)) => left
            .partial_cmp(&right)
            .unwrap_or(std::cmp::Ordering::Equal),
        _ => left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase()),
    }
}

fn table_row_key(row: &DisplayTableRow) -> String {
    format!("{}\t{}", row.source_index, row.cells.join("\t"))
}

fn table_header_row(
    ui: &mut egui::Ui,
    cells: &[String],
    widths: &[f32],
    alignments: &[egui::Align],
    sort: &mut Option<TableSort>,
) {
    let fill = egui::Color32::from_rgb(235, 240, 247);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        for (index, width) in widths.iter().enumerate() {
            let cell = cells.get(index).map(String::as_str).unwrap_or("");
            let align = alignments.get(index).copied().unwrap_or(egui::Align::Min);
            let marker = match sort {
                Some(current) if current.column == index && current.ascending => " ^",
                Some(current) if current.column == index => " v",
                _ => "",
            };
            egui::Frame::default()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
                .inner_margin(egui::Margin::symmetric(5, 2))
                .show(ui, |ui| {
                    ui.set_width(*width);
                    let response = ui.add_sized(
                        [*width, TABLE_HEADER_CELL_HEIGHT],
                        egui::Label::new(
                            egui::RichText::new(format!("{cell}{marker}"))
                                .size(TABLE_HEADER_TEXT_SIZE)
                                .color(COLOR_MUTED)
                                .strong(),
                        )
                        .selectable(false)
                        .truncate()
                        .halign(align)
                        .sense(egui::Sense::click()),
                    );
                    if response.clicked() {
                        *sort = match sort {
                            Some(current) if current.column == index => Some(TableSort {
                                column: index,
                                ascending: !current.ascending,
                            }),
                            _ => Some(TableSort {
                                column: index,
                                ascending: true,
                            }),
                        };
                    }
                });
        }
    });
}

fn table_row(
    ui: &mut egui::Ui,
    cells: &[String],
    widths: &[f32],
    alignments: &[egui::Align],
    header: bool,
    row_index: usize,
    selected: bool,
    row_key: &str,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    process_id: Option<String>,
    process_kill_requested: &Arc<Mutex<Option<String>>>,
) {
    let fill = if selected {
        egui::Color32::from_rgb(219, 234, 254)
    } else if header {
        egui::Color32::from_rgb(235, 240, 247)
    } else if row_index % 2 == 0 {
        COLOR_PANEL
    } else {
        egui::Color32::from_rgb(248, 250, 253)
    };

    let row_text = cells.join("\t");
    let pointer_pos = ui.ctx().pointer_latest_pos();
    let mut pointer_cell = None;
    let response = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            for (index, width) in widths.iter().enumerate() {
                let cell = cells.get(index).map(String::as_str).unwrap_or("");
                let align = alignments.get(index).copied().unwrap_or(egui::Align::Min);
                let frame_response = egui::Frame::default()
                    .fill(fill)
                    .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
                    .inner_margin(egui::Margin::symmetric(5, 2))
                    .show(ui, |ui| {
                        ui.set_width(*width);
                        ui.add_sized(
                            [*width, TABLE_BODY_CELL_HEIGHT],
                            egui::Label::new(
                                egui::RichText::new(cell)
                                    .size(TABLE_BODY_TEXT_SIZE)
                                    .color(if header { COLOR_MUTED } else { COLOR_TEXT }),
                            )
                            .selectable(false)
                            .truncate()
                            .halign(align)
                            .sense(egui::Sense::hover()),
                        );
                    })
                    .response;
                if pointer_pos.is_some_and(|pos| frame_response.rect.contains(pos)) {
                    pointer_cell = Some(cell.to_string());
                }
            }
        })
        .response
        .interact(egui::Sense::click());
    if response.hovered() && !header {
        response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if (response.clicked() || response.secondary_clicked()) && !header {
        if let Ok(mut value) = table_selected_row.lock() {
            *value = Some(row_key.to_string());
        }
    }
    response.context_menu(|ui| {
        if let Some(cell) = pointer_cell.as_deref() {
            if ui.button("Copy Cell").clicked() {
                ui.ctx().copy_text(cell.to_string());
                ui.close();
            }
        }
        if ui.button("Copy Row").clicked() {
            ui.ctx().copy_text(row_text.clone());
            ui.close();
        }
        if let Some(process_id) = process_id.clone() {
            ui.separator();
            if ui.button("Kill Process").clicked() {
                if let Ok(mut selected) = table_selected_row.lock() {
                    *selected = Some(row_key.to_string());
                }
                if let Ok(mut value) = process_kill_requested.lock() {
                    *value = Some(process_id.clone());
                }
                ui.close();
            }
        }
    });
}

fn process_row_pid(command: &CommandKind, headers: &[String], row: &[String]) -> Option<String> {
    if !matches!(
        command,
        CommandKind::ProcessManager | CommandKind::WindowManager
    ) {
        return None;
    }
    let pid_index = headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case("pid"))?;
    let pid = row.get(pid_index)?.trim();
    if pid != "0" && pid.chars().all(|ch| ch.is_ascii_digit()) {
        Some(pid.to_string())
    } else {
        None
    }
}

fn table_column_widths(
    command: &CommandKind,
    headers: &[String],
    rows: &[Vec<String>],
    available_width: f32,
) -> Vec<f32> {
    let specs = headers
        .iter()
        .map(|header| table_column_spec(command, header))
        .collect::<Vec<_>>();
    let mut widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            let spec = specs[index];
            let header_width = estimated_table_text_width(header) + TABLE_SORT_MARKER_WIDTH;
            let content_width = rows
                .iter()
                .take(TABLE_WIDTH_SAMPLE_ROWS)
                .filter_map(|row| row.get(index))
                .map(|cell| estimated_table_text_width(cell))
                .fold(0.0, f32::max);

            header_width.max(content_width).clamp(spec.min, spec.max)
        })
        .collect::<Vec<_>>();

    if available_width.is_finite() {
        distribute_extra_table_width(&mut widths, &specs, available_width);
    }

    widths
}

fn distribute_extra_table_width(
    widths: &mut [f32],
    specs: &[TableColumnSpec],
    available_width: f32,
) {
    let mut extra = available_width - widths.iter().sum::<f32>();
    while extra > 1.0 {
        let total_stretch = specs
            .iter()
            .enumerate()
            .filter(|(index, spec)| spec.stretch > 0.0 && widths[*index] < spec.max)
            .map(|(_, spec)| spec.stretch)
            .sum::<f32>();
        if total_stretch <= 0.0 {
            break;
        }

        let mut used = 0.0;
        for (width, spec) in widths.iter_mut().zip(specs.iter()) {
            if spec.stretch <= 0.0 || *width >= spec.max {
                continue;
            }

            let room = spec.max - *width;
            let grow = (extra * spec.stretch / total_stretch).min(room);
            *width += grow;
            used += grow;
        }

        if used <= 0.5 {
            break;
        }
        extra -= used;
    }
}

fn table_column_alignments(command: &CommandKind, headers: &[String]) -> Vec<egui::Align> {
    headers
        .iter()
        .map(|header| table_column_spec(command, header).align)
        .collect()
}

#[derive(Clone, Copy)]
struct TableColumnSpec {
    min: f32,
    max: f32,
    stretch: f32,
    align: egui::Align,
}

fn table_column_spec(command: &CommandKind, header: &str) -> TableColumnSpec {
    match command {
        CommandKind::ProcessManager => process_column_spec(header),
        CommandKind::WindowManager => window_column_spec(header),
        CommandKind::StartupManager => startup_column_spec(header),
        CommandKind::RegistryManager => registry_column_spec(header),
        CommandKind::DriverManager => driver_column_spec(header),
        CommandKind::EventLog => event_log_column_spec(header),
        CommandKind::ActiveConnections => connection_column_spec(header),
        _ => default_column_spec(header),
    }
}

fn column_spec(min: f32, max: f32, stretch: f32, align: egui::Align) -> TableColumnSpec {
    TableColumnSpec {
        min,
        max,
        stretch,
        align,
    }
}

fn process_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "pid" | "ppid" => column_spec(42.0, 64.0, 0.0, egui::Align::Max),
        "cpu" | "pcpu" | "%cpu" | "mem" | "pmem" | "%mem" => {
            column_spec(48.0, 76.0, 0.0, egui::Align::Max)
        }
        "memorymb" => column_spec(70.0, 96.0, 0.0, egui::Align::Max),
        "name" | "processname" | "comm" => column_spec(110.0, 260.0, 1.0, egui::Align::Min),
        "command" => column_spec(180.0, 560.0, 3.0, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn window_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "windowid" => column_spec(78.0, 130.0, 0.0, egui::Align::Min),
        "desktop" | "pid" => column_spec(48.0, 74.0, 0.0, egui::Align::Max),
        "responding" | "visible" | "status" => column_spec(70.0, 96.0, 0.0, egui::Align::Min),
        "process" | "class" => column_spec(110.0, 220.0, 0.6, egui::Align::Min),
        "title" => column_spec(220.0, 620.0, 2.4, egui::Align::Min),
        "path" => column_spec(180.0, 520.0, 1.6, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn startup_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "scope" | "source" | "status" => column_spec(86.0, 150.0, 0.0, egui::Align::Min),
        "name" => column_spec(150.0, 320.0, 0.8, egui::Align::Min),
        "command" => column_spec(220.0, 720.0, 2.6, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn registry_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "hive" | "type" => column_spec(52.0, 86.0, 0.0, egui::Align::Min),
        "path" => column_spec(190.0, 460.0, 1.5, egui::Align::Min),
        "name" => column_spec(130.0, 280.0, 0.8, egui::Align::Min),
        "value" => column_spec(220.0, 760.0, 2.4, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn driver_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "index" | "refs" | "size" | "usedby" => column_spec(48.0, 80.0, 0.0, egui::Align::Max),
        "state" | "status" | "startmode" | "version" => {
            column_spec(74.0, 120.0, 0.0, egui::Align::Min)
        }
        "name" => column_spec(160.0, 360.0, 1.2, egui::Align::Min),
        "path" | "description" | "dependencies" => column_spec(220.0, 720.0, 2.2, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn event_log_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "time" | "timecreated" => column_spec(130.0, 190.0, 0.8, egui::Align::Min),
        "level" | "leveldisplayname" => column_spec(70.0, 115.0, 0.0, egui::Align::Min),
        "provider" | "providername" => column_spec(110.0, 260.0, 1.0, egui::Align::Min),
        "id" => column_spec(42.0, 70.0, 0.0, egui::Align::Max),
        "message" => column_spec(220.0, 720.0, 3.0, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn connection_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "proto" | "netid" | "protocol" => column_spec(48.0, 72.0, 0.0, egui::Align::Min),
        "local" | "localaddress" => column_spec(140.0, 320.0, 1.0, egui::Align::Min),
        "foreign" | "peer" | "peeraddress" | "foreignaddress" => {
            column_spec(140.0, 320.0, 1.0, egui::Align::Min)
        }
        "state" => column_spec(64.0, 120.0, 0.0, egui::Align::Min),
        "pid" => column_spec(42.0, 70.0, 0.0, egui::Align::Max),
        "pid/program" | "pid/programname" => column_spec(88.0, 180.0, 0.0, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn default_column_spec(header: &str) -> TableColumnSpec {
    let key = normalized_table_header(header);
    if numeric_like_header(&key) {
        column_spec(48.0, 96.0, 0.0, egui::Align::Max)
    } else {
        column_spec(72.0, 240.0, 0.3, egui::Align::Min)
    }
}

fn normalized_table_header(header: &str) -> String {
    header
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_', '-'], "")
}

fn numeric_like_header(header: &str) -> bool {
    matches!(
        header,
        "id" | "pid" | "ppid" | "cpu" | "pcpu" | "%cpu" | "mem" | "pmem" | "%mem" | "memorymb"
    ) || header.ends_with("id")
        || header.ends_with("count")
        || header.ends_with("bytes")
        || header.ends_with("mb")
}

fn estimated_table_text_width(value: &str) -> f32 {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_whitespace() {
                3.5
            } else if ch.is_ascii() {
                6.7
            } else {
                11.0
            }
        })
        .sum::<f32>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_table_keeps_numeric_columns_compact() {
        let headers = strings(["PID", "Name", "CPU", "MemoryMB"]);
        let rows = vec![
            strings(["7", "launchd", "0.0", "13.5"]),
            strings(["12345", "very-long-process-name", "12.3", "1024.0"]),
        ];

        let widths = table_column_widths(&CommandKind::ProcessManager, &headers, &rows, 760.0);

        assert!(widths[0] <= 64.0);
        assert!(widths[2] <= 76.0);
        assert!(widths[3] <= 96.0);
        assert!(widths[1] > widths[0]);
    }

    #[test]
    fn process_table_ignores_infinite_scroll_width() {
        let headers = strings(["PID", "Name", "CPU"]);
        let rows = vec![strings(["1", "init", "0.0"])];

        let widths =
            table_column_widths(&CommandKind::ProcessManager, &headers, &rows, f32::INFINITY);

        assert!(widths.iter().all(|width| width.is_finite()));
        assert!(widths[0] <= 64.0);
    }

    #[test]
    fn detail_status_reads_session_result_status() {
        assert_eq!(
            detail_status("delete_client\nstatus=scheduled\nmessage=ok").as_deref(),
            Some("scheduled")
        );
        assert_eq!(
            detail_status("delete_client\nstatus=dry_run\nmessage=ok").as_deref(),
            Some("dry_run")
        );
    }

    #[test]
    fn table_row_keys_stay_unique_for_duplicate_display_values() {
        let table = ResultTable {
            headers: strings(["Time", "Level", "Message"]),
            rows: vec![
                strings(["2026-05-16 12:00:00", "Info", "same"]),
                strings(["2026-05-16 12:00:00", "Info", "same"]),
            ],
        };

        let rows = filtered_table_rows(&table, "");

        assert_eq!(rows.len(), 2);
        assert_ne!(table_row_key(&rows[0]), table_row_key(&rows[1]));
    }

    #[test]
    fn table_row_keys_stay_unique_after_sorting_duplicate_times() {
        let table = ResultTable {
            headers: strings(["Time", "Level", "Message"]),
            rows: vec![
                strings(["2026-05-16 12:00:00", "Info", "first"]),
                strings(["2026-05-16 12:00:00", "Warn", "second"]),
                strings(["2026-05-16 12:01:00", "Info", "third"]),
            ],
        };
        let mut rows = filtered_table_rows(&table, "");

        sort_table_rows(
            &mut rows,
            Some(TableSort {
                column: 0,
                ascending: true,
            }),
        );
        let keys = rows.iter().map(table_row_key).collect::<Vec<_>>();
        let unique = keys.iter().collect::<std::collections::HashSet<_>>();

        assert_eq!(keys.len(), unique.len());
    }

    #[test]
    fn window_manager_rows_can_request_process_kill() {
        let headers = strings(["PID", "Process", "Title"]);
        let row = strings(["1234", "Terminal", "shell"]);

        assert_eq!(
            process_row_pid(&CommandKind::WindowManager, &headers, &row).as_deref(),
            Some("1234")
        );
    }

    #[test]
    fn process_kill_action_ignores_pid_zero_rows() {
        let headers = strings(["PID", "Process", "Title"]);
        let row = strings(["0", "Info", "No windows"]);

        assert!(process_row_pid(&CommandKind::WindowManager, &headers, &row).is_none());
        assert!(process_row_pid(&CommandKind::ProcessManager, &headers, &row).is_none());
    }

    #[test]
    fn table_parse_failures_only_report_status_notice() {
        assert_eq!(
            command_status_notice(
                &CommandKind::ProcessManager,
                CommandResultStatus::Accepted,
                "not a table"
            )
            .as_deref(),
            Some("Table data could not be parsed")
        );
    }

    #[test]
    fn manager_commands_expect_table_results() {
        for command in [
            CommandKind::WindowManager,
            CommandKind::StartupManager,
            CommandKind::RegistryManager,
            CommandKind::DriverManager,
        ] {
            assert!(command_expects_result_table(&command));
            assert!(!command_allows_plain_detail(&command, "not a table"));
        }
    }

    #[test]
    fn plain_detail_is_suppressed_for_table_commands() {
        assert!(!command_allows_plain_detail(
            &CommandKind::EventLog,
            "not a table"
        ));
    }

    #[test]
    fn plain_detail_is_available_for_regular_command_results() {
        assert!(command_allows_plain_detail(
            &CommandKind::ComputerInfo,
            "computer_info:\nhostname=test"
        ));
        assert_eq!(
            command_status_notice(
                &CommandKind::ComputerInfo,
                CommandResultStatus::Accepted,
                "computer_info:\nhostname=test"
            )
            .as_deref(),
            Some("Result received")
        );
    }

    #[test]
    fn camera_text_errors_fall_back_to_plain_detail() {
        assert!(command_allows_plain_detail(
            &CommandKind::Camera,
            "camera_error\nmessage=no device"
        ));
    }

    #[test]
    fn camera_frames_do_not_show_raw_base64_detail() {
        assert!(!command_allows_plain_detail(
            &CommandKind::Camera,
            "camera_frame\nimage_base64=abcd"
        ));
    }

    #[test]
    fn performance_metrics_parse_structured_percent_fields() {
        let detail =
            "performance_snapshot:\ncpu_percent=12.5\nmemory_percent=45.0\ndisk_percent=67.2";

        let metrics = parse_performance_metrics(detail);

        assert_metric(metrics.cpu.as_ref(), "CPU", 12.5);
        assert_metric(metrics.memory.as_ref(), "Memory", 45.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 67.2);
    }

    #[test]
    fn performance_metrics_parse_windows_format_list() {
        let detail = "\
Cpu           : Intel
LoadPercent   : 23
TotalMemoryMB : 16000
FreeMemoryMB  : 4000
DiskPercent   : 55";

        let metrics = parse_performance_metrics(detail);

        assert_metric(metrics.cpu.as_ref(), "CPU", 23.0);
        assert_metric(metrics.memory.as_ref(), "Memory", 75.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 55.0);
    }

    #[test]
    fn performance_metrics_parse_unix_command_output() {
        let detail = "\
performance_snapshot:
 18:19:21 up 1 day,  4:51,  load average: 1.50, 1.20, 0.90
              total        used        free      shared  buff/cache   available
Mem:           1000         250         500          10         250         700
Filesystem     1024-blocks Used Available Capacity Mounted on
/dev/disk1       100000000 4200  5800     42%      .";

        let metrics = parse_performance_metrics(detail);

        assert_eq!(
            metrics.cpu.as_ref().map(|metric| metric.label),
            Some("CPU Load")
        );
        assert_eq!(
            metrics.cpu.as_ref().map(|metric| metric.value.as_str()),
            Some("1.50 load")
        );
        assert_metric(metrics.memory.as_ref(), "Memory", 25.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 42.0);
    }

    #[test]
    fn performance_metrics_parse_macos_vm_stat_memory() {
        let detail = "\
performance_snapshot:
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               100.
Pages active:                             100.
Pages inactive:                           100.
Pages speculative:                          0.
Pages wired down:                         100.
Pages occupied by compressor:             100.
Filesystem 512-blocks Used Available Capacity iused ifree %iused Mounted on
/dev/disk3s1 1000000 610000 390000 61% 1 2 1% .";

        let metrics = parse_performance_metrics(detail);

        assert_metric(metrics.memory.as_ref(), "Memory", 80.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 61.0);
    }

    #[test]
    fn performance_auto_refresh_waits_for_interval() {
        let now = Instant::now();
        let mut window = test_command_window(CommandKind::PerformanceMonitor);
        window.auto_refresh_enabled.store(true, Ordering::Relaxed);

        assert!(!performance_auto_refresh_due(&mut window, now));
        assert!(!performance_auto_refresh_due(
            &mut window,
            now + PERFORMANCE_AUTO_REFRESH_INTERVAL - Duration::from_millis(1)
        ));
        assert!(performance_auto_refresh_due(
            &mut window,
            now + PERFORMANCE_AUTO_REFRESH_INTERVAL
        ));
    }

    #[test]
    fn performance_auto_refresh_stops_when_window_closes() {
        let now = Instant::now();
        let mut window = test_command_window(CommandKind::PerformanceMonitor);
        window.auto_refresh_enabled.store(true, Ordering::Relaxed);
        window.last_auto_refresh_at = Some(now - PERFORMANCE_AUTO_REFRESH_INTERVAL);
        window.open = false;

        assert!(!performance_auto_refresh_due(&mut window, now));
        assert!(window.last_auto_refresh_at.is_none());
    }

    fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
        values.into_iter().map(str::to_string).collect()
    }

    fn assert_metric(metric: Option<&PerformanceMetric>, label: &str, percent: f32) {
        let metric = metric.expect("metric should parse");
        assert_eq!(metric.label, label);
        assert!(
            (metric.percent - percent).abs() < 0.1,
            "expected {percent}, got {}",
            metric.percent
        );
    }

    fn test_command_window(command: CommandKind) -> CommandResultWindow {
        CommandResultWindow {
            id: 1,
            client_id: "client".to_string(),
            hostname: "host".to_string(),
            username: "user".to_string(),
            command,
            status: CommandResultStatus::Accepted,
            detail: String::new(),
            open: true,
            close_requested: Arc::new(AtomicBool::new(false)),
            refresh_requested: Arc::new(AtomicBool::new(false)),
            auto_refresh_enabled: Arc::new(AtomicBool::new(false)),
            last_auto_refresh_at: None,
            process_kill_requested: Arc::new(Mutex::new(None)),
            table_filter: Arc::new(Mutex::new(String::new())),
            table_sort: Arc::new(Mutex::new(None)),
            table_selected_row: Arc::new(Mutex::new(None)),
        }
    }
}

fn status_badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::default()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(999.0)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(color).strong());
        });
}

fn compact_id(value: &str) -> String {
    if value.len() > 22 {
        format!("{}...", &value[..22])
    } else {
        value.to_string()
    }
}

fn command_title(command: &CommandKind) -> String {
    command
        .as_str()
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn last_seen_label(last_seen_epoch_ms: u128) -> String {
    if last_seen_epoch_ms == 0 {
        return "Never".to_string();
    }
    format_epoch_utc(last_seen_epoch_ms / 1000)
}

fn format_epoch_utc(epoch_seconds: u128) -> String {
    let seconds = epoch_seconds as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_param = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_param + 2) / 5 + 1;
    let month = month_param + if month_param < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(COLOR_MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).color(COLOR_TEXT).strong());
        });
    });
}

fn empty_state(ui: &mut egui::Ui) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("No clients online")
                .size(16.0)
                .color(COLOR_TEXT),
        );
        ui.label(
            egui::RichText::new("Start a client or refresh after it connects.")
                .size(13.0)
                .color(COLOR_MUTED),
        );
    });
    ui.add_space(48.0);
}

fn terminal_input_loop(input_tx: SyncSender<AdminInput>) {
    println!("commands:");
    println!("  list");
    println!("  cmd <client-id> <command-kind> [payload]");
    println!("  quit");
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed == "quit" || trimmed == "exit" {
            thread::sleep(std::time::Duration::from_millis(1200));
            let _ = input_tx.send(AdminInput::Quit);
            break;
        }
        if trimmed == "list" {
            let _ = input_tx.send(AdminInput::List);
            continue;
        }
        let mut parts = trimmed.splitn(3, ' ');
        if let (Some("cmd"), Some(target_id), Some(command)) =
            (parts.next(), parts.next(), parts.next())
        {
            let (command_name, payload) = command
                .split_once(' ')
                .map(|(name, payload)| (name, payload.to_string()))
                .unwrap_or((command, String::new()));
            if let Some(command) = CommandKind::parse(command_name) {
                let _ = input_tx.send(AdminInput::Command {
                    target_id: target_id.to_string(),
                    command,
                    payload,
                });
            }
        }
    }
}

fn video_stream_payload(payload: &str) -> bool {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| matches!(action.trim(), "start" | "stop"))
        .unwrap_or(false)
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

fn run_file_upload_transfer(
    input_tx: &SyncSender<AdminInput>,
    client_id: &str,
    transfer_id: u64,
    local_path: &str,
    remote_path: &str,
    cancel_flag: Arc<AtomicBool>,
) -> io::Result<()> {
    let source = PathBuf::from(local_path);
    let metadata = fs::metadata(&source)?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    collect_upload_entries(&source, Path::new(""), &metadata, &mut dirs, &mut files)?;
    let total_bytes = files.iter().map(|file| file.size).sum::<u64>();
    let mut transferred_bytes = 0u64;

    send_file_transfer_input_cancelable(
        input_tx,
        file_transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            FileTransferAction::Start,
            remote_path.to_string(),
            String::new(),
            total_bytes,
            0,
            0,
            0,
            Vec::new(),
            "upload started".to_string(),
        ),
        &cancel_flag,
    )?;

    for dir in dirs {
        if cancel_flag.load(Ordering::Relaxed) {
            return send_upload_cancel(input_tx, client_id, transfer_id, remote_path);
        }
        send_file_transfer_input_cancelable(
            input_tx,
            file_transfer_message(
                client_id.to_string(),
                transfer_id,
                FileTransferDirection::Upload,
                FileTransferAction::Directory,
                remote_path.to_string(),
                protocol_relative_path(&dir),
                total_bytes,
                transferred_bytes,
                0,
                0,
                Vec::new(),
                String::new(),
            ),
            &cancel_flag,
        )?;
    }

    let mut buffer = vec![0u8; FILE_TRANSFER_CHUNK_SIZE];
    for file in files {
        if cancel_flag.load(Ordering::Relaxed) {
            return send_upload_cancel(input_tx, client_id, transfer_id, remote_path);
        }
        let mut input = File::open(&file.path)?;
        let mut offset = 0u64;
        let relative_path = protocol_relative_path(&file.relative);
        loop {
            if cancel_flag.load(Ordering::Relaxed) {
                return send_upload_cancel(input_tx, client_id, transfer_id, remote_path);
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            transferred_bytes = transferred_bytes.saturating_add(count as u64);
            send_file_transfer_input_cancelable(
                input_tx,
                file_transfer_message(
                    client_id.to_string(),
                    transfer_id,
                    FileTransferDirection::Upload,
                    FileTransferAction::Chunk,
                    remote_path.to_string(),
                    relative_path.clone(),
                    total_bytes,
                    transferred_bytes,
                    file.size,
                    offset,
                    buffer[..count].to_vec(),
                    String::new(),
                ),
                &cancel_flag,
            )?;
            offset = offset.saturating_add(count as u64);
        }
    }

    send_file_transfer_input_cancelable(
        input_tx,
        file_transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            FileTransferAction::Finish,
            remote_path.to_string(),
            String::new(),
            total_bytes,
            transferred_bytes,
            0,
            0,
            Vec::new(),
            "upload finished".to_string(),
        ),
        &cancel_flag,
    )
}

fn send_upload_cancel(
    input_tx: &SyncSender<AdminInput>,
    client_id: &str,
    transfer_id: u64,
    remote_path: &str,
) -> io::Result<()> {
    send_file_transfer_input(
        input_tx,
        file_transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            FileTransferAction::Cancel,
            remote_path.to_string(),
            String::new(),
            0,
            0,
            0,
            0,
            Vec::new(),
            "upload cancelled".to_string(),
        ),
    )
}

#[derive(Clone)]
struct UploadFileEntry {
    path: PathBuf,
    relative: PathBuf,
    size: u64,
}

fn collect_upload_entries(
    path: &Path,
    relative: &Path,
    metadata: &fs::Metadata,
    dirs: &mut Vec<PathBuf>,
    files: &mut Vec<UploadFileEntry>,
) -> io::Result<()> {
    if metadata.is_dir() {
        dirs.push(relative.to_path_buf());
        let mut children = fs::read_dir(path)?.flatten().collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let child_metadata = child.metadata()?;
            let child_relative = relative.join(child.file_name());
            collect_upload_entries(&child.path(), &child_relative, &child_metadata, dirs, files)?;
        }
    } else {
        files.push(UploadFileEntry {
            path: path.to_path_buf(),
            relative: relative.to_path_buf(),
            size: metadata.len(),
        });
    }
    Ok(())
}

fn protocol_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn send_file_transfer_input(input_tx: &SyncSender<AdminInput>, message: Message) -> io::Result<()> {
    input_tx
        .send(AdminInput::FileTransfer(message))
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
}

fn send_file_transfer_input_cancelable(
    input_tx: &SyncSender<AdminInput>,
    message: Message,
    cancel_flag: &AtomicBool,
) -> io::Result<()> {
    let mut input = AdminInput::FileTransfer(message);
    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "file upload cancelled",
            ));
        }
        match input_tx.try_send(input) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                input = returned;
                thread::sleep(Duration::from_millis(5));
            }
            Err(TrySendError::Disconnected(returned)) => {
                drop(returned);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "admin input queue disconnected",
                ));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn file_transfer_message(
    target_id: String,
    transfer_id: u64,
    direction: FileTransferDirection,
    action: FileTransferAction,
    path: String,
    relative_path: String,
    total_bytes: u64,
    transferred_bytes: u64,
    file_size: u64,
    offset: u64,
    bytes: Vec<u8>,
    message: String,
) -> Message {
    Message::FileTransfer {
        target_id,
        transfer_id,
        direction,
        action,
        path,
        relative_path,
        total_bytes,
        transferred_bytes,
        file_size,
        offset,
        bytes,
        message,
    }
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

enum AdminInput {
    List,
    Command {
        target_id: String,
        command: CommandKind,
        payload: String,
    },
    DesktopControl {
        target_id: String,
        payload: String,
    },
    DesktopInput {
        target_id: String,
        payload: String,
    },
    VideoControl {
        target_id: String,
        source: VideoSource,
        payload: String,
    },
    FileTransfer(Message),
    Reconnect {
        reason: String,
    },
    Quit,
}

enum AdminEvent {
    Connected,
    Disconnected,
    Clients(Vec<ClientInfo>),
    Ack {
        client_id: String,
        command: CommandKind,
        accepted: bool,
        detail: String,
    },
    CommandOutput {
        client_id: String,
        command: CommandKind,
        stream_id: u64,
        sequence: u64,
        stream: CommandOutputStream,
        chunk: String,
        current_dir: String,
        finished: bool,
        success: bool,
    },
    DesktopFrame {
        client_id: String,
        payload: String,
    },
    DecodedDesktopFrame {
        client_id: String,
        result: Result<live_control::remote_desktop::DesktopFrame, String>,
    },
    DecodedCameraFrame {
        client_id: String,
        result: Result<live_control::camera::CameraFrame, String>,
    },
    VideoFrame {
        client_id: String,
        source: VideoSource,
        seq: u64,
        source_width: u32,
        source_height: u32,
        image_width: u32,
        image_height: u32,
        format: String,
        bytes: Vec<u8>,
    },
    FileTransfer(Message),
    Log(String),
}

#[derive(Clone)]
struct AdminEventSink {
    tx: Sender<AdminEvent>,
    repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
}

impl AdminEventSink {
    fn new(
        tx: Sender<AdminEvent>,
        repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
    ) -> Self {
        Self { tx, repaint_handle }
    }

    fn send(&self, event: AdminEvent) {
        let _ = self.tx.send(event);
        if let Some(ctx) = self
            .repaint_handle
            .as_ref()
            .and_then(|handle| handle.lock().ok().and_then(|ctx| ctx.clone()))
        {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

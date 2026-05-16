mod command_result;
mod event;
mod file_transfer;
mod network;
mod payload;
mod terminal;
mod ui;

use self::{
    command_result::{
        command_status_notice, command_title, command_window_identity_title, detail_status,
        kill_target_process_succeeded, performance_auto_refresh_due,
        quiet_user_interaction_command, refresh_command_window, render_command_result,
        render_command_window_status_bar, session_command_requires_confirmation,
        update_command_window, CommandResultStatus, CommandResultWindow,
    },
    event::{AdminEvent, AdminEventSink, AdminInput},
    file_transfer::{
        file_transfer_message, run_file_upload_transfer, sanitize_log_value,
        send_file_transfer_input, send_upload_cancel, should_log_admin_file_transfer_event,
    },
    network::admin_network_loop,
    payload::{payload_field, video_stream_payload},
    terminal::run_terminal,
    ui::{
        activity_context_menu, apply_admin_theme, cell_label, centered_cell, compact_id,
        connection_status_pill, empty_state, last_seen_label, metric, panel, prune_activity_logs,
        section_title, table_header, timestamped_log, COLOR_BAD, COLOR_BG, COLOR_GOOD, COLOR_MUTED,
        COLOR_WARN, TOOLBAR_CONTROL_HEIGHT,
    },
};
use crate::{
    command_menu, live_control, remote_management,
    runtime::{terminal_mode, Config},
    user_interaction, windowing,
};
use eframe::egui;
use rdl_protocol::{
    AudioSource, ClientInfo, CommandKind, CommandOutputStream, FileTransferAction,
    FileTransferDirection, Message, VideoSource,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::Instant;

const GUI_FRAME_INTERVAL_MS: u64 = 250;
const ADMIN_INPUT_QUEUE_CAPACITY: usize = 8;
const MAX_GUI_EVENTS_PER_FRAME: usize = 512;

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
    audio_windows: Vec<live_control::audio_listen::AudioListenWindow>,
    terminal_windows: Vec<remote_management::remote_terminal::TerminalWindow>,
    chat_windows: Vec<user_interaction::text_chat::ChatWindow>,
    voice_chat_windows: Vec<user_interaction::voice_chat::VoiceChatWindow>,
    interaction_command_windows: Vec<user_interaction::InteractionCommandWindow>,
    session_command_windows: Vec<crate::session::SessionCommandWindow>,
    execute_windows: Vec<crate::execute::ExecuteWindow>,
    file_transfer_cancel_flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
    log_lines: Vec<String>,
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
            audio_windows: Vec::new(),
            terminal_windows: Vec::new(),
            chat_windows: Vec::new(),
            voice_chat_windows: Vec::new(),
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
                AdminEvent::AudioFrame {
                    client_id,
                    source,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                } => match source {
                    AudioSource::AudioListen => {
                        match live_control::audio_listen::decode_audio_frame(
                            seq,
                            sample_rate,
                            channels,
                            format,
                            bytes,
                        ) {
                            Ok(frame) => live_control::audio_listen::handle_audio_frame(
                                &mut self.audio_windows,
                                &client_id,
                                frame,
                            ),
                            Err(message) => self.handle_audio_ack(
                                &client_id,
                                true,
                                format!("audio_listen_error\nmessage={message}"),
                            ),
                        }
                    }
                    AudioSource::VoiceChat => {
                        match user_interaction::voice_chat::decode_audio_frame(
                            seq,
                            sample_rate,
                            channels,
                            format,
                            bytes,
                        ) {
                            Ok(frame) => user_interaction::voice_chat::handle_audio_frame(
                                &mut self.voice_chat_windows,
                                &client_id,
                                frame,
                            ),
                            Err(message) => self.handle_voice_chat_ack(
                                &client_id,
                                true,
                                format!("voice_chat_error\nmessage={message}"),
                            ),
                        }
                    }
                },
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
        if command.requires_client_gui() && !self.client_gui_available(client_id) {
            self.push_log(format!(
                "blocked command={} to {}: client has no GUI session",
                command.as_str(),
                client_id
            ));
            return;
        }

        if session_command_requires_confirmation(&command) {
            self.open_session_command_window(client_id, command);
            return;
        }
        if command == CommandKind::TextChat {
            self.open_chat_window(client_id);
            return;
        }
        if command == CommandKind::VoiceChat {
            self.open_voice_chat_window(client_id);
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
        if command == CommandKind::AudioListen {
            self.open_audio_window(client_id);
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

    fn open_voice_chat_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::voice_chat::open_window(
            &mut self.voice_chat_windows,
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

    fn open_audio_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::audio_listen::open_window(
            &mut self.audio_windows,
            client_id,
            hostname,
            username,
        );
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
        user_interaction::open_window(
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

    fn client_gui_available(&self, client_id: &str) -> bool {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| row.info.gui_available)
            .unwrap_or(false)
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
        if command == CommandKind::VoiceChat {
            self.handle_voice_chat_ack(&client_id, accepted, detail);
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
        if command == CommandKind::AudioListen {
            self.handle_audio_ack(&client_id, accepted, detail);
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

    fn handle_voice_chat_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::voice_chat::handle_ack(
            &mut self.voice_chat_windows,
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

    fn handle_audio_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::audio_listen::handle_ack(
            &mut self.audio_windows,
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
                    let gui_available = self.client_gui_available(&client_id);
                    command_menu::render_context_menu(
                        ui,
                        &client_id,
                        gui_available,
                        &mut |client_id, command| {
                            self.send_command(client_id, command);
                        },
                    );
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
                                        client.gui_available,
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

    fn render_voice_chat_windows(&mut self, ctx: &egui::Context) {
        for outbound in
            user_interaction::voice_chat::render_windows(ctx, &mut self.voice_chat_windows)
        {
            match outbound {
                user_interaction::voice_chat::OutboundCommand::Command { client_id, payload } => {
                    let _ = self.input_tx.send(AdminInput::Command {
                        target_id: client_id.clone(),
                        command: CommandKind::VoiceChat,
                        payload,
                    });
                    self.push_log(format!("sent voice_chat invite to {client_id}"));
                }
                user_interaction::voice_chat::OutboundCommand::AudioControl {
                    client_id,
                    payload,
                } => {
                    let _ = self.input_tx.send(AdminInput::AudioControl {
                        target_id: client_id,
                        source: AudioSource::VoiceChat,
                        payload,
                    });
                }
                user_interaction::voice_chat::OutboundCommand::AudioFrame {
                    client_id,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                } => {
                    let _ = self.input_tx.send(AdminInput::AudioFrame {
                        target_id: client_id,
                        source: AudioSource::VoiceChat,
                        seq,
                        sample_rate,
                        channels,
                        format,
                        bytes,
                    });
                }
            }
        }
    }

    fn render_interaction_command_windows(&mut self, ctx: &egui::Context) {
        for outbound in user_interaction::render_windows(ctx, &mut self.interaction_command_windows)
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

    fn render_audio_windows(&mut self, ctx: &egui::Context) {
        for outbound in live_control::audio_listen::render_windows(ctx, &mut self.audio_windows) {
            let message = if video_stream_payload(&outbound.payload) {
                AdminInput::AudioControl {
                    target_id: outbound.client_id.clone(),
                    source: AudioSource::AudioListen,
                    payload: outbound.payload,
                }
            } else {
                AdminInput::Command {
                    target_id: outbound.client_id.clone(),
                    command: CommandKind::AudioListen,
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
        self.render_audio_windows(ui.ctx());
        self.render_terminal_windows(ui.ctx());
        self.render_chat_windows(ui.ctx());
        self.render_voice_chat_windows(ui.ctx());
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

mod client_map;
mod command_result;
mod event;
mod file_transfer;
mod network;
mod payload;
mod ui;

use self::{
    client_map::ClientMapWindow,
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
    ui::{
        activity_context_menu, apply_admin_theme, cell_label, centered_cell,
        connection_status_pill, empty_state, panel, prune_activity_logs, section_title,
        table_header, timestamped_log, COLOR_BAD, COLOR_BG, COLOR_BORDER, COLOR_GOOD, COLOR_MUTED,
        COLOR_PANEL, COLOR_TEXT, COLOR_WARN, TOOLBAR_CONTROL_HEIGHT,
    },
};
use crate::{
    command_menu, live_control, remote_management, runtime::Config, user_interaction, windowing,
};
use eframe::egui;
use rdl_protocol::{
    audio_udp, now_epoch_ms, AudioSource, ClientInfo, CommandKind, CommandOutputStream,
    FileTransferAction, FileTransferDirection, Message, VideoSource,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::UdpSocket;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const GUI_IDLE_FRAME_INTERVAL_MS: u64 = 250;
const GUI_REALTIME_AUDIO_FRAME_INTERVAL_MS: u64 = 16;
const ADMIN_INPUT_QUEUE_CAPACITY: usize = 8;
const VOICE_AUDIO_OUTBOUND_QUEUE_CAPACITY: usize = 128;
const MAX_GUI_EVENTS_PER_FRAME: usize = 4096;
const MAX_PENDING_AUDIO_MS: u64 = 240;
const MAX_PENDING_AUDIO_FRAMES_PER_SOURCE: usize = 32;
const CLIENT_ONLINE_TOAST_TTL: Duration = Duration::from_secs(5);
const MAX_CLIENT_ONLINE_TOASTS: usize = 4;
const AUDIO_UDP_REGISTER_INTERVAL_MS: u64 = 250;
const AUDIO_UDP_RECV_TIMEOUT_MS: u64 = 20;
const AUDIO_STREAM_REPORT_INTERVAL_MS: u64 = 1_000;

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    run_gui(config)?;
    Ok(())
}

fn run_gui(config: Config) -> eframe::Result {
    disable_macos_automatic_window_tabbing();

    let (input_tx, input_rx) = mpsc::sync_channel(ADMIN_INPUT_QUEUE_CAPACITY);
    let (voice_audio_tx, voice_audio_rx) = mpsc::sync_channel(VOICE_AUDIO_OUTBOUND_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel();
    let ui_event_tx = event_tx.clone();
    let network_config = config.clone();
    let repaint_handle = Arc::new(Mutex::new(None));
    let network_repaint_handle = repaint_handle.clone();
    let ignored_file_transfers = Arc::new(Mutex::new(HashSet::new()));
    let network_ignored_file_transfers = ignored_file_transfers.clone();
    let audio_playback_registry = live_control::audio_listen::AudioPlaybackRegistry::default();
    let network_audio_playback_registry = audio_playback_registry.clone();
    let voice_udp_senders = Arc::new(Mutex::new(HashMap::new()));
    let voice_udp_endpoints = Arc::new(Mutex::new(HashMap::new()));
    let voice_audio_udp_senders = voice_udp_senders.clone();
    let voice_audio_udp_endpoints = voice_udp_endpoints.clone();

    thread::spawn(move || {
        voice_audio_forward_loop(
            voice_audio_rx,
            voice_audio_udp_senders,
            voice_audio_udp_endpoints,
        )
    });

    thread::spawn(move || {
        let event_sink = AdminEventSink::new(
            event_tx,
            Some(network_repaint_handle),
            Some(network_audio_playback_registry),
        );
        if let Err(error) = admin_network_loop(
            network_config,
            input_rx,
            event_sink.clone(),
            network_ignored_file_transfers,
        ) {
            event_sink.send(AdminEvent::Log(format!("network stopped: {error}")));
        }
    });

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([780.0, 520.0]);
    let viewport = match rust_desk_light_assets::app_window_icon() {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };
    let native_options = eframe::NativeOptions {
        viewport,
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
                audio_playback_registry,
                voice_audio_tx,
                voice_udp_senders,
                voice_udp_endpoints,
            )))
        }),
    )
}

fn voice_audio_forward_loop(
    voice_audio_rx: Receiver<user_interaction::voice_chat::OutboundCommand>,
    voice_udp_senders: Arc<Mutex<HashMap<String, AudioUdpSender>>>,
    voice_udp_endpoints: Arc<Mutex<HashMap<String, AudioUdpEndpoint>>>,
) {
    let mut missing_senders = HashMap::<String, (u64, Instant)>::new();
    let mut send_failures = HashMap::<String, (u64, Instant, String)>::new();
    while let Ok(command) = voice_audio_rx.recv() {
        let user_interaction::voice_chat::OutboundCommand::AudioFrame {
            client_id,
            seq,
            sample_rate,
            channels,
            format,
            bytes,
        } = command
        else {
            continue;
        };
        let mut remove_sender = false;
        let endpoint = voice_udp_endpoints
            .lock()
            .ok()
            .and_then(|endpoints| endpoints.get(&client_id).cloned());
        if let Ok(mut senders) = voice_udp_senders.lock() {
            if !senders.contains_key(&client_id) {
                if let Some(endpoint) = endpoint.as_ref() {
                    match AudioUdpSender::connect(endpoint) {
                        Ok(sender) => {
                            debug_log!(
                                "debug event=voice_chat_udp_sender_recovered client={} stream={}",
                                client_id,
                                endpoint.stream_id
                            );
                            senders.insert(client_id.clone(), sender);
                        }
                        Err(error) => {
                            debug_log!(
                                "debug event=voice_chat_udp_sender_recover_failed client={} error={}",
                                client_id, error
                            );
                        }
                    }
                }
            }
            if let Some(sender) = senders.get_mut(&client_id) {
                missing_senders.remove(&client_id);
                if let Err(error) =
                    sender.send_frame(&client_id, seq, sample_rate, channels, &format, &bytes)
                {
                    report_voice_udp_send_failure(&mut send_failures, &client_id, &error);
                    remove_sender = error.kind() != io::ErrorKind::InvalidInput;
                } else {
                    send_failures.remove(&client_id);
                }
            }
            if remove_sender {
                senders.remove(&client_id);
            } else if !senders.contains_key(&client_id) {
                let entry = missing_senders
                    .entry(client_id.clone())
                    .or_insert((0, Instant::now()));
                entry.0 = entry.0.saturating_add(1);
                if entry.1.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
                    debug_log!(
                        "debug event=voice_chat_udp_missing_sender client={} frames={}",
                        client_id,
                        entry.0
                    );
                    entry.1 = Instant::now();
                }
            }
        }
    }
}

fn report_voice_udp_send_failure(
    send_failures: &mut HashMap<String, (u64, Instant, String)>,
    client_id: &str,
    error: &io::Error,
) {
    let error_text = error.to_string();
    let entry = send_failures
        .entry(client_id.to_string())
        .or_insert_with(|| {
            (
                0,
                Instant::now() - Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS),
                error_text.clone(),
            )
        });
    if entry.2 != error_text {
        entry.0 = 0;
        entry.1 = Instant::now() - Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS);
        entry.2 = error_text;
    }
    entry.0 = entry.0.saturating_add(1);
    if entry.1.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
        debug_log!(
            "debug event=voice_chat_udp_send_failed client={} count={} error={}",
            client_id,
            entry.0,
            entry.2
        );
        entry.0 = 0;
        entry.1 = Instant::now();
    }
}

fn audio_udp_receive_loop(
    socket: UdpSocket,
    server_addr: String,
    client_id: String,
    source: AudioSource,
    stream_id: u64,
    stop: Arc<AtomicBool>,
    event_sink: AdminEventSink,
) {
    let mut register_packet = Vec::new();
    let mut unregister_packet = Vec::new();
    audio_udp::encode_register(stream_id, &mut register_packet);
    audio_udp::encode_unregister(stream_id, &mut unregister_packet);
    let mut last_register = Instant::now() - Duration::from_millis(AUDIO_UDP_REGISTER_INTERVAL_MS);
    let mut buf = [0_u8; audio_udp::MAX_PACKET_BYTES];

    while !stop.load(Ordering::Relaxed) {
        if last_register.elapsed() >= Duration::from_millis(AUDIO_UDP_REGISTER_INTERVAL_MS) {
            if let Err(error) = socket.send_to(&register_packet, &server_addr) {
                event_sink.send(AdminEvent::Log(format!(
                    "audio udp register failed: {error}"
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
                    event_sink.send(AdminEvent::AudioFrame {
                        client_id: client_id.clone(),
                        source: source.clone(),
                        seq,
                        sample_rate,
                        channels,
                        format: format.to_string(),
                        bytes: bytes.to_vec(),
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    event_sink.send(AdminEvent::Log(format!(
                        "audio udp packet ignored: {error}"
                    )));
                }
            },
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => {
                event_sink.send(AdminEvent::Log(format!(
                    "audio udp receive failed: {error}"
                )));
                break;
            }
        }
    }

    let _ = socket.send_to(&unregister_packet, server_addr);
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
    voice_audio_tx: SyncSender<user_interaction::voice_chat::OutboundCommand>,
    audio_playback_registry: live_control::audio_listen::AudioPlaybackRegistry,
    connected: bool,
    clients: Vec<ClientRow>,
    client_filter: String,
    client_map_window: ClientMapWindow,
    selected_client_id: Option<String>,
    command_windows: Vec<CommandResultWindow>,
    file_manager_windows: Vec<remote_management::file_manager::FileManagerWindow>,
    desktop_windows: Vec<live_control::remote_desktop::RemoteDesktopWindow>,
    camera_windows: Vec<live_control::camera::CameraWindow>,
    audio_windows: Vec<live_control::audio_listen::AudioListenWindow>,
    audio_udp_sessions: HashMap<String, AudioUdpSession>,
    audio_udp_next_stream_id: u64,
    terminal_windows: Vec<remote_management::remote_terminal::TerminalWindow>,
    chat_windows: Vec<user_interaction::text_chat::ChatWindow>,
    voice_chat_windows: Vec<user_interaction::voice_chat::VoiceChatWindow>,
    interaction_command_windows: Vec<user_interaction::InteractionCommandWindow>,
    session_command_windows: Vec<crate::session::SessionCommandWindow>,
    execute_windows: Vec<crate::execute::ExecuteWindow>,
    voice_udp_sessions: HashMap<String, AudioUdpSession>,
    voice_udp_senders: Arc<Mutex<HashMap<String, AudioUdpSender>>>,
    voice_udp_endpoints: Arc<Mutex<HashMap<String, AudioUdpEndpoint>>>,
    file_transfer_cancel_flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
    log_lines: Vec<String>,
    client_online_toasts: VecDeque<ClientOnlineToast>,
    auth_token_prompt_open: bool,
    auth_token_input: String,
    auth_token_error: String,
}

#[derive(Clone)]
struct ClientRow {
    info: ClientInfo,
    status: ClientStatus,
}

struct ClientOnlineToast {
    title: String,
    detail: String,
    created_at: Instant,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ClientStatus {
    Online,
    Stale,
    Offline,
}

impl ClientStatus {
    fn can_receive_commands(self) -> bool {
        matches!(self, ClientStatus::Online)
    }

    fn label(self) -> &'static str {
        match self {
            ClientStatus::Online => "online",
            ClientStatus::Stale => "stale",
            ClientStatus::Offline => "offline",
        }
    }
}

struct PendingVideoFrame {
    seq: u64,
    source_width: u32,
    source_height: u32,
    image_width: u32,
    image_height: u32,
    format: String,
    bytes: Vec<u8>,
}

struct PendingAudioFrame {
    source: AudioSource,
    seq: u64,
    sample_rate: u32,
    channels: u16,
    format: String,
    bytes: Vec<u8>,
}

struct AudioUdpSession {
    stream_id: u64,
    stop: Arc<AtomicBool>,
}

#[derive(Clone)]
struct AudioUdpEndpoint {
    host: String,
    port: u16,
    stream_id: u64,
}

struct AudioUdpSender {
    socket: UdpSocket,
    stream_id: u64,
    packet: Vec<u8>,
    sent_packets: u64,
    sent_bytes: u64,
    last_report: Instant,
}

impl AudioUdpSender {
    fn connect(endpoint: &AudioUdpEndpoint) -> Result<Self, String> {
        let socket =
            UdpSocket::bind("0.0.0.0:0").map_err(|error| format!("bind udp failed: {error}"))?;
        socket
            .connect(endpoint.addr())
            .map_err(|error| format!("connect udp relay failed: {error}"))?;
        Ok(Self {
            socket,
            stream_id: endpoint.stream_id,
            packet: Vec::with_capacity(audio_udp::MAX_PACKET_BYTES),
            sent_packets: 0,
            sent_bytes: 0,
            last_report: Instant::now(),
        })
    }

    fn send_frame(
        &mut self,
        client_id: &str,
        seq: u64,
        sample_rate: u32,
        channels: u16,
        format: &str,
        bytes: &[u8],
    ) -> io::Result<()> {
        audio_udp::encode_audio(
            self.stream_id,
            seq,
            now_epoch_ms() as u64,
            sample_rate,
            channels,
            format,
            bytes,
            &mut self.packet,
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.socket.send(&self.packet)?;
        self.sent_packets = self.sent_packets.saturating_add(1);
        self.sent_bytes = self.sent_bytes.saturating_add(bytes.len() as u64);
        if self.last_report.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
            debug_log!(
                "debug event=voice_chat_tx client={} transport=udp packets={} bytes={}",
                client_id,
                self.sent_packets,
                self.sent_bytes
            );
            self.last_report = Instant::now();
        }
        Ok(())
    }
}

impl AudioUdpEndpoint {
    fn from_payload(payload: &str) -> Result<Option<Self>, String> {
        if payload_field(payload, "transport").as_deref() != Some("udp") {
            return Ok(None);
        }
        let host = payload_field(payload, "udp_host")
            .ok_or_else(|| "missing audio udp host".to_string())?;
        let port = payload_field(payload, "udp_port")
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| "missing audio udp port".to_string())?;
        let stream_id = payload_field(payload, "udp_stream")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| "missing audio udp stream".to_string())?;
        Ok(Some(Self {
            host,
            port,
            stream_id,
        }))
    }

    fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn push_pending_audio_frame(
    queues: &mut HashMap<(String, u8), VecDeque<PendingAudioFrame>>,
    client_id: String,
    frame: PendingAudioFrame,
) {
    let source_key = audio_source_key(&frame.source);
    let queue = queues.entry((client_id, source_key)).or_default();
    queue.push_back(frame);
    while queue.len() > MAX_PENDING_AUDIO_FRAMES_PER_SOURCE
        || pending_audio_duration_ms(queue) > MAX_PENDING_AUDIO_MS
    {
        if queue.len() <= 1 {
            break;
        }
        let _ = queue.pop_front();
    }
}

fn pending_audio_duration_ms(queue: &VecDeque<PendingAudioFrame>) -> u64 {
    queue.iter().map(pending_audio_frame_duration_ms).sum()
}

fn pending_audio_frame_duration_ms(frame: &PendingAudioFrame) -> u64 {
    let channels = frame.channels.max(1) as usize;
    let sample_rate = frame.sample_rate.max(1) as u64;
    let frames = frame.bytes.len() / 2 / channels;
    ((frames as u64 * 1000) / sample_rate).max(1)
}

fn audio_source_key(source: &AudioSource) -> u8 {
    match source {
        AudioSource::AudioListen => 1,
        AudioSource::VoiceChat => 2,
    }
}

fn client_status_text(ui: &mut egui::Ui, status: ClientStatus) {
    let (text, color) = client_status_display(status);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, color);
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(12.0).color(color).strong())
                .selectable(false)
                .sense(egui::Sense::hover()),
        );
    });
}

fn client_status_display(status: ClientStatus) -> (&'static str, egui::Color32) {
    match status {
        ClientStatus::Online => ("Online", COLOR_GOOD),
        ClientStatus::Stale => ("Stale", COLOR_WARN),
        ClientStatus::Offline => ("Offline", COLOR_BAD),
    }
}

fn client_commands_disabled_text(status: ClientStatus) -> &'static str {
    match status {
        ClientStatus::Online => "",
        ClientStatus::Stale => "Client stale - commands disabled",
        ClientStatus::Offline => "Client offline - commands disabled",
    }
}

fn overview_metric(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    let value = value.into();
    egui::Frame::default()
        .fill(COLOR_BG)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(match label {
                "Selected" => 170.0,
                "Version" => 112.0,
                _ => 82.0,
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).size(12.0).color(COLOR_MUTED));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(value.clone())
                            .size(13.0)
                            .color(COLOR_TEXT)
                            .strong(),
                    )
                    .selectable(false),
                )
                .on_hover_text(value);
            });
        });
}

fn client_location_label(client: &ClientInfo) -> String {
    client
        .location
        .as_ref()
        .map(|location| {
            let label = location.label.trim();
            if label.is_empty() {
                format!("{:.2}, {:.2}", location.latitude(), location.longitude())
            } else {
                label.to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string())
}

fn client_identity_label(client: &ClientInfo) -> String {
    match (client.hostname.trim(), client.username.trim()) {
        ("", "") => client.id.clone(),
        (hostname, "") => hostname.to_string(),
        ("", username) => username.to_string(),
        (hostname, username) => format!("{hostname} / {username}"),
    }
}

fn client_online_notice(client: &ClientInfo) -> (String, String) {
    let title = format!("{} is online", client_identity_label(client));
    let detail = if client.peer_addr.trim().is_empty() {
        client.id.clone()
    } else {
        format!("{} - {}", client.id, client.peer_addr)
    };
    (title, detail)
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
        audio_playback_registry: live_control::audio_listen::AudioPlaybackRegistry,
        voice_audio_tx: SyncSender<user_interaction::voice_chat::OutboundCommand>,
        voice_udp_senders: Arc<Mutex<HashMap<String, AudioUdpSender>>>,
        voice_udp_endpoints: Arc<Mutex<HashMap<String, AudioUdpEndpoint>>>,
    ) -> Self {
        apply_admin_theme(&cc.egui_ctx);
        if let Ok(mut handle) = repaint_handle.lock() {
            *handle = Some(cc.egui_ctx.clone());
        }
        let initial_auth_token = config.auth_token.clone();
        let auth_token_prompt_open = initial_auth_token.trim().is_empty();
        Self {
            config,
            input_tx,
            event_rx,
            event_tx,
            repaint_handle,
            audio_playback_registry,
            connected: false,
            clients: Vec::new(),
            client_filter: String::new(),
            client_map_window: ClientMapWindow::new(),
            selected_client_id: None,
            command_windows: Vec::new(),
            file_manager_windows: Vec::new(),
            desktop_windows: Vec::new(),
            camera_windows: Vec::new(),
            audio_windows: Vec::new(),
            audio_udp_sessions: HashMap::new(),
            audio_udp_next_stream_id: now_epoch_ms() as u64,
            terminal_windows: Vec::new(),
            chat_windows: Vec::new(),
            voice_chat_windows: Vec::new(),
            voice_audio_tx,
            interaction_command_windows: Vec::new(),
            session_command_windows: Vec::new(),
            execute_windows: Vec::new(),
            voice_udp_sessions: HashMap::new(),
            voice_udp_senders,
            voice_udp_endpoints,
            file_transfer_cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            ignored_file_transfers,
            log_lines: vec![timestamped_log(format!(
                "admin gui started version={}",
                rdl_version::display_version()
            ))],
            client_online_toasts: VecDeque::new(),
            auth_token_prompt_open,
            auth_token_input: initial_auth_token,
            auth_token_error: String::new(),
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut changed = false;
        let mut latest_desktop_frames = HashMap::<String, String>::new();
        let mut latest_camera_frames = HashMap::<String, String>::new();
        let mut latest_desktop_video_frames = HashMap::<String, PendingVideoFrame>::new();
        let mut latest_camera_video_frames = HashMap::<String, PendingVideoFrame>::new();
        let mut pending_audio_frames = HashMap::<(String, u8), VecDeque<PendingAudioFrame>>::new();
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
                    self.stop_all_audio_udp_sessions();
                    self.stop_all_voice_udp_sessions();
                    self.clear_voice_udp_senders();
                    for client in &mut self.clients {
                        client.status = ClientStatus::Offline;
                    }
                }
                AdminEvent::AuthTokenRequired => {
                    self.auth_token_prompt_open = true;
                    self.auth_token_error = "Server requires an auth token.".to_string();
                    self.auth_token_input.clear();
                }
                AdminEvent::AuthTokenRejected(detail) => {
                    self.auth_token_prompt_open = true;
                    self.auth_token_error = detail;
                    self.auth_token_input.clear();
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
                } => {
                    let frame = PendingVideoFrame {
                        seq,
                        source_width,
                        source_height,
                        image_width,
                        image_height,
                        format,
                        bytes,
                    };
                    match source {
                        VideoSource::RemoteDesktop => {
                            latest_desktop_video_frames.insert(client_id, frame);
                        }
                        VideoSource::Camera => {
                            latest_camera_video_frames.insert(client_id, frame);
                        }
                    }
                }
                AdminEvent::AudioFrame {
                    client_id,
                    source,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                } => push_pending_audio_frame(
                    &mut pending_audio_frames,
                    client_id,
                    PendingAudioFrame {
                        source,
                        seq,
                        sample_rate,
                        channels,
                        format,
                        bytes,
                    },
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
                            debug_log!(
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
        for (client_id, frame) in latest_desktop_video_frames {
            self.spawn_video_frame_decode(client_id, VideoSource::RemoteDesktop, frame);
        }
        for (client_id, frame) in latest_camera_video_frames {
            self.spawn_video_frame_decode(client_id, VideoSource::Camera, frame);
        }
        for ((client_id, _), frames) in pending_audio_frames {
            for frame in frames {
                self.handle_pending_audio_frame(&client_id, frame);
            }
        }
        changed
    }

    fn render_auth_token_prompt(&mut self, ctx: &egui::Context) {
        if !self.auth_token_prompt_open {
            return;
        }

        egui::Window::new("Auth Token")
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Enter the shared server token to connect.")
                        .color(COLOR_TEXT),
                );
                ui.add_space(8.0);
                let response = ui.add_sized(
                    [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
                    egui::TextEdit::singleline(&mut self.auth_token_input)
                        .password(true)
                        .hint_text("Auth token")
                        .vertical_align(egui::Align::Center),
                );
                if !self.auth_token_error.is_empty() {
                    ui.label(
                        egui::RichText::new(&self.auth_token_error)
                            .size(12.0)
                            .color(COLOR_BAD),
                    );
                }
                ui.add_space(8.0);
                let submit = ui.button("Save and reconnect").clicked()
                    || (response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                if submit {
                    let token = self.auth_token_input.trim().to_string();
                    if token.is_empty() {
                        self.auth_token_error = "Token cannot be empty.".to_string();
                    } else {
                        match self.config.save_auth_token(&token) {
                            Ok(()) => {
                                self.auth_token_prompt_open = false;
                                self.auth_token_error.clear();
                                self.push_log("saved auth token");
                                let _ = self.input_tx.send(AdminInput::Reconnect {
                                    reason: "auth token updated".to_string(),
                                });
                            }
                            Err(error) => {
                                self.auth_token_error = format!("Save failed: {error}");
                            }
                        }
                    }
                }
            });
    }

    fn handle_pending_audio_frame(&mut self, client_id: &str, frame: PendingAudioFrame) {
        match frame.source {
            AudioSource::AudioListen => match live_control::audio_listen::decode_audio_frame(
                frame.seq,
                frame.sample_rate,
                frame.channels,
                frame.format,
                frame.bytes,
            ) {
                Ok(frame) => {
                    live_control::audio_listen::handle_audio_frame(
                        &mut self.audio_windows,
                        client_id,
                        frame,
                    );
                }
                Err(message) => self.handle_audio_ack(
                    client_id,
                    true,
                    format!("audio_listen_error\nmessage={message}"),
                ),
            },
            AudioSource::VoiceChat => match user_interaction::voice_chat::decode_audio_frame(
                frame.seq,
                frame.sample_rate,
                frame.channels,
                frame.format,
                frame.bytes,
            ) {
                Ok(frame) => {
                    user_interaction::voice_chat::handle_audio_frame(
                        &mut self.voice_chat_windows,
                        client_id,
                        frame,
                    );
                }
                Err(message) => self.handle_voice_chat_ack(
                    client_id,
                    true,
                    format!("voice_chat_error\nmessage={message}"),
                ),
            },
        }
    }

    fn realtime_audio_active(&self) -> bool {
        live_control::audio_listen::has_active_windows(&self.audio_windows)
            || user_interaction::voice_chat::has_active_windows(&self.voice_chat_windows)
    }

    fn ignore_file_transfer(&self, client_id: &str, transfer_id: u64) {
        if let Ok(mut ignored) = self.ignored_file_transfers.lock() {
            ignored.insert((client_id.to_string(), transfer_id));
        }
        debug_log!(
            "debug event=admin_file_transfer_ignore_add client={client_id} id={transfer_id}"
        );
    }

    fn unignore_file_transfer(&self, client_id: &str, transfer_id: u64) {
        if let Ok(mut ignored) = self.ignored_file_transfers.lock() {
            ignored.remove(&(client_id.to_string(), transfer_id));
        }
        debug_log!(
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
        let sink = AdminEventSink::new(
            self.event_tx.clone(),
            Some(self.repaint_handle.clone()),
            None,
        );
        thread::spawn(move || {
            let result = live_control::remote_desktop::decode_frame_payload(&payload);
            sink.send(AdminEvent::DecodedDesktopFrame { client_id, result });
        });
    }

    fn spawn_camera_frame_decode(&self, client_id: String, payload: String) {
        let sink = AdminEventSink::new(
            self.event_tx.clone(),
            Some(self.repaint_handle.clone()),
            None,
        );
        thread::spawn(move || {
            let result = live_control::camera::decode_frame_payload(&payload);
            sink.send(AdminEvent::DecodedCameraFrame { client_id, result });
        });
    }

    fn spawn_video_frame_decode(
        &self,
        client_id: String,
        source: VideoSource,
        frame: PendingVideoFrame,
    ) {
        let sink = AdminEventSink::new(
            self.event_tx.clone(),
            Some(self.repaint_handle.clone()),
            None,
        );
        thread::spawn(move || match source {
            VideoSource::RemoteDesktop => {
                let result = live_control::remote_desktop::decode_video_frame(
                    frame.seq,
                    frame.source_width,
                    frame.source_height,
                    frame.image_width,
                    frame.image_height,
                    frame.format,
                    frame.bytes,
                );
                sink.send(AdminEvent::DecodedDesktopFrame { client_id, result });
            }
            VideoSource::Camera => {
                let result = live_control::camera::decode_video_frame(
                    frame.seq,
                    frame.image_width,
                    frame.image_height,
                    frame.format,
                    frame.bytes,
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

    fn push_client_online_toast(&mut self, title: String, detail: String) {
        self.push_log(format!("client online: {title}"));
        self.client_online_toasts.push_back(ClientOnlineToast {
            title,
            detail,
            created_at: Instant::now(),
        });
        while self.client_online_toasts.len() > MAX_CLIENT_ONLINE_TOASTS {
            self.client_online_toasts.pop_front();
        }
    }

    fn merge_clients(&mut self, clients: Vec<ClientInfo>) {
        let online_ids: HashSet<String> = clients.iter().map(|client| client.id.clone()).collect();
        let mut online_notices = Vec::new();
        for client in clients {
            if let Some(existing) = self.clients.iter_mut().find(|row| row.info.id == client.id) {
                let was_online = existing.status == ClientStatus::Online;
                existing.info = client;
                existing.status = ClientStatus::Online;
                if !was_online {
                    online_notices.push(client_online_notice(&existing.info));
                }
            } else {
                online_notices.push(client_online_notice(&client));
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

        for (title, detail) in online_notices {
            self.push_client_online_toast(title, detail);
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
                    || client_location_label(&row.info)
                        .to_ascii_lowercase()
                        .contains(&filter)
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

    fn client_status_for(&self, client_id: &str) -> Option<ClientStatus> {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| row.status)
    }

    fn selected_client_label(&self) -> String {
        let Some(selected_id) = self.selected_client_id.as_deref() else {
            return "None".to_string();
        };

        self.clients
            .iter()
            .find(|row| row.info.id == selected_id)
            .map(|row| client_identity_label(&row.info))
            .unwrap_or_else(|| selected_id.to_string())
    }

    fn send_command(&mut self, client_id: &str, command: CommandKind) {
        if !self
            .client_status_for(client_id)
            .map(ClientStatus::can_receive_commands)
            .unwrap_or(false)
        {
            self.push_log(format!(
                "blocked command={} to {}: client is offline",
                command.as_str(),
                client_id
            ));
            return;
        }

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
            self.audio_playback_registry.clone(),
        );
    }

    fn next_audio_udp_stream_id(&mut self) -> u64 {
        let stream_id = self.audio_udp_next_stream_id.max(1);
        self.audio_udp_next_stream_id = self.audio_udp_next_stream_id.saturating_add(1);
        stream_id
    }

    fn start_audio_udp_receive_session(
        &mut self,
        client_id: &str,
        source: AudioSource,
    ) -> Option<AudioUdpSession> {
        let server_addr = format!("{}:{}", self.config.ip, self.config.port);
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(socket) => socket,
            Err(error) => {
                self.push_log(format!("audio udp bind failed: {error}"));
                return None;
            }
        };
        if let Err(error) =
            socket.set_read_timeout(Some(Duration::from_millis(AUDIO_UDP_RECV_TIMEOUT_MS)))
        {
            self.push_log(format!("audio udp timeout setup failed: {error}"));
            return None;
        }
        let stream_id = self.next_audio_udp_stream_id();
        let stop = Arc::new(AtomicBool::new(false));
        let event_sink = AdminEventSink::new(
            self.event_tx.clone(),
            Some(self.repaint_handle.clone()),
            Some(self.audio_playback_registry.clone()),
        );
        let worker_stop = stop.clone();
        let worker_client_id = client_id.to_string();
        let worker_server_addr = server_addr.clone();
        let worker_source = source.clone();
        thread::spawn(move || {
            audio_udp_receive_loop(
                socket,
                worker_server_addr,
                worker_client_id,
                worker_source,
                stream_id,
                worker_stop,
                event_sink,
            );
        });
        self.push_log(format!(
            "audio udp session started client={client_id} source={} stream={stream_id} relay={server_addr}",
            source.as_str()
        ));
        Some(AudioUdpSession { stream_id, stop })
    }

    fn start_audio_udp_session(&mut self, client_id: &str) -> Option<u64> {
        self.stop_audio_udp_session(client_id);
        let session = self.start_audio_udp_receive_session(client_id, AudioSource::AudioListen)?;
        let stream_id = session.stream_id;
        self.audio_udp_sessions
            .insert(client_id.to_string(), session);
        Some(stream_id)
    }

    fn stop_audio_udp_session(&mut self, client_id: &str) {
        if let Some(session) = self.audio_udp_sessions.remove(client_id) {
            session.stop.store(true, Ordering::Relaxed);
            self.push_log(format!(
                "audio udp session stopped client={client_id} stream={}",
                session.stream_id
            ));
        }
    }

    fn stop_all_audio_udp_sessions(&mut self) {
        let client_ids: Vec<String> = self.audio_udp_sessions.keys().cloned().collect();
        for client_id in client_ids {
            self.stop_audio_udp_session(&client_id);
        }
    }

    fn start_voice_udp_session(&mut self, client_id: &str) -> Option<u64> {
        self.stop_voice_udp_session(client_id);
        let session = self.start_audio_udp_receive_session(client_id, AudioSource::VoiceChat)?;
        let stream_id = session.stream_id;
        self.voice_udp_sessions
            .insert(client_id.to_string(), session);
        Some(stream_id)
    }

    fn stop_voice_udp_session(&mut self, client_id: &str) {
        if let Some(session) = self.voice_udp_sessions.remove(client_id) {
            session.stop.store(true, Ordering::Relaxed);
            self.push_log(format!(
                "voice udp session stopped client={client_id} stream={}",
                session.stream_id
            ));
        }
    }

    fn stop_all_voice_udp_sessions(&mut self) {
        let client_ids: Vec<String> = self.voice_udp_sessions.keys().cloned().collect();
        for client_id in client_ids {
            self.stop_voice_udp_session(&client_id);
        }
    }

    fn set_voice_udp_sender(&mut self, client_id: &str, detail: &str) -> Result<(), String> {
        let endpoint = AudioUdpEndpoint::from_payload(detail)?
            .ok_or_else(|| "voice chat udp transport unavailable".to_string())?;
        self.set_voice_udp_sender_endpoint(client_id, &endpoint)
    }

    fn set_voice_udp_sender_endpoint(
        &mut self,
        client_id: &str,
        endpoint: &AudioUdpEndpoint,
    ) -> Result<(), String> {
        let sender = AudioUdpSender::connect(endpoint)?;
        self.voice_udp_endpoints
            .lock()
            .map_err(|_| "voice udp endpoint map is poisoned".to_string())?
            .insert(client_id.to_string(), endpoint.clone());
        self.voice_udp_senders
            .lock()
            .map_err(|_| "voice udp sender map is poisoned".to_string())?
            .insert(client_id.to_string(), sender);
        self.push_log(format!(
            "voice udp sender ready client={client_id} stream={} relay={}:{}",
            endpoint.stream_id, endpoint.host, endpoint.port
        ));
        Ok(())
    }

    fn remove_voice_udp_sender(&mut self, client_id: &str) {
        if let Ok(mut senders) = self.voice_udp_senders.lock() {
            senders.remove(client_id);
        }
        if let Ok(mut endpoints) = self.voice_udp_endpoints.lock() {
            endpoints.remove(client_id);
        }
    }

    fn clear_voice_udp_senders(&mut self) {
        if let Ok(mut senders) = self.voice_udp_senders.lock() {
            senders.clear();
        }
        if let Ok(mut endpoints) = self.voice_udp_endpoints.lock() {
            endpoints.clear();
        }
    }

    fn open_session_command_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        crate::session::open_window(
            &mut self.session_command_windows,
            client_id,
            hostname,
            username,
            command,
            &self.config.ip,
            self.config.port,
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
            user_interaction::handle_ack(
                &mut self.interaction_command_windows,
                &client_id,
                &command,
                accepted,
                &detail,
            );
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
        if user_interaction::handle_ack(
            &mut self.interaction_command_windows,
            &client_id,
            &command,
            accepted,
            &detail,
        ) {
            self.push_log(format!(
                "result client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                sanitize_log_value(&detail)
            ));
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
        let mut accepted = accepted;
        let mut detail = detail;
        if accepted && detail.starts_with("voice_chat_accepted") {
            if let Err(error) = self.set_voice_udp_sender(client_id, &detail) {
                self.remove_voice_udp_sender(client_id);
                self.stop_voice_udp_session(client_id);
                accepted = false;
                detail = format!("voice_chat_error\nmessage={error}");
            }
        } else if !accepted
            || detail.starts_with("voice_chat_ended")
            || detail.starts_with("voice_chat_error")
            || detail.starts_with("voice_chat_declined")
        {
            self.remove_voice_udp_sender(client_id);
            self.stop_voice_udp_session(client_id);
        }
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::voice_chat::handle_ack(
            &mut self.voice_chat_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
            self.voice_audio_tx.clone(),
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
                if ui.button("🌐 Client Map").clicked() {
                    self.client_map_window.open();
                }
                if let Some(client_id) = self.selected_client_id.clone() {
                    ui.separator();
                    let status = self
                        .client_status_for(&client_id)
                        .unwrap_or(ClientStatus::Offline);
                    if status.can_receive_commands() {
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
                        let (_, color) = client_status_display(status);
                        ui.label(
                            egui::RichText::new(client_commands_disabled_text(status))
                                .size(12.0)
                                .color(color)
                                .strong(),
                        )
                        .on_hover_text(
                            "Remote commands become available when the client reconnects",
                        );
                    }
                } else {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Select a client for actions")
                            .size(12.0)
                            .color(COLOR_MUTED),
                    );
                }
                #[cfg(debug_assertions)]
                {
                    ui.separator();
                    ui.menu_button("Debug", |ui| {
                        if ui.button("输出中文日志").clicked() {
                            self.push_log("中文日志测试：菜单和日志应正常显示，不应乱码。");
                            ui.close();
                        }
                    });
                }
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
            ui.horizontal_wrapped(|ui| {
                overview_metric(ui, "Online", self.online_client_count().to_string());
                overview_metric(ui, "Known", self.clients.len().to_string());
                overview_metric(ui, "Selected", self.selected_client_label());
                overview_metric(ui, "Version", rdl_version::display_version());
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
            ui.add_space(6.0);
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
                ui.add_sized(
                    [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
                    egui::TextEdit::singleline(&mut self.client_filter)
                        .hint_text("Search by id, fingerprint, host, user, OS, or location")
                        .vertical_align(egui::Align::Center),
                );
            });
            ui.add_space(8.0);

            let clients = self.filtered_clients();
            if clients.is_empty() {
                empty_state(ui);
                return;
            }

            let ctx = ui.ctx().clone();
            egui_extras::TableBuilder::new(ui)
                .id_salt("admin_clients_table_resizable")
                .striped(false)
                .sense(egui::Sense::click())
                .resizable(true)
                .column(egui_extras::Column::initial(86.0).at_least(72.0).clip(true))
                .column(
                    egui_extras::Column::initial(190.0)
                        .at_least(140.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(150.0)
                        .at_least(120.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(180.0)
                        .at_least(120.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(150.0)
                        .at_least(120.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(100.0)
                        .at_least(80.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(220.0)
                        .at_least(130.0)
                        .clip(true),
                )
                .header(24.0, |mut header| {
                    header.col(|ui| table_header(ui, "Status"));
                    header.col(|ui| table_header(ui, "Client ID"));
                    header.col(|ui| table_header(ui, "IP"));
                    header.col(|ui| table_header(ui, "Location"));
                    header.col(|ui| table_header(ui, "Host"));
                    header.col(|ui| table_header(ui, "User"));
                    header.col(|ui| table_header(ui, "OS Version"));
                })
                .body(|body| {
                    body.rows(30.0, clients.len(), |mut row| {
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
                                cell_label(ui, &client.id);
                            });
                        });
                        row.col(|ui| {
                            centered_cell(ui, |ui| {
                                cell_label(ui, &client.peer_addr);
                            });
                        });
                        row.col(|ui| {
                            centered_cell(ui, |ui| {
                                cell_label(ui, client_location_label(client));
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
                        let response = row.response();
                        if response.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            self.selected_client_id = Some(client.id.clone());
                        }
                        response.context_menu(|ui| {
                            if row_data.status.can_receive_commands() {
                                command_menu::render_context_menu(
                                    ui,
                                    &client.id,
                                    client.gui_available,
                                    &mut |client_id, command| {
                                        self.send_command(client_id, command);
                                    },
                                );
                            } else {
                                command_menu::render_unavailable_client_menu(
                                    ui,
                                    &client.id,
                                    row_data.status.label(),
                                );
                            }
                        });
                    });
                });
        });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Activity");
            ui.add_space(6.0);
            let output = egui::ScrollArea::vertical()
                .id_salt("admin_activity_scroll_area")
                .stick_to_bottom(true)
                .max_height(120.0)
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

    fn render_client_online_toasts(&mut self, ctx: &egui::Context) {
        self.client_online_toasts
            .retain(|toast| toast.created_at.elapsed() < CLIENT_ONLINE_TOAST_TTL);
        if self.client_online_toasts.is_empty() {
            return;
        }

        let mut dismiss_index = None;
        egui::Area::new(egui::Id::new("admin_client_online_toasts"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-18.0, 18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                for index in (0..self.client_online_toasts.len()).rev() {
                    let toast = &self.client_online_toasts[index];
                    let title = toast.title.clone();
                    let detail = toast.detail.clone();
                    egui::Frame::default()
                        .fill(COLOR_PANEL)
                        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(8.0, 8.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), 4.0, COLOR_GOOD);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(title)
                                            .size(13.0)
                                            .color(COLOR_TEXT)
                                            .strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(detail).size(12.0).color(COLOR_MUTED),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Min),
                                    |ui| {
                                        if ui.small_button("x").on_hover_text("Dismiss").clicked() {
                                            dismiss_index = Some(index);
                                        }
                                    },
                                );
                            });
                        });
                }
            });

        if let Some(index) = dismiss_index {
            self.client_online_toasts.remove(index);
        }
        ctx.request_repaint_after(Duration::from_millis(250));
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
                user_interaction::voice_chat::OutboundCommand::Command {
                    client_id,
                    mut payload,
                } => {
                    if payload_field(&payload, "action").as_deref() == Some("invite") {
                        let Some(stream_id) = self.start_voice_udp_session(&client_id) else {
                            self.handle_voice_chat_ack(
                                &client_id,
                                false,
                                "voice_chat_error\nmessage=voice udp setup failed".to_string(),
                            );
                            continue;
                        };
                        let client_receive_stream_id = self.next_audio_udp_stream_id();
                        let client_receive_endpoint = AudioUdpEndpoint {
                            host: self.config.ip.clone(),
                            port: self.config.port,
                            stream_id: client_receive_stream_id,
                        };
                        if let Err(error) =
                            self.set_voice_udp_sender_endpoint(&client_id, &client_receive_endpoint)
                        {
                            self.stop_voice_udp_session(&client_id);
                            self.handle_voice_chat_ack(
                                &client_id,
                                false,
                                format!("voice_chat_error\nmessage={error}"),
                            );
                            continue;
                        }
                        payload.push_str(&format!(
                            "\ntransport=udp\nudp_host={}\nudp_port={}\nudp_stream={stream_id}\nudp_return_stream={client_receive_stream_id}",
                            self.config.ip, self.config.port
                        ));
                    }
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
                    match payload_field(&payload, "action").as_deref() {
                        Some("stop") | Some("end") => {
                            self.remove_voice_udp_sender(&client_id);
                            self.stop_voice_udp_session(&client_id);
                        }
                        _ => {}
                    }
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
                    let mut remove_sender = false;
                    if let Ok(mut senders) = self.voice_udp_senders.lock() {
                        if let Some(sender) = senders.get_mut(&client_id) {
                            if sender
                                .send_frame(&client_id, seq, sample_rate, channels, &format, &bytes)
                                .is_err()
                            {
                                remove_sender = true;
                            }
                        }
                        if remove_sender {
                            senders.remove(&client_id);
                        }
                    }
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
                debug_log!(
                    "debug event=admin_file_manager_send client={} action={} path={}",
                    outbound.client_id,
                    action,
                    path
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
                debug_log!(
                    "debug event=admin_file_transfer_request client={} id={} direction=upload local_path={} remote_path={}",
                    client_id, transfer_id, local_path, remote_path
                );
                let cancel_flag = Arc::new(AtomicBool::new(false));
                if let Ok(mut flags) = self.file_transfer_cancel_flags.lock() {
                    flags.insert(transfer_id, cancel_flag.clone());
                }
                let input_tx = self.input_tx.clone();
                let flags = self.file_transfer_cancel_flags.clone();
                let sink = AdminEventSink::new(
                    self.event_tx.clone(),
                    Some(self.repaint_handle.clone()),
                    None,
                );
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
                debug_log!(
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
                debug_log!(
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
        for mut outbound in live_control::audio_listen::render_windows(ctx, &mut self.audio_windows)
        {
            match payload_field(&outbound.payload, "action").as_deref() {
                Some("start") => {
                    if let Some(stream_id) = self.start_audio_udp_session(&outbound.client_id) {
                        outbound.payload.push_str(&format!(
                            "\ntransport=udp\nudp_host={}\nudp_port={}\nudp_stream={stream_id}",
                            self.config.ip, self.config.port
                        ));
                    }
                }
                Some("stop") => self.stop_audio_udp_session(&outbound.client_id),
                _ => {}
            }
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
        let horizontal_margin = if ui.available_width() < 880.0 { 10 } else { 18 };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::default()
                    .inner_margin(egui::Margin::symmetric(horizontal_margin, 18))
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            self.render_menu_bar(ui);
                            ui.add_space(12.0);
                            self.render_overview(ui);
                            ui.add_space(12.0);
                            self.render_clients(ui);
                            ui.add_space(12.0);
                            self.render_activity(ui);
                        });
                    });
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
        self.render_auth_token_prompt(ui.ctx());
        self.client_map_window.render(
            ui.ctx(),
            &self.clients,
            &mut self.selected_client_id,
            &mut self.client_filter,
        );
        self.render_client_online_toasts(ui.ctx());

        if changed {
            ui.ctx().request_repaint();
        } else {
            let interval_ms = if self.realtime_audio_active() {
                GUI_REALTIME_AUDIO_FRAME_INTERVAL_MS
            } else {
                GUI_IDLE_FRAME_INTERVAL_MS
            };
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(interval_ms));
        }
    }
}

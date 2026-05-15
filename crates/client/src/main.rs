mod commands;
mod live_control;
mod remote_management;
mod support;
mod system_info;
mod user_interaction;

use eframe::egui;
use rdl_protocol::{
    write_envelope_with_token, CommandKind, EnvelopeDecoder, Message, Role, DEFAULT_SERVER_IP,
    DEFAULT_SERVER_PORT,
};
use std::fs;
use std::io;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

const INITIAL_RECONNECT_DELAY_MS: u64 = 500;
const MAX_RECONNECT_DELAY_MS: u64 = 8_000;
const NETWORK_POLL_INTERVAL_MS: u64 = 16;
const GUI_FRAME_INTERVAL_MS: u64 = 16;
const NETWORK_IDLE_SLEEP_MS: u64 = 4;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env();
    if gui_available() {
        run_gui(config)?;
    } else {
        run_terminal(config)?;
    }
    Ok(())
}

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

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 520.0])
            .with_min_inner_size([680.0, 440.0]),
        ..Default::default()
    };

    eframe::run_native(
        "rust-desk-light client",
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

#[cfg(target_os = "macos")]
fn disable_macos_automatic_window_tabbing() {
    if let Some(main_thread) = objc2_foundation::MainThreadMarker::new() {
        objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, main_thread);
    }
}

#[cfg(not(target_os = "macos"))]
fn disable_macos_automatic_window_tabbing() {}

fn run_terminal(config: Config) -> io::Result<()> {
    let identity = load_client_identity();
    let (event_tx, event_rx) = mpsc::channel();
    let (_input_tx, input_rx) = mpsc::channel();
    println!(
        "rust-desk-light client terminal fallback, server={}:{}",
        config.ip, config.port
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
    loop {
        match client_connection_once(
            config.clone(),
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
    let (out_tx, out_rx) = mpsc::channel();
    thread::spawn(move || client_writer_loop(writer, out_rx));
    queue_message(
        &out_tx,
        "",
        Message::Hello {
            role: Role::Client,
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
                if command == CommandKind::TextChat && gui_mode {
                    event_sink.send(ClientEvent::ChatMessage {
                        text: payload.clone(),
                    });
                }
                let worker_tx = out_tx.clone();
                let worker_token = session_token.clone();
                thread::spawn(move || {
                    let detail = commands::handle_command(&command, &payload, gui_mode);
                    let _ = queue_message(
                        &worker_tx,
                        &worker_token,
                        Message::CommandAck {
                            client_id: target_id,
                            command,
                            accepted: true,
                            detail,
                        },
                    );
                });
            }
            Message::DesktopControl { target_id, payload } => {
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
                let worker_tx = out_tx.clone();
                let worker_token = session_token.clone();
                thread::spawn(move || {
                    let should_reply = !desktop_payload_is_move(&payload);
                    let payload =
                        crate::live_control::handle(&CommandKind::RemoteDesktop, &payload);
                    if should_reply {
                        let _ = queue_message(
                            &worker_tx,
                            &worker_token,
                            Message::DesktopFrame {
                                client_id: target_id,
                                payload,
                            },
                        );
                    }
                });
            }
            Message::Ping => queue_message(&out_tx, &session_token, Message::Pong)?,
            other => {
                event_sink.send(ClientEvent::Log(format!("server: {other:?}")));
            }
        }
    }

    Ok(())
}

struct ClientApp {
    config: Config,
    identity: LocalIdentity,
    input_tx: Sender<ClientInput>,
    event_rx: Receiver<ClientEvent>,
    connected: bool,
    log_lines: Vec<String>,
    chat_window: Option<user_interaction::text_chat::ChatWindow>,
}

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
            log_lines: vec!["client gui started".to_string()],
            chat_window: None,
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.event_rx.try_recv() {
            changed = true;
            match event {
                ClientEvent::Connected => {
                    self.connected = true;
                    self.log_lines.push("connected to server".to_string());
                }
                ClientEvent::Disconnected => {
                    self.connected = false;
                    self.log_lines.push("disconnected from server".to_string());
                }
                ClientEvent::Command { command, payload } => {
                    self.log_lines.push(format!(
                        "received command={} payload={payload}",
                        command.as_str()
                    ));
                }
                ClientEvent::ChatMessage { text } => {
                    user_interaction::text_chat::receive_admin_message(&mut self.chat_window, text);
                }
                ClientEvent::Log(line) => self.log_lines.push(line),
            }
            if self.log_lines.len() > 200 {
                self.log_lines.remove(0);
            }
        }
        changed
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
                    egui::RichText::new("Client Agent")
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

    fn render_activity(&self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Activity");
            ui.add_space(10.0);
            egui::ScrollArea::vertical()
                .id_salt("client_activity_scroll_area")
                .stick_to_bottom(true)
                .max_height(220.0)
                .show(ui, |ui| {
                    for line in &self.log_lines {
                        ui.monospace(egui::RichText::new(line).size(12.0).color(COLOR_MUTED));
                    }
                });
        });
    }
}

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
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);

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

fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).color(COLOR_MUTED));
    ui.label(egui::RichText::new(value).color(COLOR_TEXT).strong());
    ui.end_row();
}

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
    Log(String),
}

#[derive(Clone)]
struct ClientEventSink {
    tx: Sender<ClientEvent>,
    repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
}

impl ClientEventSink {
    fn new(
        tx: Sender<ClientEvent>,
        repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
    ) -> Self {
        Self { tx, repaint_handle }
    }

    fn send(&self, event: ClientEvent) {
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

enum ClientInput {
    ChatReply { text: String },
}

struct ClientOutbound {
    session_token: String,
    message: Message,
}

struct DesktopStreamState {
    running: AtomicBool,
    generation: std::sync::atomic::AtomicU64,
}

fn client_writer_loop(mut writer: TcpStream, out_rx: Receiver<ClientOutbound>) {
    let mut next_message_id = 1u64;
    for outbound in out_rx {
        let fallback = command_ack_send_failure(&outbound.message);
        if let Err(error) = send(
            &mut writer,
            &mut next_message_id,
            &outbound.session_token,
            outbound.message,
        ) {
            if let Some(message) = fallback {
                eprintln!("client write failed, sending command error ack: {error}");
                if let Err(fallback_error) = send(
                    &mut writer,
                    &mut next_message_id,
                    &outbound.session_token,
                    message(error),
                ) {
                    eprintln!("client fallback write failed: {fallback_error}");
                    break;
                }
                continue;
            }
            eprintln!("client write failed: {error}");
            break;
        }
    }
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
    out_tx: Sender<ClientOutbound>,
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

fn quality_fps(value: &str) -> u64 {
    match value {
        "low" => 10,
        "high" => 2,
        _ => 5,
    }
}

fn queue_message(
    out_tx: &Sender<ClientOutbound>,
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

#[derive(Clone)]
struct LocalIdentity {
    id: String,
    fingerprint: String,
}

fn load_client_identity() -> LocalIdentity {
    let path = identity_file_path("client.identity");
    if let Ok(text) = fs::read_to_string(&path) {
        let mut id = String::new();
        let mut fingerprint = String::new();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("id=") {
                id = value.trim().to_string();
            }
            if let Some(value) = line.strip_prefix("fingerprint=") {
                fingerprint = value.trim().to_string();
            }
        }
        if !id.is_empty() && !fingerprint.is_empty() {
            return LocalIdentity { id, fingerprint };
        }
    }

    let seed = format!(
        "{}|{}|{}|{}|{}",
        hostname(),
        username(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        rdl_protocol::now_epoch_ms()
    );
    let id = format!("client-{:016x}", simple_hash(&seed));
    let fingerprint = fingerprint_for(&id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, format!("id={id}\nfingerprint={fingerprint}\n"));
    LocalIdentity { id, fingerprint }
}

fn fingerprint_for(id: &str) -> String {
    format!(
        "fp-{:016x}",
        simple_hash(&format!(
            "{}|{}|{}|{}|{}",
            id,
            hostname(),
            username(),
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    )
}

fn identity_file_path(file_name: &str) -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata)
            .join("rust-desk-light")
            .join(file_name);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("rust-desk-light")
            .join(file_name);
    }
    PathBuf::from(file_name)
}

fn simple_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn gui_available() -> bool {
    if std::env::var_os("RDL_FORCE_TERMINAL").is_some() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| {
            std::fs::read_to_string("/etc/hostname")
                .map(|value| value.trim().to_string())
                .map_err(|error| error.to_string())
        })
        .unwrap_or_else(|_| "unknown-host".to_string())
}

fn os_label() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
            if let Some(value) = text
                .lines()
                .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            {
                return value.trim_matches('"').to_string();
            }
        }
    }
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

fn username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string())
}

#[derive(Clone)]
struct Config {
    ip: String,
    port: u16,
}

impl Config {
    fn from_env() -> Self {
        let mut ip = DEFAULT_SERVER_IP.to_string();
        let mut port = DEFAULT_SERVER_PORT;
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--ip" => {
                    if let Some(value) = args.next() {
                        ip = value;
                    }
                }
                "--port" => {
                    if let Some(value) = args.next() {
                        if let Ok(value) = value.parse() {
                            port = value;
                        }
                    }
                }
                "--help" | "-h" => {
                    println!("Usage: rdl-client [--ip 127.0.0.1] [--port 21115]");
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        Self { ip, port }
    }
}

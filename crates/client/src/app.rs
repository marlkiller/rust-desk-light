use crate::{
    commands,
    runtime::{
        gui_available, hostname, load_client_identity, os_label, username, Config, LocalIdentity,
    },
    user_interaction,
};
use eframe::egui;
use rdl_protocol::{
    write_envelope_with_token, CommandKind, EnvelopeDecoder, Message, Role, VideoSource,
};
use std::io;
use std::net::TcpStream;
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

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
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
    let camera_stream = Arc::new(DesktopStreamState {
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
                    let worker_token = session_token.clone();
                    thread::spawn(move || {
                        video_stream_loop(
                            target_id,
                            source,
                            payload,
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
            log_lines: vec![timestamped_log("client gui started")],
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
                ClientEvent::Log(line) => self.push_log(line),
            }
        }
        changed
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.log_lines.push(timestamped_log(line));
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
const ACTIVITY_LOG_LIMIT: usize = 300;

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

fn video_stream_loop(
    client_id: String,
    source: VideoSource,
    start_payload: String,
    out_tx: Sender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
) {
    let quality = remote_desktop_value(&start_payload, "quality")
        .or_else(|| video_control_value(&start_payload, "quality"))
        .unwrap_or_else(|| "medium".to_string());
    let fps = quality_fps(&quality);
    let interval = Duration::from_millis((1000 / fps).max(1));
    let mut seq = 1u64;
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
                if queue_message(&out_tx, &session_token, message).is_err() {
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

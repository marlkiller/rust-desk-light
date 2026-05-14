mod commands;
mod remote_management;
mod support;
mod system_info;
mod user_interaction;

use eframe::egui;
use rdl_protocol::{
    read_envelope, write_envelope_with_token, CommandKind, Message, Role, DEFAULT_SERVER_IP,
    DEFAULT_SERVER_PORT,
};
use std::fs;
use std::io;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

const INITIAL_RECONNECT_DELAY_MS: u64 = 500;
const MAX_RECONNECT_DELAY_MS: u64 = 8_000;

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
    let identity = load_client_identity();
    let (event_tx, event_rx) = mpsc::channel();
    let (input_tx, input_rx) = mpsc::channel();
    let app_config = config.clone();
    let network_identity = identity.clone();

    thread::spawn(move || {
        if let Err(error) = client_network_loop(
            app_config,
            network_identity,
            true,
            event_tx.clone(),
            input_rx,
        ) {
            let _ = event_tx.send(ClientEvent::Log(format!("network stopped: {error}")));
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
                cc, config, identity, event_rx, input_tx,
            )))
        }),
    )
}

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
        if let Err(error) = client_network_loop(config, identity, false, event_tx.clone(), input_rx)
        {
            let _ = event_tx.send(ClientEvent::Log(format!("network stopped: {error}")));
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
    event_tx: Sender<ClientEvent>,
    input_rx: Receiver<ClientInput>,
) -> io::Result<()> {
    let mut delay = INITIAL_RECONNECT_DELAY_MS;
    loop {
        match client_connection_once(
            config.clone(),
            identity.clone(),
            gui_mode,
            event_tx.clone(),
            &input_rx,
        ) {
            Ok(()) => delay = INITIAL_RECONNECT_DELAY_MS,
            Err(error) => {
                let _ = event_tx.send(ClientEvent::Log(format!(
                    "connect failed: {error}; retrying in {delay}ms"
                )));
            }
        }
        let _ = event_tx.send(ClientEvent::Disconnected);
        thread::sleep(Duration::from_millis(delay));
        delay = (delay * 2).min(MAX_RECONNECT_DELAY_MS);
    }
}

fn client_connection_once(
    config: Config,
    identity: LocalIdentity,
    gui_mode: bool,
    event_tx: Sender<ClientEvent>,
    input_rx: &Receiver<ClientInput>,
) -> io::Result<()> {
    let stream = TcpStream::connect(format!("{}:{}", config.ip, config.port))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let mut writer = stream.try_clone()?;
    let mut next_message_id = 1u64;
    send(
        &mut writer,
        &mut next_message_id,
        "",
        Message::Hello {
            role: Role::Client,
            id: identity.id.clone(),
            fingerprint: identity.fingerprint.clone(),
            hostname: hostname(),
            os: std::env::consts::OS.to_string(),
            username: username(),
            gui_available: gui_mode,
        },
    )?;

    let mut reader = stream;
    let mut session_token = String::new();
    loop {
        while let Ok(input) = input_rx.try_recv() {
            match input {
                ClientInput::ChatReply { text } => send(
                    &mut writer,
                    &mut next_message_id,
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

        let message = match read_envelope(&mut reader) {
            Ok(envelope) => envelope.message,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => {
                let _ = event_tx.send(ClientEvent::Log(format!("network read failed: {error}")));
                break;
            }
        };

        match message {
            Message::Session { token } => {
                session_token = token;
                let _ = event_tx.send(ClientEvent::Connected);
            }
            Message::Command {
                target_id,
                command,
                payload,
            } => {
                let detail = commands::handle_command(&command, &payload, gui_mode);
                let _ = event_tx.send(ClientEvent::Command {
                    command: command.clone(),
                    payload: payload.clone(),
                });
                if command == CommandKind::TextChat && gui_mode {
                    let _ = event_tx.send(ClientEvent::ChatMessage { text: payload });
                }
                send(
                    &mut writer,
                    &mut next_message_id,
                    &session_token,
                    Message::CommandAck {
                        client_id: target_id,
                        command,
                        accepted: true,
                        detail,
                    },
                )?;
            }
            Message::Ping => send(
                &mut writer,
                &mut next_message_id,
                &session_token,
                Message::Pong,
            )?,
            other => {
                let _ = event_tx.send(ClientEvent::Log(format!("server: {other:?}")));
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
    ) -> Self {
        apply_client_theme(&cc.egui_ctx);
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

    fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
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
        self.drain_events();

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

        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
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

enum ClientInput {
    ChatReply { text: String },
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
        .unwrap_or_else(|_| "unknown-host".to_string())
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

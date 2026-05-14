use eframe::egui;
use rdl_protocol::{
    read_envelope, write_envelope, CommandKind, Message, Role, DEFAULT_SERVER_IP,
    DEFAULT_SERVER_PORT,
};
use std::io;
use std::net::TcpStream;
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
    let client_id = stable_client_id();
    let (event_tx, event_rx) = mpsc::channel();
    let app_config = config.clone();
    let network_client_id = client_id.clone();

    thread::spawn(move || {
        if let Err(error) =
            client_network_loop(app_config, network_client_id, true, event_tx.clone())
        {
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
        Box::new(move |cc| Ok(Box::new(ClientApp::new(cc, config, client_id, event_rx)))),
    )
}

fn run_terminal(config: Config) -> io::Result<()> {
    let client_id = stable_client_id();
    let (event_tx, event_rx) = mpsc::channel();
    println!(
        "rust-desk-light client terminal fallback, server={}:{}",
        config.ip, config.port
    );
    println!("client id: {client_id}");
    println!("waiting for admin commands; press Ctrl+C to exit");

    thread::spawn(move || {
        if let Err(error) = client_network_loop(config, client_id, false, event_tx.clone()) {
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
            ClientEvent::Log(line) => println!("{line}"),
        }
    }

    Ok(())
}

fn client_network_loop(
    config: Config,
    client_id: String,
    gui_mode: bool,
    event_tx: Sender<ClientEvent>,
) -> io::Result<()> {
    let mut delay = INITIAL_RECONNECT_DELAY_MS;
    loop {
        match client_connection_once(
            config.clone(),
            client_id.clone(),
            gui_mode,
            event_tx.clone(),
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
    client_id: String,
    gui_mode: bool,
    event_tx: Sender<ClientEvent>,
) -> io::Result<()> {
    let stream = TcpStream::connect(format!("{}:{}", config.ip, config.port))?;
    let mut writer = stream.try_clone()?;
    let mut next_message_id = 1u64;
    send(
        &mut writer,
        &mut next_message_id,
        Message::Hello {
            role: Role::Client,
            id: client_id,
            hostname: hostname(),
            os: std::env::consts::OS.to_string(),
            username: username(),
            gui_available: gui_mode,
        },
    )?;
    let _ = event_tx.send(ClientEvent::Connected);

    let mut reader = stream;
    loop {
        let message = match read_envelope(&mut reader) {
            Ok(envelope) => envelope.message,
            Err(error) => {
                let _ = event_tx.send(ClientEvent::Log(format!("network read failed: {error}")));
                break;
            }
        };

        match message {
            Message::Command {
                target_id,
                command,
                payload,
            } => {
                let detail = handle_command(&command, &payload, gui_mode);
                let _ = event_tx.send(ClientEvent::Command {
                    command: command.clone(),
                    payload,
                });
                send(
                    &mut writer,
                    &mut next_message_id,
                    Message::CommandAck {
                        client_id: target_id,
                        command,
                        accepted: true,
                        detail,
                    },
                )?;
            }
            Message::Ping => send(&mut writer, &mut next_message_id, Message::Pong)?,
            other => {
                let _ = event_tx.send(ClientEvent::Log(format!("server: {other:?}")));
            }
        }
    }

    Ok(())
}

struct ClientApp {
    config: Config,
    client_id: String,
    event_rx: Receiver<ClientEvent>,
    connected: bool,
    log_lines: Vec<String>,
}

impl ClientApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        client_id: String,
        event_rx: Receiver<ClientEvent>,
    ) -> Self {
        apply_client_theme(&cc.egui_ctx);
        Self {
            config,
            client_id,
            event_rx,
            connected: false,
            log_lines: vec!["client gui started".to_string()],
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
                    detail_row(ui, "Client ID", &self.client_id);
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
    Log(String),
}

fn handle_command(command: &CommandKind, payload: &str, gui_mode: bool) -> String {
    match command {
        CommandKind::ComputerInfo => format!(
            "hostname={} os={} arch={} user={}",
            hostname(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            username()
        ),
        CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::TextChat => {
            if gui_mode {
                format!("shown in client gui log: {payload}")
            } else {
                println!("admin message: {payload}");
                "shown in terminal fallback".to_string()
            }
        }
        _ => format!(
            "{} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

fn send(writer: &mut TcpStream, next_message_id: &mut u64, message: Message) -> io::Result<()> {
    let result = write_envelope(writer, Role::Client, *next_message_id, None, message);
    *next_message_id = next_message_id.saturating_add(1);
    result
}

fn stable_client_id() -> String {
    format!(
        "{}-{}-{}",
        hostname(),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
    .chars()
    .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
    .collect()
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

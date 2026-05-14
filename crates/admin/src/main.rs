use eframe::egui;
use rdl_protocol::{
    read_envelope, write_envelope, ClientInfo, CommandKind, Message, Role, DEFAULT_SERVER_IP,
    DEFAULT_SERVER_PORT,
};
use std::io::{self, BufRead};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env();
    if terminal_mode() {
        run_terminal(config)?;
    } else {
        run_gui(config)?;
    }
    Ok(())
}

fn run_gui(config: Config) -> eframe::Result {
    let (input_tx, input_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let network_config = config.clone();

    thread::spawn(move || {
        if let Err(error) = admin_network_loop(network_config, input_rx, event_tx.clone()) {
            let _ = event_tx.send(AdminEvent::Log(format!("network stopped: {error}")));
        }
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([980.0, 620.0]),
        ..Default::default()
    };

    eframe::run_native(
        "rust-desk-light admin",
        native_options,
        Box::new(move |cc| Ok(Box::new(AdminApp::new(cc, config, input_tx, event_rx)))),
    )
}

fn run_terminal(config: Config) -> io::Result<()> {
    println!(
        "rust-desk-light admin terminal mode, server={}:{}",
        config.ip, config.port
    );

    let (input_tx, input_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::spawn(move || {
        if let Err(error) = admin_network_loop(config, input_rx, event_tx.clone()) {
            let _ = event_tx.send(AdminEvent::Log(format!("network stopped: {error}")));
        }
    });
    thread::spawn(move || terminal_input_loop(input_tx));

    for event in event_rx {
        match event {
            AdminEvent::Clients(clients) => {
                println!("online clients: {}", clients.len());
                for client in clients {
                    println!(
                        "- {} | host={} os={} user={} gui={}",
                        client.id,
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
    event_tx: Sender<AdminEvent>,
) -> io::Result<()> {
    let stream = TcpStream::connect(format!("{}:{}", config.ip, config.port))?;
    let mut writer = stream.try_clone()?;
    let mut next_message_id = 1u64;
    send(
        &mut writer,
        &mut next_message_id,
        Message::Hello {
            role: Role::Admin,
            id: "admin".to_string(),
            hostname: hostname(),
            os: std::env::consts::OS.to_string(),
            username: username(),
            gui_available: true,
        },
    )?;
    send(&mut writer, &mut next_message_id, Message::ListClients)?;
    let _ = event_tx.send(AdminEvent::Connected);

    let mut input_writer = writer.try_clone()?;
    let mut input_next_message_id = next_message_id;
    thread::spawn(move || {
        for input in input_rx {
            let result = match input {
                AdminInput::List => send(
                    &mut input_writer,
                    &mut input_next_message_id,
                    Message::ListClients,
                ),
                AdminInput::Command {
                    target_id,
                    command,
                    payload,
                } => send(
                    &mut input_writer,
                    &mut input_next_message_id,
                    Message::Command {
                        target_id,
                        command,
                        payload,
                    },
                ),
                AdminInput::Quit => {
                    let _ = input_writer.shutdown(Shutdown::Both);
                    break;
                }
            };
            if result.is_err() {
                break;
            }
        }
    });

    let mut reader = stream;
    loop {
        let message = match read_envelope(&mut reader) {
            Ok(envelope) => envelope.message,
            Err(error) => {
                let _ = event_tx.send(AdminEvent::Log(format!("network read failed: {error}")));
                break;
            }
        };

        match message {
            Message::Clients(clients) => {
                let _ = event_tx.send(AdminEvent::Clients(clients));
            }
            Message::CommandAck {
                client_id,
                command,
                accepted,
                detail,
            } => {
                let _ = event_tx.send(AdminEvent::Ack {
                    client_id,
                    command,
                    accepted,
                    detail,
                });
            }
            other => {
                let _ = event_tx.send(AdminEvent::Log(format!("server: {other:?}")));
            }
        }
    }

    let _ = event_tx.send(AdminEvent::Disconnected);
    Ok(())
}

struct AdminApp {
    config: Config,
    input_tx: Sender<AdminInput>,
    event_rx: Receiver<AdminEvent>,
    connected: bool,
    clients: Vec<ClientInfo>,
    selected_client_id: Option<String>,
    log_lines: Vec<String>,
}

impl AdminApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        input_tx: Sender<AdminInput>,
        event_rx: Receiver<AdminEvent>,
    ) -> Self {
        apply_admin_theme(&cc.egui_ctx);
        Self {
            config,
            input_tx,
            event_rx,
            connected: false,
            clients: Vec::new(),
            selected_client_id: None,
            log_lines: vec!["admin gui started".to_string()],
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AdminEvent::Connected => {
                    self.connected = true;
                    self.log_lines.push("connected to server".to_string());
                }
                AdminEvent::Disconnected => {
                    self.connected = false;
                    self.log_lines.push("disconnected from server".to_string());
                }
                AdminEvent::Clients(clients) => {
                    self.log_lines
                        .push(format!("online clients refreshed: {}", clients.len()));
                    self.clients = clients;
                    if self.selected_client_id.is_none() {
                        self.selected_client_id =
                            self.clients.first().map(|client| client.id.clone());
                    }
                }
                AdminEvent::Ack {
                    client_id,
                    command,
                    accepted,
                    detail,
                } => self.log_lines.push(format!(
                    "ack client={} command={} accepted={} detail={}",
                    client_id,
                    command.as_str(),
                    accepted,
                    detail
                )),
                AdminEvent::Log(line) => self.log_lines.push(line),
            }
            if self.log_lines.len() > 300 {
                self.log_lines.remove(0);
            }
        }
    }

    fn selected_client(&self) -> Option<&ClientInfo> {
        let selected_id = self.selected_client_id.as_deref()?;
        self.clients
            .iter()
            .find(|client| client.id.as_str() == selected_id)
    }

    fn send_command(&mut self, client_id: &str, command: CommandKind) {
        let _ = self.input_tx.send(AdminInput::Command {
            target_id: client_id.to_string(),
            command: command.clone(),
            payload: String::new(),
        });
        self.log_lines.push(format!(
            "sent command={} to {}",
            command.as_str(),
            client_id
        ));
    }

    fn render_menu_bar(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            ui.horizontal(|ui| {
                section_title(ui, "Commands");
                ui.separator();
                if let Some(client_id) = self.selected_client_id.clone() {
                    render_context_menu(ui, &client_id, self);
                } else {
                    ui.label(
                        egui::RichText::new("Select a client to enable command menus")
                            .color(COLOR_MUTED),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, "Refresh").clicked() {
                        let _ = self.input_tx.send(AdminInput::List);
                    }
                    status_pill(ui, self.connected);
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
                    self.clients.len().to_string(),
                );
                metric(
                    &mut columns[1],
                    "Selected",
                    self.selected_client_id
                        .as_deref()
                        .unwrap_or("None")
                        .to_string(),
                );
                metric(
                    &mut columns[2],
                    "Connection",
                    if self.connected { "Online" } else { "Offline" }.to_string(),
                );
                if let Some(client) = self.selected_client() {
                    metric(&mut columns[3], "Host", client.hostname.clone());
                } else {
                    metric(&mut columns[3], "Host", "None".to_string());
                }
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
            ui.add_space(10.0);

            if self.clients.is_empty() {
                empty_state(ui);
                return;
            }

            egui::ScrollArea::vertical()
                .id_salt("admin_clients_scroll_area")
                .show(ui, |ui| {
                    egui::Grid::new("client_table")
                        .striped(true)
                        .num_columns(5)
                        .spacing([18.0, 10.0])
                        .min_col_width(92.0)
                        .show(ui, |ui| {
                            table_header(ui, "Client ID");
                            table_header(ui, "Host");
                            table_header(ui, "OS");
                            table_header(ui, "User");
                            table_header(ui, "GUI");
                            ui.end_row();

                            let clients = self.clients.clone();
                            for client in clients {
                                let selected =
                                    self.selected_client_id.as_deref() == Some(client.id.as_str());
                                let response = ui.selectable_label(selected, &client.id);
                                if response.clicked() {
                                    self.selected_client_id = Some(client.id.clone());
                                }
                                response
                                    .context_menu(|ui| render_context_menu(ui, &client.id, self));

                                ui.label(&client.hostname);
                                ui.label(&client.os);
                                ui.label(&client.username);
                                ui.label(if client.gui_available { "Yes" } else { "No" });
                                ui.end_row();
                            }
                        });
                });
        });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Activity");
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt("admin_activity_scroll_area")
                .stick_to_bottom(true)
                .max_height(180.0)
                .show(ui, |ui| {
                    for line in &self.log_lines {
                        ui.monospace(egui::RichText::new(line).size(12.0).color(COLOR_MUTED));
                    }
                });
        });
    }
}

impl eframe::App for AdminApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();

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

        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
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

fn apply_admin_theme(ctx: &egui::Context) {
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

fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .color(egui::Color32::WHITE)
                .strong(),
        )
        .fill(COLOR_ACCENT)
        .corner_radius(7.0),
    )
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

fn render_context_menu(ui: &mut egui::Ui, client_id: &str, app: &mut AdminApp) {
    ui.menu_button("Session", |ui| {
        ui.menu_button("Client", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Update Client",
                CommandKind::UpdateClient,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Uninstall Client",
                CommandKind::UninstallClient,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Kill Client Process",
                CommandKind::KillClientProcess,
            );
        });
        ui.menu_button("Power", |ui| {
            menu_command(ui, app, client_id, "Shutdown", CommandKind::Shutdown);
            menu_command(ui, app, client_id, "Reboot", CommandKind::Reboot);
        });
        ui.menu_button("Session Management", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Move To Group",
                CommandKind::MoveToGroup,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Clone Client Settings",
                CommandKind::CloneClientSettings,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Delete Client",
                CommandKind::DeleteClient,
            );
        });
    });
    ui.menu_button("Remote Management", |ui| {
        ui.menu_button("Files And Terminal", |ui| {
            menu_command(ui, app, client_id, "File Manager", CommandKind::FileManager);
            menu_command(
                ui,
                app,
                client_id,
                "Remote Terminal",
                CommandKind::RemoteTerminal,
            );
        });
        ui.menu_button("System Tools", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Process Manager",
                CommandKind::ProcessManager,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Window Manager",
                CommandKind::WindowManager,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Startup Manager",
                CommandKind::StartupManager,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Registry Manager",
                CommandKind::RegistryManager,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Driver Manager",
                CommandKind::DriverManager,
            );
            menu_command(ui, app, client_id, "Event Log", CommandKind::EventLog);
        });
        ui.menu_button("Monitoring", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Active Connections",
                CommandKind::ActiveConnections,
            );
            menu_command(
                ui,
                app,
                client_id,
                "Performance Monitor",
                CommandKind::PerformanceMonitor,
            );
        });
    });
    ui.menu_button("Live Control", |ui| {
        ui.menu_button("Desktop", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Remote Desktop",
                CommandKind::RemoteDesktop,
            );
        });
        ui.menu_button("Media Devices", |ui| {
            menu_command(ui, app, client_id, "Camera", CommandKind::Camera);
            menu_command(ui, app, client_id, "Audio Listen", CommandKind::AudioListen);
        });
    });
    ui.menu_button("User Interaction", |ui| {
        ui.menu_button("Prompts", |ui| {
            menu_command(ui, app, client_id, "Message Box", CommandKind::MessageBox);
            menu_command(ui, app, client_id, "Balloon Tip", CommandKind::BalloonTip);
        });
        ui.menu_button("Communication", |ui| {
            menu_command(ui, app, client_id, "Text Chat", CommandKind::TextChat);
            menu_command(ui, app, client_id, "Voice Chat", CommandKind::VoiceChat);
        });
        ui.menu_button("Text Actions", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Open Text In Notepad",
                CommandKind::OpenTextInNotepad,
            );
        });
    });
    ui.menu_button("System Info", |ui| {
        ui.menu_button("Basics", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Computer Info",
                CommandKind::ComputerInfo,
            );
            menu_command(ui, app, client_id, "Clipboard", CommandKind::Clipboard);
        });
        ui.menu_button("Network", |ui| {
            menu_command(ui, app, client_id, "Proxy", CommandKind::Proxy);
        });
    });
    ui.menu_button("Execute", |ui| {
        ui.menu_button("Code And Files", |ui| {
            menu_command(ui, app, client_id, "Execute File", CommandKind::ExecuteFile);
            menu_command(ui, app, client_id, "Execute Code", CommandKind::ExecuteCode);
        });
        ui.menu_button("Tasks", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Execute Static Command",
                CommandKind::ExecuteStaticCommand,
            );
            menu_command(ui, app, client_id, "Create Task", CommandKind::CreateTask);
        });
        ui.menu_button("Automation", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Command Preset",
                CommandKind::CommandPreset,
            );
        });
    });
    ui.menu_button("Plugins", |ui| {
        ui.menu_button("Extensions", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "Plugin Manager",
                CommandKind::PluginManager,
            );
        });
    });
}

fn menu_command(
    ui: &mut egui::Ui,
    app: &mut AdminApp,
    client_id: &str,
    label: &str,
    command: CommandKind,
) {
    if ui.button(label).clicked() {
        app.send_command(client_id, command);
        ui.close();
    }
}

fn terminal_input_loop(input_tx: Sender<AdminInput>) {
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

fn send(writer: &mut TcpStream, next_message_id: &mut u64, message: Message) -> io::Result<()> {
    let result = write_envelope(writer, Role::Admin, *next_message_id, None, message);
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
    Log(String),
}

fn terminal_mode() -> bool {
    std::env::var_os("RDL_FORCE_TERMINAL").is_some()
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
                    println!("Usage: rdl-admin [--ip 127.0.0.1] [--port 21115]");
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        Self { ip, port }
    }
}

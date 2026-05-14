mod command_menu;
mod remote_management;
mod user_interaction;

use eframe::egui;
use rdl_protocol::{
    read_envelope, write_envelope_with_token, ClientInfo, CommandKind, Message, Role,
    DEFAULT_SERVER_IP, DEFAULT_SERVER_PORT,
};
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead};
use std::net::{Shutdown, TcpStream};
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
    let repaint_handle = Arc::new(Mutex::new(None));
    let network_repaint_handle = repaint_handle.clone();

    thread::spawn(move || {
        let event_sink = AdminEventSink::new(event_tx, Some(network_repaint_handle));
        if let Err(error) = admin_network_loop(network_config, input_rx, event_sink.clone()) {
            event_sink.send(AdminEvent::Log(format!("network stopped: {error}")));
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
        Box::new(move |cc| {
            Ok(Box::new(AdminApp::new(
                cc,
                config,
                input_tx,
                event_rx,
                repaint_handle,
            )))
        }),
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
        let event_sink = AdminEventSink::new(event_tx, None);
        if let Err(error) = admin_network_loop(config, input_rx, event_sink.clone()) {
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
) -> io::Result<()> {
    let mut delay = INITIAL_RECONNECT_DELAY_MS;
    loop {
        match admin_connection_once(&config, &input_rx, &event_sink) {
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
            os: std::env::consts::OS.to_string(),
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
                AdminInput::Quit => {
                    let _ = stream.shutdown(Shutdown::Both);
                    return Ok(AdminConnectionExit::Quit);
                }
            };
            if result.is_err() {
                return Ok(AdminConnectionExit::Disconnected);
            }
        }

        let message = match read_envelope(&mut stream) {
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
                event_sink.send(AdminEvent::Log(format!("network read failed: {error}")));
                return Ok(AdminConnectionExit::Disconnected);
            }
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
    loop {
        let message = match read_envelope(stream) {
            Ok(envelope) => envelope.message,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
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

struct AdminApp {
    config: Config,
    input_tx: Sender<AdminInput>,
    event_rx: Receiver<AdminEvent>,
    connected: bool,
    clients: Vec<ClientRow>,
    client_filter: String,
    selected_client_id: Option<String>,
    command_windows: Vec<CommandResultWindow>,
    terminal_windows: Vec<remote_management::remote_terminal::TerminalWindow>,
    chat_windows: Vec<user_interaction::text_chat::ChatWindow>,
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
        input_tx: Sender<AdminInput>,
        event_rx: Receiver<AdminEvent>,
        repaint_handle: Arc<Mutex<Option<egui::Context>>>,
    ) -> Self {
        apply_admin_theme(&cc.egui_ctx);
        if let Ok(mut handle) = repaint_handle.lock() {
            *handle = Some(cc.egui_ctx.clone());
        }
        Self {
            config,
            input_tx,
            event_rx,
            connected: false,
            clients: Vec::new(),
            client_filter: String::new(),
            selected_client_id: None,
            command_windows: Vec::new(),
            terminal_windows: Vec::new(),
            chat_windows: Vec::new(),
            log_lines: vec![timestamped_log("admin gui started")],
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.event_rx.try_recv() {
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
                } => self.handle_command_ack(client_id, command, accepted, detail),
                AdminEvent::Log(line) => self.push_log(line),
            }
            if self.log_lines.len() > 300 {
                self.log_lines.remove(0);
            }
        }
        changed
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.log_lines.push(timestamped_log(line));
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
        if command == CommandKind::TextChat {
            self.open_chat_window(client_id);
            return;
        }
        if command == CommandKind::RemoteTerminal {
            self.open_terminal_window(client_id);
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

    fn open_terminal_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        remote_management::remote_terminal::open_window(
            &mut self.terminal_windows,
            client_id,
            hostname,
            username,
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
        if command == CommandKind::RemoteTerminal {
            self.handle_terminal_ack(&client_id, accepted, detail);
            return;
        }

        let (hostname, username) = self.client_window_identity(&client_id);
        if let Some(window) = self.command_windows.iter_mut().rev().find(|window| {
            window.client_id == client_id
                && window.command == command
                && matches!(window.status, CommandResultStatus::Pending)
        }) {
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
            process_kill_requested: Arc::new(Mutex::new(None)),
            table_filter: Arc::new(Mutex::new(String::new())),
            table_sort: Arc::new(Mutex::new(None)),
            table_selected_row: Arc::new(Mutex::new(None)),
        });
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

    fn handle_terminal_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        remote_management::remote_terminal::handle_ack(
            &mut self.terminal_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn refresh_process_window(&mut self, client_id: &str) {
        let Some(window) = self.command_windows.iter_mut().rev().find(|window| {
            window.client_id == client_id && window.command == CommandKind::ProcessManager
        }) else {
            return;
        };

        let _ = self.input_tx.send(AdminInput::Command {
            target_id: client_id.to_string(),
            command: CommandKind::ProcessManager,
            payload: String::new(),
        });
        window.status = CommandResultStatus::Pending;
        window.detail = "Refreshing process list...".to_string();
        window.open = true;
        self.push_log(format!("refresh command=process_manager to {client_id}"));
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
                metric(
                    &mut columns[3],
                    "Connection",
                    if self.connected {
                        "Online"
                    } else {
                        "Reconnecting"
                    }
                    .to_string(),
                );
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
            ui.add(
                egui::TextEdit::singleline(&mut self.client_filter)
                    .hint_text("Search by id, fingerprint, host, user, or OS")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(10.0);

            let clients = self.filtered_clients();
            if clients.is_empty() {
                empty_state(ui);
                return;
            }

            egui::ScrollArea::vertical()
                .id_salt("admin_clients_scroll_area")
                .show(ui, |ui| {
                    egui::Grid::new("client_table")
                        .striped(true)
                        .num_columns(7)
                        .spacing([14.0, 10.0])
                        .min_col_width(82.0)
                        .show(ui, |ui| {
                            table_header(ui, "Status");
                            table_header(ui, "Client ID");
                            table_header(ui, "Fingerprint");
                            table_header(ui, "Host");
                            table_header(ui, "User");
                            table_header(ui, "OS");
                            table_header(ui, "Last Heartbeat");
                            ui.end_row();

                            for row in clients {
                                let client = row.info;
                                let selected =
                                    self.selected_client_id.as_deref() == Some(client.id.as_str());
                                client_status_badge(ui, row.status);
                                let response =
                                    ui.selectable_label(selected, compact_id(&client.id));
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

                                ui.label(egui::RichText::new(&client.fingerprint).size(12.0));
                                ui.label(&client.hostname);
                                ui.label(&client.username);
                                ui.label(&client.os);
                                ui.label(last_seen_label(client.last_seen_epoch_ms));
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
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(ui.available_width());
                        for line in &self.log_lines {
                            ui.label(egui::RichText::new(line).size(12.0).color(COLOR_MUTED));
                        }
                    });
                });
        });
    }

    fn render_command_windows(&mut self, ctx: &egui::Context) {
        let mut pending_logs = Vec::new();
        for window in &mut self.command_windows {
            if window.close_requested.load(Ordering::Relaxed) {
                window.open = false;
            }
            if window.refresh_requested.swap(false, Ordering::Relaxed) {
                let _ = self.input_tx.send(AdminInput::Command {
                    target_id: window.client_id.clone(),
                    command: window.command.clone(),
                    payload: String::new(),
                });
                window.status = CommandResultStatus::Pending;
                window.detail = "Refreshing command result...".to_string();
                window.open = true;
                pending_logs.push(format!(
                    "refresh command={} to {}",
                    window.command.as_str(),
                    window.client_id
                ));
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
            let builder = egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([760.0, 460.0])
                .with_min_inner_size([260.0, 180.0])
                .with_resizable(true);

            let command = window.command.clone();
            let status = window.status;
            let detail = window.detail.clone();
            let close_requested = window.close_requested.clone();
            let refresh_requested = window.refresh_requested.clone();
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
        self.render_terminal_windows(ui.ctx());
        self.render_chat_windows(ui.ctx());

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

fn connection_status_pill(ui: &mut egui::Ui, connected: bool) {
    let (text, color) = if connected {
        ("Online", COLOR_GOOD)
    } else {
        ("Reconnecting", COLOR_BAD)
    };
    status_badge(ui, text, color);
}

fn client_status_badge(ui: &mut egui::Ui, status: ClientStatus) {
    let (text, color) = match status {
        ClientStatus::Online => ("Online", COLOR_GOOD),
        ClientStatus::Stale => ("Stale", COLOR_WARN),
        ClientStatus::Offline => ("Offline", COLOR_BAD),
    };
    status_badge(ui, text, color);
}

fn timestamped_log(line: impl Into<String>) -> String {
    format!("[{}] {}", activity_time_label(), line.into())
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
    let expects_table = matches!(
        command,
        CommandKind::ProcessManager | CommandKind::EventLog | CommandKind::ActiveConnections
    );
    if expects_table
        && matches!(status, CommandResultStatus::Accepted)
        && parse_result_table(detail).is_none()
    {
        Some("Table data could not be parsed; showing raw output".to_string())
    } else {
        None
    }
}

fn kill_target_process_succeeded(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("ok")
        && !detail.contains("refused")
        && !detail.contains("requires")
        && !detail.contains("failed")
        && !detail.contains("exited with error")
}

fn render_command_result(
    ui: &mut egui::Ui,
    command: &CommandKind,
    detail: &mut String,
    table_filter: &Arc<Mutex<String>>,
    table_sort: &Arc<Mutex<Option<TableSort>>>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    refresh_requested: &Arc<AtomicBool>,
    refresh_in_flight: bool,
    process_kill_requested: &Arc<Mutex<Option<String>>>,
) {
    let expects_table = matches!(
        command,
        CommandKind::ProcessManager | CommandKind::EventLog | CommandKind::ActiveConnections
    );
    if expects_table {
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
    }

    ui.add(
        egui::TextEdit::multiline(detail)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .desired_rows(18)
            .interactive(false),
    );
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
        ui.label(egui::RichText::new("Filter").size(12.0).color(COLOR_MUTED));
        let response = ui.add(
            egui::TextEdit::singleline(&mut filter)
                .hint_text("Filter table content")
                .desired_width(240.0),
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

struct ResultTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
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
    let widths = table_column_widths(command, &table.headers, ui.available_width());
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

    egui::Frame::default()
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(6.0)
        .show(ui, |ui| {
            table_header_row(ui, &table.headers, &widths, &mut sort);
            for (row_index, row) in rows.iter().enumerate() {
                let row_key = table_row_key(row);
                table_row(
                    ui,
                    row,
                    &widths,
                    false,
                    row_index,
                    selected_row.as_deref() == Some(row_key.as_str()),
                    &row_key,
                    table_selected_row,
                    process_row_pid(command, &table.headers, row),
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

fn filtered_table_rows(table: &ResultTable, filter: &str) -> Vec<Vec<String>> {
    table
        .rows
        .iter()
        .filter(|row| {
            filter.is_empty()
                || row
                    .iter()
                    .any(|cell| cell.to_ascii_lowercase().contains(filter))
        })
        .cloned()
        .collect()
}

fn sort_table_rows(rows: &mut [Vec<String>], sort: Option<TableSort>) {
    let Some(sort) = sort else {
        return;
    };
    rows.sort_by(|left, right| {
        let left = left.get(sort.column).map(String::as_str).unwrap_or("");
        let right = right.get(sort.column).map(String::as_str).unwrap_or("");
        let ordering = compare_table_cells(left, right);
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

fn table_row_key(row: &[String]) -> String {
    row.join("\t")
}

fn table_header_row(
    ui: &mut egui::Ui,
    cells: &[String],
    widths: &[f32],
    sort: &mut Option<TableSort>,
) {
    let fill = egui::Color32::from_rgb(235, 240, 247);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        for (index, width) in widths.iter().enumerate() {
            let cell = cells.get(index).map(String::as_str).unwrap_or("");
            let marker = match sort {
                Some(current) if current.column == index && current.ascending => " ^",
                Some(current) if current.column == index => " v",
                _ => "",
            };
            egui::Frame::default()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
                .inner_margin(egui::Margin::symmetric(4, 4))
                .show(ui, |ui| {
                    ui.set_width(*width);
                    let response = ui.add_sized(
                        [*width, 18.0],
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "{}{}",
                                truncate_table_cell(cell, *width - 14.0),
                                marker
                            ))
                            .size(12.0)
                            .color(COLOR_MUTED)
                            .strong(),
                        )
                        .frame(false),
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

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        let row_text = cells.join("\t");
        for (index, width) in widths.iter().enumerate() {
            let cell = cells.get(index).map(String::as_str).unwrap_or("");
            egui::Frame::default()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
                .inner_margin(egui::Margin::symmetric(4, 4))
                .show(ui, |ui| {
                    ui.set_width(*width);
                    let response = ui.add_sized(
                        [*width, 18.0],
                        egui::Label::new(
                            egui::RichText::new(truncate_table_cell(cell, *width))
                                .size(12.0)
                                .color(if header { COLOR_MUTED } else { COLOR_TEXT })
                                .strong(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if response.clicked() && !header {
                        if let Ok(mut value) = table_selected_row.lock() {
                            *value = Some(row_key.to_string());
                        }
                    }
                    response.context_menu(|ui| {
                        if ui.button("Copy Cell").clicked() {
                            ui.ctx().copy_text(cell.to_string());
                            ui.close();
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
                });
        }
    });
}

fn process_row_pid(command: &CommandKind, headers: &[String], row: &[String]) -> Option<String> {
    if *command != CommandKind::ProcessManager {
        return None;
    }
    let pid_index = headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case("pid"))?;
    let pid = row.get(pid_index)?.trim();
    if pid.chars().all(|ch| ch.is_ascii_digit()) {
        Some(pid.to_string())
    } else {
        None
    }
}

fn table_column_widths(
    command: &CommandKind,
    headers: &[String],
    available_width: f32,
) -> Vec<f32> {
    let available_width = available_width.max(1.0);
    let specs = headers
        .iter()
        .map(|header| match command {
            CommandKind::ProcessManager => process_column_spec(header),
            CommandKind::EventLog => event_log_column_spec(header),
            CommandKind::ActiveConnections => connection_column_spec(header),
            _ => (1.0, 0.0),
        })
        .collect::<Vec<_>>();
    let total_weight = specs
        .iter()
        .map(|(weight, _)| *weight)
        .sum::<f32>()
        .max(1.0);

    specs
        .iter()
        .map(|(weight, _)| (available_width * *weight) / total_weight)
        .collect()
}

fn process_column_spec(header: &str) -> (f32, f32) {
    match header.to_ascii_lowercase().as_str() {
        "pid" | "ppid" => (0.8, 0.0),
        "cpu" | "pcpu" | "%cpu" | "memorymb" | "pmem" | "%mem" => (0.9, 0.0),
        "name" | "processname" | "comm" => (2.4, 0.0),
        _ => (1.2, 0.0),
    }
}

fn event_log_column_spec(header: &str) -> (f32, f32) {
    match header.to_ascii_lowercase().as_str() {
        "time" | "timecreated" => (1.6, 0.0),
        "level" | "leveldisplayname" => (0.9, 0.0),
        "provider" | "providername" => (1.8, 0.0),
        "id" => (0.6, 0.0),
        "message" => (4.2, 0.0),
        _ => (1.2, 0.0),
    }
}

fn connection_column_spec(header: &str) -> (f32, f32) {
    match header.to_ascii_lowercase().as_str() {
        "proto" | "netid" | "protocol" => (0.7, 0.0),
        "local" | "localaddress" | "local-address" | "local_address" => (2.2, 0.0),
        "foreign" | "peer" | "peeraddress" | "foreignaddress" | "foreign-address" => (2.2, 0.0),
        "state" => (1.0, 0.0),
        "pid" | "pid/program" | "pid/program name" => (1.4, 0.0),
        _ => (1.2, 0.0),
    }
}

fn truncate_table_cell(value: &str, width: f32) -> String {
    let max_chars = (width / 7.0).max(8.0) as usize;
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    format!("{}...", value.chars().take(keep).collect::<String>())
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
struct LocalIdentity {
    id: String,
    fingerprint: String,
}

fn load_admin_identity() -> LocalIdentity {
    let path = identity_file_path("admin.identity");
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
        "{}|{}|{}|{}",
        username(),
        hostname(),
        std::env::consts::OS,
        rdl_protocol::now_epoch_ms()
    );
    let id = format!(
        "admin-{}-{:08x}",
        sanitize(&username()),
        simple_hash(&seed) as u32
    );
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
            "{}|{}|{}|{}",
            id,
            hostname(),
            username(),
            std::env::consts::OS
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

fn sanitize(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    if sanitized.is_empty() {
        "admin".to_string()
    } else {
        sanitized
    }
}

fn simple_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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

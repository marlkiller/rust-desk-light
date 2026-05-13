use eframe::egui;
use rdl_protocol::{
    ClientInfo, CommandKind, Message, Role, DEFAULT_SERVER_IP, DEFAULT_SERVER_PORT,
};
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
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
        viewport: egui::ViewportBuilder::default().with_inner_size([1080.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "rust-desk-light admin",
        native_options,
        Box::new(move |_cc| Ok(Box::new(AdminApp::new(config, input_tx, event_rx)))),
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
    send(
        &mut writer,
        Message::Hello {
            role: Role::Admin,
            id: "admin".to_string(),
            hostname: hostname(),
            os: std::env::consts::OS.to_string(),
            username: username(),
            gui_available: true,
        },
    )?;
    send(&mut writer, Message::ListClients)?;
    let _ = event_tx.send(AdminEvent::Connected);

    let mut input_writer = writer.try_clone()?;
    thread::spawn(move || {
        for input in input_rx {
            let result = match input {
                AdminInput::List => send(&mut input_writer, Message::ListClients),
                AdminInput::Command {
                    target_id,
                    command,
                    payload,
                } => send(
                    &mut input_writer,
                    Message::Command {
                        target_id,
                        command,
                        payload,
                    },
                ),
                AdminInput::Quit => break,
            };
            if result.is_err() {
                break;
            }
        }
    });

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        match Message::decode(&line) {
            Ok(Message::Clients(clients)) => {
                let _ = event_tx.send(AdminEvent::Clients(clients));
            }
            Ok(Message::CommandAck {
                client_id,
                command,
                accepted,
                detail,
            }) => {
                let _ = event_tx.send(AdminEvent::Ack {
                    client_id,
                    command,
                    accepted,
                    detail,
                });
            }
            Ok(other) => {
                let _ = event_tx.send(AdminEvent::Log(format!("server: {other:?}")));
            }
            Err(error) => {
                let _ = event_tx.send(AdminEvent::Log(format!("protocol error: {error}")));
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
    payload: String,
    log_lines: Vec<String>,
}

impl AdminApp {
    fn new(config: Config, input_tx: Sender<AdminInput>, event_rx: Receiver<AdminEvent>) -> Self {
        Self {
            config,
            input_tx,
            event_rx,
            connected: false,
            clients: Vec::new(),
            selected_client_id: None,
            payload: String::new(),
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

    fn send_command(&mut self, client_id: &str, command: CommandKind) {
        let payload = self.payload.clone();
        let _ = self.input_tx.send(AdminInput::Command {
            target_id: client_id.to_string(),
            command: command.clone(),
            payload,
        });
        self.log_lines.push(format!(
            "sent command={} to {}",
            command.as_str(),
            client_id
        ));
    }
}

impl eframe::App for AdminApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let _ = frame;
        self.drain_events();

        ui.horizontal(|ui| {
            ui.heading("rust-desk-light admin");
            ui.separator();
            ui.label(if self.connected { "Online" } else { "Offline" });
            ui.separator();
            ui.monospace(format!("server {}:{}", self.config.ip, self.config.port));
            if ui.button("Refresh").clicked() {
                let _ = self.input_tx.send(AdminInput::List);
            }
        });
        ui.separator();

        ui.columns(2, |columns| {
            columns[0].vertical(|ui| {
                ui.heading("Online Clients");
                ui.label("Right-click a client row to open the management menu.");
                ui.separator();

                egui::Grid::new("client_table")
                    .striped(true)
                    .min_col_width(90.0)
                    .show(ui, |ui| {
                        ui.strong("Client ID");
                        ui.strong("Host");
                        ui.strong("OS");
                        ui.strong("User");
                        ui.strong("GUI");
                        ui.end_row();

                        let clients = self.clients.clone();
                        for client in clients {
                            let selected =
                                self.selected_client_id.as_deref() == Some(client.id.as_str());
                            let response = ui.selectable_label(selected, &client.id);
                            if response.clicked() {
                                self.selected_client_id = Some(client.id.clone());
                            }
                            response.context_menu(|ui| {
                                render_context_menu(ui, &client.id, self);
                            });
                            ui.label(&client.hostname);
                            ui.label(&client.os);
                            ui.label(&client.username);
                            ui.label(if client.gui_available { "yes" } else { "no" });
                            ui.end_row();
                        }
                    });
            });

            columns[1].vertical(|ui| {
                ui.heading("Action");
                ui.label("Payload");
                ui.text_edit_multiline(&mut self.payload);
                ui.separator();

                if let Some(client_id) = self.selected_client_id.clone() {
                    ui.label("Selected Client");
                    ui.monospace(&client_id);
                    ui.separator();
                    command_button(ui, "Computer Info", || {
                        self.send_command(&client_id, CommandKind::ComputerInfo)
                    });
                    command_button(ui, "Message Box", || {
                        self.send_command(&client_id, CommandKind::MessageBox)
                    });
                    command_button(ui, "Remote Terminal", || {
                        self.send_command(&client_id, CommandKind::RemoteTerminal)
                    });
                    command_button(ui, "Remote Desktop", || {
                        self.send_command(&client_id, CommandKind::RemoteDesktop)
                    });
                } else {
                    ui.label("No client selected");
                }

                ui.separator();
                ui.heading("Log");
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log_lines {
                            ui.monospace(line);
                        }
                    });
            });
        });

        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }
}

fn command_button(ui: &mut egui::Ui, label: &str, action: impl FnOnce()) {
    if ui.button(label).clicked() {
        action();
    }
}

fn render_context_menu(ui: &mut egui::Ui, client_id: &str, app: &mut AdminApp) {
    ui.menu_button("会话", |ui| {
        ui.menu_button("客户端", |ui| {
            menu_command(ui, app, client_id, "更新客户端", CommandKind::UpdateClient);
            menu_command(
                ui,
                app,
                client_id,
                "卸载客户端",
                CommandKind::UninstallClient,
            );
            menu_command(
                ui,
                app,
                client_id,
                "结束客户端进程",
                CommandKind::KillClientProcess,
            );
        });
        ui.menu_button("系统电源", |ui| {
            menu_command(ui, app, client_id, "关机", CommandKind::Shutdown);
            menu_command(ui, app, client_id, "重启", CommandKind::Reboot);
        });
        ui.menu_button("会话管理", |ui| {
            menu_command(ui, app, client_id, "移动到分组", CommandKind::MoveToGroup);
            menu_command(
                ui,
                app,
                client_id,
                "克隆客户端设置",
                CommandKind::CloneClientSettings,
            );
            menu_command(ui, app, client_id, "删除客户端", CommandKind::DeleteClient);
        });
    });
    ui.menu_button("远程管理", |ui| {
        ui.menu_button("文件与终端", |ui| {
            menu_command(ui, app, client_id, "文件管理", CommandKind::FileManager);
            menu_command(ui, app, client_id, "远程终端", CommandKind::RemoteTerminal);
        });
        ui.menu_button("系统管理", |ui| {
            menu_command(ui, app, client_id, "进程管理", CommandKind::ProcessManager);
            menu_command(ui, app, client_id, "窗口管理", CommandKind::WindowManager);
            menu_command(
                ui,
                app,
                client_id,
                "启动项管理",
                CommandKind::StartupManager,
            );
            menu_command(
                ui,
                app,
                client_id,
                "注册表管理",
                CommandKind::RegistryManager,
            );
            menu_command(ui, app, client_id, "驱动管理", CommandKind::DriverManager);
            menu_command(ui, app, client_id, "事件日志", CommandKind::EventLog);
        });
        ui.menu_button("系统监控", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "活动连接",
                CommandKind::ActiveConnections,
            );
            menu_command(
                ui,
                app,
                client_id,
                "性能监视",
                CommandKind::PerformanceMonitor,
            );
        });
    });
    ui.menu_button("实时控制", |ui| {
        ui.menu_button("桌面控制", |ui| {
            menu_command(ui, app, client_id, "远程桌面", CommandKind::RemoteDesktop);
        });
        ui.menu_button("媒体设备", |ui| {
            menu_command(ui, app, client_id, "摄像头", CommandKind::Camera);
            menu_command(ui, app, client_id, "音频监听", CommandKind::AudioListen);
        });
    });
    ui.menu_button("用户交互", |ui| {
        ui.menu_button("用户提示", |ui| {
            menu_command(ui, app, client_id, "消息框", CommandKind::MessageBox);
            menu_command(ui, app, client_id, "气泡提示", CommandKind::BalloonTip);
        });
        ui.menu_button("通信功能", |ui| {
            menu_command(ui, app, client_id, "文本聊天", CommandKind::TextChat);
            menu_command(ui, app, client_id, "语音聊天", CommandKind::VoiceChat);
        });
        ui.menu_button("文本交互", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "记事本打开文本",
                CommandKind::OpenTextInNotepad,
            );
        });
    });
    ui.menu_button("系统信息", |ui| {
        ui.menu_button("基础信息", |ui| {
            menu_command(ui, app, client_id, "计算机信息", CommandKind::ComputerInfo);
            menu_command(ui, app, client_id, "剪贴板", CommandKind::Clipboard);
        });
        ui.menu_button("网络能力", |ui| {
            menu_command(ui, app, client_id, "代理", CommandKind::Proxy);
        });
    });
    ui.menu_button("执行", |ui| {
        ui.menu_button("代码与文件执行", |ui| {
            menu_command(ui, app, client_id, "执行文件", CommandKind::ExecuteFile);
            menu_command(ui, app, client_id, "代码执行", CommandKind::ExecuteCode);
        });
        ui.menu_button("任务功能", |ui| {
            menu_command(
                ui,
                app,
                client_id,
                "执行静态命令",
                CommandKind::ExecuteStaticCommand,
            );
            menu_command(ui, app, client_id, "创建任务", CommandKind::CreateTask);
        });
        ui.menu_button("自动化", |ui| {
            menu_command(ui, app, client_id, "命令预设", CommandKind::CommandPreset);
        });
    });
    ui.menu_button("插件", |ui| {
        ui.menu_button("扩展功能", |ui| {
            menu_command(ui, app, client_id, "插件管理", CommandKind::PluginManager);
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

fn send(writer: &mut TcpStream, message: Message) -> io::Result<()> {
    writeln!(writer, "{}", message.encode())
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

use eframe::egui;
use rdl_protocol::{CommandKind, Message, Role, DEFAULT_SERVER_IP, DEFAULT_SERVER_PORT};
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

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
        viewport: egui::ViewportBuilder::default().with_inner_size([760.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "rust-desk-light client",
        native_options,
        Box::new(move |_cc| Ok(Box::new(ClientApp::new(config, client_id, event_rx)))),
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
    let stream = TcpStream::connect(format!("{}:{}", config.ip, config.port))?;
    let mut writer = stream.try_clone()?;
    send(
        &mut writer,
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

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        match Message::decode(&line) {
            Ok(Message::Command {
                target_id,
                command,
                payload,
            }) => {
                let detail = handle_command(&command, &payload, gui_mode);
                let _ = event_tx.send(ClientEvent::Command {
                    command: command.clone(),
                    payload,
                });
                send(
                    &mut writer,
                    Message::CommandAck {
                        client_id: target_id,
                        command,
                        accepted: true,
                        detail,
                    },
                )?;
            }
            Ok(Message::Ping) => send(&mut writer, Message::Pong)?,
            Ok(other) => {
                let _ = event_tx.send(ClientEvent::Log(format!("server: {other:?}")));
            }
            Err(error) => {
                let _ = event_tx.send(ClientEvent::Log(format!("protocol error: {error}")));
            }
        }
    }

    let _ = event_tx.send(ClientEvent::Disconnected);
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
    fn new(config: Config, client_id: String, event_rx: Receiver<ClientEvent>) -> Self {
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
}

impl eframe::App for ClientApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let _ = frame;
        self.drain_events();

        ui.heading("rust-desk-light client");
        ui.separator();

        egui::Grid::new("client_status")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Status");
                ui.label(if self.connected {
                    "Online"
                } else {
                    "Connecting / Offline"
                });
                ui.end_row();

                ui.label("Client ID");
                ui.monospace(&self.client_id);
                ui.end_row();

                ui.label("Server");
                ui.monospace(format!("{}:{}", self.config.ip, self.config.port));
                ui.end_row();

                ui.label("Host");
                ui.monospace(hostname());
                ui.end_row();

                ui.label("OS");
                ui.monospace(format!(
                    "{} {}",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ));
                ui.end_row();
            });

        ui.separator();
        ui.label("Session Log");
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.log_lines {
                    ui.monospace(line);
                }
            });

        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }
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

fn send(writer: &mut TcpStream, message: Message) -> io::Result<()> {
    writeln!(writer, "{}", message.encode())
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

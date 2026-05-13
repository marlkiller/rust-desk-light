use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_SERVER_IP: &str = "127.0.0.1";
pub const DEFAULT_SERVER_PORT: u16 = 21115;
pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Role {
    Client,
    Admin,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Admin => "admin",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "client" => Some(Self::Client),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandKind {
    UpdateClient,
    UninstallClient,
    KillClientProcess,
    Shutdown,
    Reboot,
    MoveToGroup,
    CloneClientSettings,
    DeleteClient,
    FileManager,
    RemoteTerminal,
    ProcessManager,
    WindowManager,
    StartupManager,
    RegistryManager,
    DriverManager,
    EventLog,
    ActiveConnections,
    PerformanceMonitor,
    RemoteDesktop,
    Camera,
    AudioListen,
    MessageBox,
    BalloonTip,
    TextChat,
    VoiceChat,
    OpenTextInNotepad,
    ComputerInfo,
    Clipboard,
    Proxy,
    ExecuteFile,
    ExecuteCode,
    ExecuteStaticCommand,
    CreateTask,
    CommandPreset,
    PluginManager,
}

impl CommandKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UpdateClient => "update_client",
            Self::UninstallClient => "uninstall_client",
            Self::KillClientProcess => "kill_client_process",
            Self::Shutdown => "shutdown",
            Self::Reboot => "reboot",
            Self::MoveToGroup => "move_to_group",
            Self::CloneClientSettings => "clone_client_settings",
            Self::DeleteClient => "delete_client",
            Self::FileManager => "file_manager",
            Self::RemoteTerminal => "remote_terminal",
            Self::ProcessManager => "process_manager",
            Self::WindowManager => "window_manager",
            Self::StartupManager => "startup_manager",
            Self::RegistryManager => "registry_manager",
            Self::DriverManager => "driver_manager",
            Self::EventLog => "event_log",
            Self::ActiveConnections => "active_connections",
            Self::PerformanceMonitor => "performance_monitor",
            Self::RemoteDesktop => "remote_desktop",
            Self::Camera => "camera",
            Self::AudioListen => "audio_listen",
            Self::MessageBox => "message_box",
            Self::BalloonTip => "balloon_tip",
            Self::TextChat => "text_chat",
            Self::VoiceChat => "voice_chat",
            Self::OpenTextInNotepad => "open_text_in_notepad",
            Self::ComputerInfo => "computer_info",
            Self::Clipboard => "clipboard",
            Self::Proxy => "proxy",
            Self::ExecuteFile => "execute_file",
            Self::ExecuteCode => "execute_code",
            Self::ExecuteStaticCommand => "execute_static_command",
            Self::CreateTask => "create_task",
            Self::CommandPreset => "command_preset",
            Self::PluginManager => "plugin_manager",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "update_client" => Self::UpdateClient,
            "uninstall_client" => Self::UninstallClient,
            "kill_client_process" => Self::KillClientProcess,
            "shutdown" => Self::Shutdown,
            "reboot" => Self::Reboot,
            "move_to_group" => Self::MoveToGroup,
            "clone_client_settings" => Self::CloneClientSettings,
            "delete_client" => Self::DeleteClient,
            "file_manager" => Self::FileManager,
            "remote_terminal" => Self::RemoteTerminal,
            "process_manager" => Self::ProcessManager,
            "window_manager" => Self::WindowManager,
            "startup_manager" => Self::StartupManager,
            "registry_manager" => Self::RegistryManager,
            "driver_manager" => Self::DriverManager,
            "event_log" => Self::EventLog,
            "active_connections" => Self::ActiveConnections,
            "performance_monitor" => Self::PerformanceMonitor,
            "remote_desktop" => Self::RemoteDesktop,
            "camera" => Self::Camera,
            "audio_listen" => Self::AudioListen,
            "message_box" => Self::MessageBox,
            "balloon_tip" => Self::BalloonTip,
            "text_chat" => Self::TextChat,
            "voice_chat" => Self::VoiceChat,
            "open_text_in_notepad" => Self::OpenTextInNotepad,
            "computer_info" => Self::ComputerInfo,
            "clipboard" => Self::Clipboard,
            "proxy" => Self::Proxy,
            "execute_file" => Self::ExecuteFile,
            "execute_code" => Self::ExecuteCode,
            "execute_static_command" => Self::ExecuteStaticCommand,
            "create_task" => Self::CreateTask,
            "command_preset" => Self::CommandPreset,
            "plugin_manager" => Self::PluginManager,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientInfo {
    pub id: String,
    pub hostname: String,
    pub os: String,
    pub username: String,
    pub gui_available: bool,
    pub started_at_epoch_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Hello {
        role: Role,
        id: String,
        hostname: String,
        os: String,
        username: String,
        gui_available: bool,
    },
    ListClients,
    Clients(Vec<ClientInfo>),
    Command {
        target_id: String,
        command: CommandKind,
        payload: String,
    },
    CommandAck {
        client_id: String,
        command: CommandKind,
        accepted: bool,
        detail: String,
    },
    Error {
        detail: String,
    },
    Ping,
    Pong,
}

impl Message {
    pub fn encode(&self) -> String {
        match self {
            Self::Hello {
                role,
                id,
                hostname,
                os,
                username,
                gui_available,
            } => join_fields(&[
                "HELLO",
                role.as_str(),
                id,
                hostname,
                os,
                username,
                if *gui_available { "1" } else { "0" },
            ]),
            Self::ListClients => "LIST_CLIENTS".to_string(),
            Self::Clients(clients) => {
                let mut fields = vec!["CLIENTS".to_string(), clients.len().to_string()];
                for client in clients {
                    fields.push(client.id.clone());
                    fields.push(client.hostname.clone());
                    fields.push(client.os.clone());
                    fields.push(client.username.clone());
                    fields.push(if client.gui_available { "1" } else { "0" }.to_string());
                    fields.push(client.started_at_epoch_ms.to_string());
                }
                join_owned_fields(&fields)
            }
            Self::Command {
                target_id,
                command,
                payload,
            } => join_fields(&["COMMAND", target_id, command.as_str(), payload]),
            Self::CommandAck {
                client_id,
                command,
                accepted,
                detail,
            } => join_fields(&[
                "COMMAND_ACK",
                client_id,
                command.as_str(),
                if *accepted { "1" } else { "0" },
                detail,
            ]),
            Self::Error { detail } => join_fields(&["ERROR", detail]),
            Self::Ping => "PING".to_string(),
            Self::Pong => "PONG".to_string(),
        }
    }

    pub fn decode(line: &str) -> Result<Self, ProtocolError> {
        let fields = split_fields(line.trim_end_matches(['\r', '\n']));
        let tag = fields.first().map(String::as_str).unwrap_or_default();

        match tag {
            "HELLO" => {
                if fields.len() != 7 {
                    return Err(ProtocolError::InvalidFieldCount("HELLO"));
                }
                Ok(Self::Hello {
                    role: Role::parse(&fields[1]).ok_or(ProtocolError::InvalidRole)?,
                    id: fields[2].clone(),
                    hostname: fields[3].clone(),
                    os: fields[4].clone(),
                    username: fields[5].clone(),
                    gui_available: fields[6] == "1",
                })
            }
            "LIST_CLIENTS" => Ok(Self::ListClients),
            "CLIENTS" => {
                if fields.len() < 2 {
                    return Err(ProtocolError::InvalidFieldCount("CLIENTS"));
                }
                let count: usize = fields[1]
                    .parse()
                    .map_err(|_| ProtocolError::InvalidNumber)?;
                let expected_len = 2 + count * 6;
                if fields.len() != expected_len {
                    return Err(ProtocolError::InvalidFieldCount("CLIENTS"));
                }
                let mut clients = Vec::with_capacity(count);
                for chunk in fields[2..].chunks_exact(6) {
                    clients.push(parse_client_info_fields(chunk)?);
                }
                Ok(Self::Clients(clients))
            }
            "COMMAND" => {
                if fields.len() != 4 {
                    return Err(ProtocolError::InvalidFieldCount("COMMAND"));
                }
                Ok(Self::Command {
                    target_id: fields[1].clone(),
                    command: CommandKind::parse(&fields[2]).ok_or(ProtocolError::InvalidCommand)?,
                    payload: fields[3].clone(),
                })
            }
            "COMMAND_ACK" => {
                if fields.len() != 5 {
                    return Err(ProtocolError::InvalidFieldCount("COMMAND_ACK"));
                }
                Ok(Self::CommandAck {
                    client_id: fields[1].clone(),
                    command: CommandKind::parse(&fields[2]).ok_or(ProtocolError::InvalidCommand)?,
                    accepted: fields[3] == "1",
                    detail: fields[4].clone(),
                })
            }
            "ERROR" => {
                if fields.len() != 2 {
                    return Err(ProtocolError::InvalidFieldCount("ERROR"));
                }
                Ok(Self::Error {
                    detail: fields[1].clone(),
                })
            }
            "PING" => Ok(Self::Ping),
            "PONG" => Ok(Self::Pong),
            _ => Err(ProtocolError::UnknownTag(tag.to_string())),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    UnknownTag(String),
    InvalidFieldCount(&'static str),
    InvalidRole,
    InvalidCommand,
    InvalidClientInfo,
    InvalidNumber,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTag(tag) => write!(f, "unknown message tag: {tag}"),
            Self::InvalidFieldCount(tag) => write!(f, "invalid field count for {tag}"),
            Self::InvalidRole => write!(f, "invalid role"),
            Self::InvalidCommand => write!(f, "invalid command"),
            Self::InvalidClientInfo => write!(f, "invalid client info"),
            Self::InvalidNumber => write!(f, "invalid number"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn now_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn parse_client_info_fields(fields: &[String]) -> Result<ClientInfo, ProtocolError> {
    if fields.len() != 6 {
        return Err(ProtocolError::InvalidClientInfo);
    }
    Ok(ClientInfo {
        id: fields[0].clone(),
        hostname: fields[1].clone(),
        os: fields[2].clone(),
        username: fields[3].clone(),
        gui_available: fields[4] == "1",
        started_at_epoch_ms: fields[5]
            .parse()
            .map_err(|_| ProtocolError::InvalidNumber)?,
    })
}

fn join_fields(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| escape(field))
        .collect::<Vec<_>>()
        .join("|")
}

fn join_owned_fields(fields: &[String]) -> String {
    fields
        .iter()
        .map(|field| escape(field))
        .collect::<Vec<_>>()
        .join("|")
}

fn split_fields(value: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for ch in value.chars() {
        if escaped {
            current.push(match ch {
                'n' => '\n',
                'r' => '\r',
                'p' => '|',
                'b' => '\\',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            fields.push(current);
            current = String::new();
        } else {
            current.push(ch);
        }
    }
    fields.push(current);
    fields
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\b")
        .replace('|', "\\p")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_round_trips() {
        let message = Message::Command {
            target_id: "client|1".to_string(),
            command: CommandKind::RemoteTerminal,
            payload: "whoami\nhostname".to_string(),
        };

        let decoded = Message::decode(&message.encode()).unwrap();
        assert_eq!(message, decoded);
    }

    #[test]
    fn clients_round_trip() {
        let message = Message::Clients(vec![ClientInfo {
            id: "a".to_string(),
            hostname: "host".to_string(),
            os: "linux".to_string(),
            username: "user".to_string(),
            gui_available: true,
            started_at_epoch_ms: 42,
        }]);

        let decoded = Message::decode(&message.encode()).unwrap();
        assert_eq!(message, decoded);
    }
}

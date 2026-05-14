use std::fmt;
use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_SERVER_IP: &str = "127.0.0.1";
pub const DEFAULT_SERVER_PORT: u16 = 21115;
pub const PROTOCOL_VERSION: u16 = 1;
pub const FRAME_MAGIC: [u8; 4] = *b"RDL1";
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

const HEADER_LEN: usize = 10;
const ENVELOPE_FIXED_LEN: usize = 27;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Role {
    Client,
    Admin,
    Server,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Admin => "admin",
            Self::Server => "server",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "client" => Some(Self::Client),
            "admin" => Some(Self::Admin),
            "server" => Some(Self::Server),
            _ => None,
        }
    }

    fn to_code(&self) -> u8 {
        match self {
            Self::Client => 1,
            Self::Admin => 2,
            Self::Server => 3,
        }
    }

    fn from_code(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Client),
            2 => Ok(Self::Admin),
            3 => Ok(Self::Server),
            _ => Err(ProtocolError::InvalidRole),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandKind {
    UpdateClient,
    UninstallClient,
    KillClientProcess,
    KillTargetProcess,
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
            Self::KillTargetProcess => "kill_target_process",
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
            "kill_target_process" => Self::KillTargetProcess,
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

    fn to_code(&self) -> u16 {
        match self {
            Self::UpdateClient => 1,
            Self::UninstallClient => 2,
            Self::KillClientProcess => 3,
            Self::Shutdown => 4,
            Self::Reboot => 5,
            Self::MoveToGroup => 6,
            Self::CloneClientSettings => 7,
            Self::DeleteClient => 8,
            Self::FileManager => 9,
            Self::RemoteTerminal => 10,
            Self::ProcessManager => 11,
            Self::WindowManager => 12,
            Self::StartupManager => 13,
            Self::RegistryManager => 14,
            Self::DriverManager => 15,
            Self::EventLog => 16,
            Self::ActiveConnections => 17,
            Self::PerformanceMonitor => 18,
            Self::RemoteDesktop => 19,
            Self::Camera => 20,
            Self::AudioListen => 21,
            Self::MessageBox => 22,
            Self::BalloonTip => 23,
            Self::TextChat => 24,
            Self::VoiceChat => 25,
            Self::OpenTextInNotepad => 26,
            Self::ComputerInfo => 27,
            Self::Clipboard => 28,
            Self::Proxy => 29,
            Self::ExecuteFile => 30,
            Self::ExecuteCode => 31,
            Self::ExecuteStaticCommand => 32,
            Self::CreateTask => 33,
            Self::CommandPreset => 34,
            Self::PluginManager => 35,
            Self::KillTargetProcess => 36,
        }
    }

    fn from_code(value: u16) -> Result<Self, ProtocolError> {
        Ok(match value {
            1 => Self::UpdateClient,
            2 => Self::UninstallClient,
            3 => Self::KillClientProcess,
            4 => Self::Shutdown,
            5 => Self::Reboot,
            6 => Self::MoveToGroup,
            7 => Self::CloneClientSettings,
            8 => Self::DeleteClient,
            9 => Self::FileManager,
            10 => Self::RemoteTerminal,
            11 => Self::ProcessManager,
            12 => Self::WindowManager,
            13 => Self::StartupManager,
            14 => Self::RegistryManager,
            15 => Self::DriverManager,
            16 => Self::EventLog,
            17 => Self::ActiveConnections,
            18 => Self::PerformanceMonitor,
            19 => Self::RemoteDesktop,
            20 => Self::Camera,
            21 => Self::AudioListen,
            22 => Self::MessageBox,
            23 => Self::BalloonTip,
            24 => Self::TextChat,
            25 => Self::VoiceChat,
            26 => Self::OpenTextInNotepad,
            27 => Self::ComputerInfo,
            28 => Self::Clipboard,
            29 => Self::Proxy,
            30 => Self::ExecuteFile,
            31 => Self::ExecuteCode,
            32 => Self::ExecuteStaticCommand,
            33 => Self::CreateTask,
            34 => Self::CommandPreset,
            35 => Self::PluginManager,
            36 => Self::KillTargetProcess,
            _ => return Err(ProtocolError::InvalidCommand),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientInfo {
    pub id: String,
    pub fingerprint: String,
    pub hostname: String,
    pub os: String,
    pub username: String,
    pub gui_available: bool,
    pub started_at_epoch_ms: u128,
    pub last_seen_epoch_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Hello {
        role: Role,
        id: String,
        fingerprint: String,
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
    Session {
        token: String,
    },
    Ping,
    Pong,
}

impl Message {
    fn kind_code(&self) -> u16 {
        match self {
            Self::Hello { .. } => 1,
            Self::ListClients => 2,
            Self::Clients(_) => 3,
            Self::Command { .. } => 4,
            Self::CommandAck { .. } => 5,
            Self::Error { .. } => 6,
            Self::Session { .. } => 7,
            Self::Ping => 8,
            Self::Pong => 9,
        }
    }

    fn encode_payload(&self, writer: &mut BinaryWriter) {
        match self {
            Self::Hello {
                role,
                id,
                fingerprint,
                hostname,
                os,
                username,
                gui_available,
            } => {
                writer.u8(role.to_code());
                writer.string(id);
                writer.string(fingerprint);
                writer.string(hostname);
                writer.string(os);
                writer.string(username);
                writer.bool(*gui_available);
            }
            Self::ListClients | Self::Ping | Self::Pong => {}
            Self::Clients(clients) => {
                writer.u32(clients.len() as u32);
                for client in clients {
                    writer.string(&client.id);
                    writer.string(&client.fingerprint);
                    writer.string(&client.hostname);
                    writer.string(&client.os);
                    writer.string(&client.username);
                    writer.bool(client.gui_available);
                    writer.u128(client.started_at_epoch_ms);
                    writer.u128(client.last_seen_epoch_ms);
                }
            }
            Self::Command {
                target_id,
                command,
                payload,
            } => {
                writer.string(target_id);
                writer.u16(command.to_code());
                writer.string(payload);
            }
            Self::CommandAck {
                client_id,
                command,
                accepted,
                detail,
            } => {
                writer.string(client_id);
                writer.u16(command.to_code());
                writer.bool(*accepted);
                writer.string(detail);
            }
            Self::Error { detail } => writer.string(detail),
            Self::Session { token } => writer.string(token),
        }
    }

    fn decode_payload(kind: u16, payload: &[u8]) -> Result<Self, ProtocolError> {
        let mut reader = BinaryReader::new(payload);
        let message = match kind {
            1 => Self::Hello {
                role: Role::from_code(reader.u8()?)?,
                id: reader.string()?,
                fingerprint: reader.string()?,
                hostname: reader.string()?,
                os: reader.string()?,
                username: reader.string()?,
                gui_available: reader.bool()?,
            },
            2 => Self::ListClients,
            3 => {
                let count = reader.u32()? as usize;
                let mut clients = Vec::with_capacity(count);
                for _ in 0..count {
                    clients.push(ClientInfo {
                        id: reader.string()?,
                        fingerprint: reader.string()?,
                        hostname: reader.string()?,
                        os: reader.string()?,
                        username: reader.string()?,
                        gui_available: reader.bool()?,
                        started_at_epoch_ms: reader.u128()?,
                        last_seen_epoch_ms: reader.u128()?,
                    });
                }
                Self::Clients(clients)
            }
            4 => Self::Command {
                target_id: reader.string()?,
                command: CommandKind::from_code(reader.u16()?)?,
                payload: reader.string()?,
            },
            5 => Self::CommandAck {
                client_id: reader.string()?,
                command: CommandKind::from_code(reader.u16()?)?,
                accepted: reader.bool()?,
                detail: reader.string()?,
            },
            6 => Self::Error {
                detail: reader.string()?,
            },
            7 => Self::Session {
                token: reader.string()?,
            },
            8 => Self::Ping,
            9 => Self::Pong,
            _ => return Err(ProtocolError::InvalidMessageKind(kind)),
        };
        reader.finish()?;
        Ok(message)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub version: u16,
    pub message_id: u64,
    pub correlation_id: Option<u64>,
    pub role: Role,
    pub session_token: String,
    pub message: Message,
}

pub fn encode_envelope(envelope: &Envelope) -> Result<Vec<u8>, ProtocolError> {
    let mut payload = BinaryWriter::default();
    envelope.message.encode_payload(&mut payload);
    let payload = payload.into_inner();
    let token = envelope.session_token.as_bytes();
    let remaining_len = ENVELOPE_FIXED_LEN
        .checked_add(token.len())
        .ok_or(ProtocolError::FrameTooLarge)?
        .checked_add(payload.len())
        .ok_or(ProtocolError::FrameTooLarge)?;
    if remaining_len > MAX_FRAME_LEN as usize {
        return Err(ProtocolError::FrameTooLarge);
    }

    let mut frame = Vec::with_capacity(HEADER_LEN + remaining_len);
    frame.extend_from_slice(&FRAME_MAGIC);
    frame.extend_from_slice(&envelope.version.to_be_bytes());
    frame.extend_from_slice(&(remaining_len as u32).to_be_bytes());
    frame.extend_from_slice(&envelope.message_id.to_be_bytes());
    frame.extend_from_slice(&envelope.correlation_id.unwrap_or_default().to_be_bytes());
    frame.push(envelope.role.to_code());
    frame.extend_from_slice(&envelope.message.kind_code().to_be_bytes());
    frame.extend_from_slice(&(token.len() as u32).to_be_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(token);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_envelope(frame: &[u8]) -> Result<Envelope, ProtocolError> {
    if frame.len() < HEADER_LEN + ENVELOPE_FIXED_LEN {
        return Err(ProtocolError::TruncatedFrame);
    }
    if frame[0..4] != FRAME_MAGIC {
        return Err(ProtocolError::InvalidMagic);
    }

    let version = u16::from_be_bytes([frame[4], frame[5]]);
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(version));
    }

    let remaining_len = u32::from_be_bytes([frame[6], frame[7], frame[8], frame[9]]) as usize;
    if remaining_len > MAX_FRAME_LEN as usize {
        return Err(ProtocolError::FrameTooLarge);
    }
    if frame.len() != HEADER_LEN + remaining_len {
        return Err(ProtocolError::InvalidFrameLength);
    }

    let mut reader = BinaryReader::new(&frame[HEADER_LEN..]);
    let message_id = reader.u64()?;
    let correlation_id = match reader.u64()? {
        0 => None,
        value => Some(value),
    };
    let role = Role::from_code(reader.u8()?)?;
    let kind = reader.u16()?;
    let token_len = reader.u32()? as usize;
    let payload_len = reader.u32()? as usize;
    let session_token = String::from_utf8(reader.bytes(token_len)?.to_vec())
        .map_err(|_| ProtocolError::InvalidUtf8)?;
    let payload = reader.bytes(payload_len)?;
    reader.finish()?;

    Ok(Envelope {
        version,
        message_id,
        correlation_id,
        role,
        session_token,
        message: Message::decode_payload(kind, payload)?,
    })
}

pub fn write_envelope(
    writer: &mut impl Write,
    role: Role,
    message_id: u64,
    correlation_id: Option<u64>,
    message: Message,
) -> io::Result<()> {
    write_envelope_with_token(writer, role, message_id, correlation_id, "", message)
}

pub fn write_envelope_with_token(
    writer: &mut impl Write,
    role: Role,
    message_id: u64,
    correlation_id: Option<u64>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    let envelope = Envelope {
        version: PROTOCOL_VERSION,
        message_id,
        correlation_id,
        role,
        session_token: session_token.to_string(),
        message,
    };
    let frame = encode_envelope(&envelope).map_err(to_invalid_data)?;
    writer.write_all(&frame)
}

pub fn read_envelope(reader: &mut impl Read) -> io::Result<Envelope> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header)?;
    if header[0..4] != FRAME_MAGIC {
        return Err(to_invalid_data(ProtocolError::InvalidMagic));
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != PROTOCOL_VERSION {
        return Err(to_invalid_data(ProtocolError::UnsupportedVersion(version)));
    }
    let remaining_len = u32::from_be_bytes([header[6], header[7], header[8], header[9]]);
    if remaining_len > MAX_FRAME_LEN {
        return Err(to_invalid_data(ProtocolError::FrameTooLarge));
    }

    let mut frame = Vec::with_capacity(HEADER_LEN + remaining_len as usize);
    frame.extend_from_slice(&header);
    frame.resize(HEADER_LEN + remaining_len as usize, 0);
    reader.read_exact(&mut frame[HEADER_LEN..])?;
    decode_envelope(&frame).map_err(to_invalid_data)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidFrameLength,
    TruncatedFrame,
    FrameTooLarge,
    InvalidRole,
    InvalidCommand,
    InvalidMessageKind(u16),
    InvalidBool(u8),
    InvalidUtf8,
    TrailingBytes(usize),
    UnexpectedEof,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid frame magic"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported protocol version: {version}")
            }
            Self::InvalidFrameLength => write!(f, "invalid frame length"),
            Self::TruncatedFrame => write!(f, "truncated frame"),
            Self::FrameTooLarge => write!(f, "frame too large"),
            Self::InvalidRole => write!(f, "invalid role"),
            Self::InvalidCommand => write!(f, "invalid command"),
            Self::InvalidMessageKind(kind) => write!(f, "invalid message kind: {kind}"),
            Self::InvalidBool(value) => write!(f, "invalid bool byte: {value}"),
            Self::InvalidUtf8 => write!(f, "invalid utf-8 string"),
            Self::TrailingBytes(count) => write!(f, "payload has {count} trailing bytes"),
            Self::UnexpectedEof => write!(f, "unexpected end of payload"),
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

fn to_invalid_data(error: ProtocolError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[derive(Default)]
struct BinaryWriter {
    buffer: Vec<u8>,
}

impl BinaryWriter {
    fn into_inner(self) -> Vec<u8> {
        self.buffer
    }

    fn u8(&mut self, value: u8) {
        self.buffer.push(value);
    }

    fn bool(&mut self, value: bool) {
        self.u8(if value { 1 } else { 0 });
    }

    fn u16(&mut self, value: u16) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    fn u128(&mut self, value: u128) {
        self.buffer.extend_from_slice(&value.to_be_bytes());
    }

    fn string(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.buffer.extend_from_slice(value.as_bytes());
    }
}

struct BinaryReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> BinaryReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn finish(&self) -> Result<(), ProtocolError> {
        let trailing = self.data.len().saturating_sub(self.offset);
        if trailing == 0 {
            Ok(())
        } else {
            Err(ProtocolError::TrailingBytes(trailing))
        }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ProtocolError::UnexpectedEof)?;
        if end > self.data.len() {
            return Err(ProtocolError::UnexpectedEof);
        }
        let bytes = &self.data[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.bytes(1)?[0])
    }

    fn bool(&mut self) -> Result<bool, ProtocolError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(ProtocolError::InvalidBool(value)),
        }
    }

    fn u16(&mut self) -> Result<u16, ProtocolError> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn u128(&mut self) -> Result<u128, ProtocolError> {
        let bytes = self.bytes(16)?;
        Ok(u128::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]))
    }

    fn string(&mut self) -> Result<String, ProtocolError> {
        let len = self.u32()? as usize;
        let bytes = self.bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
    }
}

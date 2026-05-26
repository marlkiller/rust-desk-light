use std::io::{self, ErrorKind, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

mod binary;
mod error;
pub use error::ProtocolError;

use crate::binary::{BinaryReader, BinaryWriter};

pub const DEFAULT_SERVER_IP: &str = "127.0.0.1";
pub const DEFAULT_SERVER_PORT: u16 = 5169;
pub const PROTOCOL_VERSION: u16 = 3;
pub const FRAME_MAGIC: [u8; 4] = *b"RDL1";
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;
pub const REMOTE_TERMINAL_CANCEL: &str = "__rdl_terminal_cancel";
pub const TEMP_UPDATE_PATH_PREFIX: &str = "rdl-temp://";

const HEADER_LEN: usize = 10;
const ENVELOPE_FIXED_LEN: usize = 27;

pub mod audio_udp {
    use super::ProtocolError;
    pub const MAGIC: [u8; 4] = *b"RDU1";
    pub const FORMAT_PCM_S16LE: u8 = 1;
    pub const MAX_PACKET_BYTES: usize = 1400;

    const TYPE_REGISTER: u8 = 1;
    const TYPE_UNREGISTER: u8 = 2;
    const TYPE_AUDIO: u8 = 3;
    const CONTROL_LEN: usize = 4 + 1 + 8;
    const AUDIO_HEADER_LEN: usize = 4 + 1 + 8 + 8 + 8 + 4 + 2 + 1;

    #[derive(Debug)]
    pub enum Packet<'a> {
        Register {
            stream_id: u64,
        },
        Unregister {
            stream_id: u64,
        },
        Audio {
            stream_id: u64,
            seq: u64,
            capture_epoch_ms: u64,
            sample_rate: u32,
            channels: u16,
            format: &'static str,
            bytes: &'a [u8],
        },
    }

    pub fn encode_register(stream_id: u64, out: &mut Vec<u8>) {
        encode_control(TYPE_REGISTER, stream_id, out);
    }

    pub fn encode_unregister(stream_id: u64, out: &mut Vec<u8>) {
        encode_control(TYPE_UNREGISTER, stream_id, out);
    }

    pub fn encode_audio(
        stream_id: u64,
        seq: u64,
        capture_epoch_ms: u64,
        sample_rate: u32,
        channels: u16,
        format: &str,
        bytes: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), ProtocolError> {
        if format_code(format).is_none() {
            return Err(ProtocolError::AudioUdpProtocol("unsupported audio udp format"));
        }
        if AUDIO_HEADER_LEN + bytes.len() > MAX_PACKET_BYTES {
            return Err(ProtocolError::AudioUdpProtocol("audio udp packet too large"));
        }
        out.clear();
        out.extend_from_slice(&MAGIC);
        out.push(TYPE_AUDIO);
        out.extend_from_slice(&stream_id.to_be_bytes());
        out.extend_from_slice(&seq.to_be_bytes());
        out.extend_from_slice(&capture_epoch_ms.to_be_bytes());
        out.extend_from_slice(&sample_rate.to_be_bytes());
        out.extend_from_slice(&channels.to_be_bytes());
        out.push(format_code(format).unwrap_or(FORMAT_PCM_S16LE));
        out.extend_from_slice(bytes);
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Packet<'_>, ProtocolError> {
        if bytes.len() < CONTROL_LEN || bytes[..4] != MAGIC {
            return Err(ProtocolError::AudioUdpProtocol("invalid audio udp packet"));
        }
        let packet_type = bytes[4];
        let stream_id = read_u64(bytes, 5)?;
        match packet_type {
            TYPE_REGISTER => Ok(Packet::Register { stream_id }),
            TYPE_UNREGISTER => Ok(Packet::Unregister { stream_id }),
            TYPE_AUDIO => {
                if bytes.len() < AUDIO_HEADER_LEN {
                    return Err(ProtocolError::AudioUdpProtocol("truncated audio udp packet"));
                }
                let seq = read_u64(bytes, 13)?;
                let capture_epoch_ms = read_u64(bytes, 21)?;
                let sample_rate = read_u32(bytes, 29)?;
                let channels = read_u16(bytes, 33)?;
                let format = format_name(bytes.get(35).copied().ok_or(ProtocolError::AudioUdpProtocol("missing audio udp format"))?)
                    .ok_or(ProtocolError::AudioUdpProtocol("unsupported audio udp format"))?;
                Ok(Packet::Audio {
                    stream_id,
                    seq,
                    capture_epoch_ms,
                    sample_rate,
                    channels,
                    format,
                    bytes: &bytes[AUDIO_HEADER_LEN..],
                })
            }
            _ => Err(ProtocolError::AudioUdpProtocol("unknown audio udp packet type")),
        }
    }

    fn encode_control(packet_type: u8, stream_id: u64, out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(&MAGIC);
        out.push(packet_type);
        out.extend_from_slice(&stream_id.to_be_bytes());
    }

    fn format_code(format: &str) -> Option<u8> {
        match format {
            "pcm_s16le" => Some(FORMAT_PCM_S16LE),
            _ => None,
        }
    }

    fn format_name(code: u8) -> Option<&'static str> {
        match code {
            FORMAT_PCM_S16LE => Some("pcm_s16le"),
            _ => None,
        }
    }

    fn read_u16(bytes: &[u8], start: usize) -> Result<u16, ProtocolError> {
        let raw = bytes
            .get(start..start + 2)
            .ok_or(ProtocolError::AudioUdpProtocol("truncated audio udp u16"))?;
        Ok(u16::from_be_bytes([raw[0], raw[1]]))
    }

    fn read_u32(bytes: &[u8], start: usize) -> Result<u32, ProtocolError> {
        let raw = bytes
            .get(start..start + 4)
            .ok_or(ProtocolError::AudioUdpProtocol("truncated audio udp u32"))?;
        Ok(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    fn read_u64(bytes: &[u8], start: usize) -> Result<u64, ProtocolError> {
        let raw = bytes
            .get(start..start + 8)
            .ok_or(ProtocolError::AudioUdpProtocol("truncated audio udp u64"))?;
        Ok(u64::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ]))
    }
}

pub mod p2p_udp {
    use super::Role;
    use crate::error::ProtocolError;

    pub const MAGIC: [u8; 4] = *b"RDP1";
    pub const VERSION: u8 = 1;
    pub const MAX_PACKET_BYTES: usize = 64;

    const TYPE_REGISTER: u8 = 1;
    const TYPE_PROBE: u8 = 2;
    const TYPE_ACK: u8 = 3;
    const PACKET_LEN: usize = 4 + 1 + 1 + 1 + 8 + 8 + 8 + 16;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Packet {
        Register {
            role: Role,
            session_id: u64,
            nonce: u64,
        },
        Probe {
            role: Role,
            session_id: u64,
            nonce: u64,
            sequence: u64,
            sent_epoch_ms: u128,
        },
        Ack {
            role: Role,
            session_id: u64,
            nonce: u64,
            sequence: u64,
            sent_epoch_ms: u128,
        },
    }

    pub fn encode_register(role: Role, session_id: u64, nonce: u64, out: &mut Vec<u8>) {
        encode(TYPE_REGISTER, role, session_id, nonce, 0, 0, out);
    }

    pub fn encode_probe(
        role: Role,
        session_id: u64,
        nonce: u64,
        sequence: u64,
        sent_epoch_ms: u128,
        out: &mut Vec<u8>,
    ) {
        encode(
            TYPE_PROBE,
            role,
            session_id,
            nonce,
            sequence,
            sent_epoch_ms,
            out,
        );
    }

    pub fn encode_ack(
        role: Role,
        session_id: u64,
        nonce: u64,
        sequence: u64,
        sent_epoch_ms: u128,
        out: &mut Vec<u8>,
    ) {
        encode(
            TYPE_ACK,
            role,
            session_id,
            nonce,
            sequence,
            sent_epoch_ms,
            out,
        );
    }

    pub fn decode(bytes: &[u8]) -> Result<Packet, ProtocolError> {
        if bytes.len() != PACKET_LEN || bytes[..4] != MAGIC {
            return Err(ProtocolError::P2pUdpProtocol("invalid p2p udp packet"));
        }
        if bytes[4] != VERSION {
            return Err(ProtocolError::P2pUdpProtocol("unsupported p2p udp version"));
        }
        let role = role_from_code(bytes[6]).ok_or(ProtocolError::InvalidRole)?;
        let session_id = read_u64(bytes, 7)?;
        let nonce = read_u64(bytes, 15)?;
        let sequence = read_u64(bytes, 23)?;
        let sent_epoch_ms = read_u128(bytes, 31)?;
        match bytes[5] {
            TYPE_REGISTER => Ok(Packet::Register {
                role,
                session_id,
                nonce,
            }),
            TYPE_PROBE => Ok(Packet::Probe {
                role,
                session_id,
                nonce,
                sequence,
                sent_epoch_ms,
            }),
            TYPE_ACK => Ok(Packet::Ack {
                role,
                session_id,
                nonce,
                sequence,
                sent_epoch_ms,
            }),
            _ => Err(ProtocolError::P2pUdpProtocol("unknown p2p udp packet type")),
        }
    }

    fn encode(
        packet_type: u8,
        role: Role,
        session_id: u64,
        nonce: u64,
        sequence: u64,
        sent_epoch_ms: u128,
        out: &mut Vec<u8>,
    ) {
        out.clear();
        out.reserve(PACKET_LEN);
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(packet_type);
        out.push(role_code(&role));
        out.extend_from_slice(&session_id.to_be_bytes());
        out.extend_from_slice(&nonce.to_be_bytes());
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(&sent_epoch_ms.to_be_bytes());
    }

    fn role_code(role: &Role) -> u8 {
        match role {
            Role::Client => 1,
            Role::Admin => 2,
            Role::Server => 3,
        }
    }

    fn role_from_code(value: u8) -> Option<Role> {
        match value {
            1 => Some(Role::Client),
            2 => Some(Role::Admin),
            3 => Some(Role::Server),
            _ => None,
        }
    }

    fn read_u64(bytes: &[u8], start: usize) -> Result<u64, ProtocolError> {
        let raw = bytes.get(start..start + 8).ok_or(ProtocolError::P2pUdpProtocol("truncated p2p udp u64"))?;
        Ok(u64::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ]))
    }

    fn read_u128(bytes: &[u8], start: usize) -> Result<u128, ProtocolError> {
        let raw = bytes
            .get(start..start + 16)
            .ok_or(ProtocolError::P2pUdpProtocol("truncated p2p udp u128"))?;
        Ok(u128::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7], raw[8], raw[9],
            raw[10], raw[11], raw[12], raw[13], raw[14], raw[15],
        ]))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticCommandPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub windows: &'static str,
    pub unix: &'static str,
}

pub const STATIC_COMMAND_PRESETS: &[StaticCommandPreset] = &[
    StaticCommandPreset {
        id: "whoami",
        label: "Who Am I",
        windows: "whoami",
        unix: "whoami",
    },
    StaticCommandPreset {
        id: "hostname",
        label: "Hostname",
        windows: "hostname",
        unix: "hostname",
    },
    StaticCommandPreset {
        id: "uptime",
        label: "Uptime",
        windows: "Get-CimInstance Win32_OperatingSystem | Select-Object LastBootUpTime,LocalDateTime | Format-List",
        unix: "uptime",
    },
    StaticCommandPreset {
        id: "disk_usage",
        label: "Disk Usage",
        windows: "Get-PSDrive -PSProvider FileSystem | Select-Object Name,Used,Free,Root | Format-Table -AutoSize",
        unix: "df -h",
    },
    StaticCommandPreset {
        id: "network_config",
        label: "Network Config",
        windows: "ipconfig",
        unix: "ifconfig 2>/dev/null || ip addr",
    },
    StaticCommandPreset {
        id: "environment",
        label: "Environment",
        windows: "Get-ChildItem Env: | Sort-Object Name | Format-Table -AutoSize",
        unix: "env | sort",
    },
];

pub fn static_command_presets() -> &'static [StaticCommandPreset] {
    STATIC_COMMAND_PRESETS
}

pub fn default_static_command_preset_id() -> &'static str {
    "whoami"
}

pub fn static_command_preset(id: &str) -> Option<&'static StaticCommandPreset> {
    STATIC_COMMAND_PRESETS.iter().find(|preset| preset.id == id)
}

pub fn static_command_preset_label(id: &str) -> &'static str {
    static_command_preset(id)
        .map(|preset| preset.label)
        .unwrap_or("Who Am I")
}

pub fn static_command_script_for_os(id: &str, os_label: &str) -> Option<&'static str> {
    static_command_preset(id).map(|preset| {
        if os_label.to_ascii_lowercase().contains("windows") {
            preset.windows
        } else {
            preset.unix
        }
    })
}

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
    ClientConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VideoSource {
    RemoteDesktop,
    Camera,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioSource {
    AudioListen,
    VoiceChat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandOutputStream {
    Stdout,
    Stderr,
    Status,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileTransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileTransferAction {
    Start,
    Directory,
    Chunk,
    Finish,
    Cancel,
    Progress,
    Complete,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum P2pAction {
    Start,
    Stop,
    ServerReady,
    PeerReady,
    Error,
}

impl P2pAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::ServerReady => "server_ready",
            Self::PeerReady => "peer_ready",
            Self::Error => "error",
        }
    }

    fn to_code(&self) -> u8 {
        match self {
            Self::Start => 1,
            Self::Stop => 2,
            Self::ServerReady => 3,
            Self::PeerReady => 4,
            Self::Error => 5,
        }
    }

    fn from_code(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Start),
            2 => Ok(Self::Stop),
            3 => Ok(Self::ServerReady),
            4 => Ok(Self::PeerReady),
            5 => Ok(Self::Error),
            _ => Err(ProtocolError::InvalidP2pAction),
        }
    }
}

impl VideoSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RemoteDesktop => "remote_desktop",
            Self::Camera => "camera",
        }
    }

    fn to_code(&self) -> u8 {
        match self {
            Self::RemoteDesktop => 1,
            Self::Camera => 2,
        }
    }

    fn from_code(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::RemoteDesktop),
            2 => Ok(Self::Camera),
            _ => Err(ProtocolError::InvalidVideoSource),
        }
    }
}

impl AudioSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AudioListen => "audio_listen",
            Self::VoiceChat => "voice_chat",
        }
    }

    fn to_code(&self) -> u8 {
        match self {
            Self::AudioListen => 1,
            Self::VoiceChat => 2,
        }
    }

    fn from_code(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::AudioListen),
            2 => Ok(Self::VoiceChat),
            _ => Err(ProtocolError::InvalidAudioSource),
        }
    }
}

impl CommandKind {
    pub fn requires_client_gui(&self) -> bool {
        matches!(self, Self::TextChat | Self::VoiceChat)
    }

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
            Self::ClientConfig => "client_config",
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
            "client_config" => Self::ClientConfig,
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
            Self::ClientConfig => 37,
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
            37 => Self::ClientConfig,
            _ => return Err(ProtocolError::InvalidCommand),
        })
    }
}

impl CommandOutputStream {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Status => "status",
        }
    }

    fn to_code(self) -> u8 {
        match self {
            Self::Stdout => 1,
            Self::Stderr => 2,
            Self::Status => 3,
        }
    }

    fn from_code(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Stdout),
            2 => Ok(Self::Stderr),
            3 => Ok(Self::Status),
            _ => Err(ProtocolError::InvalidCommandOutputStream),
        }
    }
}

impl FileTransferDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }

    fn to_code(self) -> u8 {
        match self {
            Self::Upload => 1,
            Self::Download => 2,
        }
    }

    fn from_code(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Upload),
            2 => Ok(Self::Download),
            _ => Err(ProtocolError::InvalidFileTransferDirection),
        }
    }
}

impl FileTransferAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Directory => "directory",
            Self::Chunk => "chunk",
            Self::Finish => "finish",
            Self::Cancel => "cancel",
            Self::Progress => "progress",
            Self::Complete => "complete",
            Self::Error => "error",
        }
    }

    fn to_code(self) -> u8 {
        match self {
            Self::Start => 1,
            Self::Directory => 2,
            Self::Chunk => 3,
            Self::Finish => 4,
            Self::Cancel => 5,
            Self::Progress => 6,
            Self::Complete => 7,
            Self::Error => 8,
        }
    }

    fn from_code(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Start),
            2 => Ok(Self::Directory),
            3 => Ok(Self::Chunk),
            4 => Ok(Self::Finish),
            5 => Ok(Self::Cancel),
            6 => Ok(Self::Progress),
            7 => Ok(Self::Complete),
            8 => Ok(Self::Error),
            _ => Err(ProtocolError::InvalidFileTransferAction),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientLocation {
    pub latitude_e7: i32,
    pub longitude_e7: i32,
    pub accuracy_meters: u32,
    pub source: String,
    pub label: String,
    pub updated_at_epoch_ms: u128,
}

impl ClientLocation {
    pub const SCALE: f64 = 10_000_000.0;

    pub fn from_degrees(
        latitude: f64,
        longitude: f64,
        accuracy_meters: u32,
        source: impl Into<String>,
        label: impl Into<String>,
        updated_at_epoch_ms: u128,
    ) -> Self {
        Self {
            latitude_e7: (latitude * Self::SCALE).round() as i32,
            longitude_e7: (longitude * Self::SCALE).round() as i32,
            accuracy_meters,
            source: source.into(),
            label: label.into(),
            updated_at_epoch_ms,
        }
    }

    pub fn latitude(&self) -> f64 {
        self.latitude_e7 as f64 / Self::SCALE
    }

    pub fn longitude(&self) -> f64 {
        self.longitude_e7 as f64 / Self::SCALE
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientInfo {
    pub id: String,
    pub fingerprint: String,
    pub peer_addr: String,
    pub hostname: String,
    pub os: String,
    pub username: String,
    pub gui_available: bool,
    pub started_at_epoch_ms: u128,
    pub last_seen_epoch_ms: u128,
    pub location: Option<ClientLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Hello {
        role: Role,
        auth_token: String,
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
    CommandOutput {
        client_id: String,
        command: CommandKind,
        stream_id: u64,
        sequence: u64,
        stream: CommandOutputStream,
        chunk: String,
        current_dir: String,
        finished: bool,
        success: bool,
    },
    FileTransfer {
        target_id: String,
        transfer_id: u64,
        direction: FileTransferDirection,
        action: FileTransferAction,
        path: String,
        relative_path: String,
        total_bytes: u64,
        transferred_bytes: u64,
        file_size: u64,
        offset: u64,
        bytes: Vec<u8>,
        message: String,
    },
    DesktopControl {
        target_id: String,
        payload: String,
    },
    DesktopInput {
        target_id: String,
        payload: String,
    },
    DesktopFrame {
        client_id: String,
        payload: String,
    },
    VideoControl {
        target_id: String,
        source: VideoSource,
        payload: String,
    },
    VideoFrame {
        client_id: String,
        source: VideoSource,
        seq: u64,
        source_width: u32,
        source_height: u32,
        image_width: u32,
        image_height: u32,
        format: String,
        bytes: Vec<u8>,
    },
    AudioControl {
        target_id: String,
        source: AudioSource,
        payload: String,
    },
    ProxyOpen {
        target_id: String,
        stream_id: u64,
        host: String,
        port: u16,
    },
    ProxyOpenResult {
        client_id: String,
        stream_id: u64,
        accepted: bool,
        detail: String,
    },
    ProxyData {
        client_id: String,
        stream_id: u64,
        bytes: Vec<u8>,
    },
    ProxyClose {
        client_id: String,
        stream_id: u64,
        reason: String,
    },
    P2pControl {
        target_id: String,
        session_id: u64,
        nonce: u64,
        action: P2pAction,
        server_udp_addr: String,
        peer_udp_addr: String,
        detail: String,
    },
    P2pResult {
        client_id: String,
        session_id: u64,
        success: bool,
        finished: bool,
        endpoint: String,
        rtt_ms: u32,
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
            Self::DesktopControl { .. } => 10,
            Self::DesktopInput { .. } => 11,
            Self::DesktopFrame { .. } => 12,
            Self::VideoControl { .. } => 13,
            Self::VideoFrame { .. } => 14,
            Self::CommandOutput { .. } => 15,
            Self::FileTransfer { .. } => 16,
            Self::AudioControl { .. } => 17,
            Self::ProxyOpen { .. } => 18,
            Self::ProxyOpenResult { .. } => 19,
            Self::ProxyData { .. } => 20,
            Self::ProxyClose { .. } => 21,
            Self::P2pControl { .. } => 22,
            Self::P2pResult { .. } => 23,
        }
    }

    fn encode_payload(&self, writer: &mut BinaryWriter) {
        match self {
            Self::Hello {
                role,
                auth_token,
                id,
                fingerprint,
                hostname,
                os,
                username,
                gui_available,
            } => {
                writer.u8(role.to_code());
                writer.string(auth_token);
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
                    writer.string(&client.peer_addr);
                    writer.string(&client.hostname);
                    writer.string(&client.os);
                    writer.string(&client.username);
                    writer.bool(client.gui_available);
                    writer.u128(client.started_at_epoch_ms);
                    writer.u128(client.last_seen_epoch_ms);
                    writer.bool(client.location.is_some());
                    if let Some(location) = client.location.as_ref() {
                        writer.i32(location.latitude_e7);
                        writer.i32(location.longitude_e7);
                        writer.u32(location.accuracy_meters);
                        writer.string(&location.source);
                        writer.string(&location.label);
                        writer.u128(location.updated_at_epoch_ms);
                    }
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
            Self::CommandOutput {
                client_id,
                command,
                stream_id,
                sequence,
                stream,
                chunk,
                current_dir,
                finished,
                success,
            } => {
                writer.string(client_id);
                writer.u16(command.to_code());
                writer.u64(*stream_id);
                writer.u64(*sequence);
                writer.u8(stream.to_code());
                writer.string(chunk);
                writer.string(current_dir);
                writer.bool(*finished);
                writer.bool(*success);
            }
            Self::FileTransfer {
                target_id,
                transfer_id,
                direction,
                action,
                path,
                relative_path,
                total_bytes,
                transferred_bytes,
                file_size,
                offset,
                bytes,
                message,
            } => {
                writer.string(target_id);
                writer.u64(*transfer_id);
                writer.u8(direction.to_code());
                writer.u8(action.to_code());
                writer.string(path);
                writer.string(relative_path);
                writer.u64(*total_bytes);
                writer.u64(*transferred_bytes);
                writer.u64(*file_size);
                writer.u64(*offset);
                writer.byte_vec(bytes);
                writer.string(message);
            }
            Self::DesktopControl { target_id, payload }
            | Self::DesktopInput { target_id, payload } => {
                writer.string(target_id);
                writer.string(payload);
            }
            Self::DesktopFrame { client_id, payload } => {
                writer.string(client_id);
                writer.string(payload);
            }
            Self::VideoControl {
                target_id,
                source,
                payload,
            } => {
                writer.string(target_id);
                writer.u8(source.to_code());
                writer.string(payload);
            }
            Self::VideoFrame {
                client_id,
                source,
                seq,
                source_width,
                source_height,
                image_width,
                image_height,
                format,
                bytes,
            } => {
                writer.string(client_id);
                writer.u8(source.to_code());
                writer.u64(*seq);
                writer.u32(*source_width);
                writer.u32(*source_height);
                writer.u32(*image_width);
                writer.u32(*image_height);
                writer.string(format);
                writer.byte_vec(bytes);
            }
            Self::AudioControl {
                target_id,
                source,
                payload,
            } => {
                writer.string(target_id);
                writer.u8(source.to_code());
                writer.string(payload);
            }
            Self::ProxyOpen {
                target_id,
                stream_id,
                host,
                port,
            } => {
                writer.string(target_id);
                writer.u64(*stream_id);
                writer.string(host);
                writer.u16(*port);
            }
            Self::ProxyOpenResult {
                client_id,
                stream_id,
                accepted,
                detail,
            } => {
                writer.string(client_id);
                writer.u64(*stream_id);
                writer.bool(*accepted);
                writer.string(detail);
            }
            Self::ProxyData {
                client_id,
                stream_id,
                bytes,
            } => {
                writer.string(client_id);
                writer.u64(*stream_id);
                writer.byte_vec(bytes);
            }
            Self::ProxyClose {
                client_id,
                stream_id,
                reason,
            } => {
                writer.string(client_id);
                writer.u64(*stream_id);
                writer.string(reason);
            }
            Self::P2pControl {
                target_id,
                session_id,
                nonce,
                action,
                server_udp_addr,
                peer_udp_addr,
                detail,
            } => {
                writer.string(target_id);
                writer.u64(*session_id);
                writer.u64(*nonce);
                writer.u8(action.to_code());
                writer.string(server_udp_addr);
                writer.string(peer_udp_addr);
                writer.string(detail);
            }
            Self::P2pResult {
                client_id,
                session_id,
                success,
                finished,
                endpoint,
                rtt_ms,
                detail,
            } => {
                writer.string(client_id);
                writer.u64(*session_id);
                writer.bool(*success);
                writer.bool(*finished);
                writer.string(endpoint);
                writer.u32(*rtt_ms);
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
                auth_token: reader.string()?,
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
                    let id = reader.string()?;
                    let fingerprint = reader.string()?;
                    let peer_addr = reader.string()?;
                    let hostname = reader.string()?;
                    let os = reader.string()?;
                    let username = reader.string()?;
                    let gui_available = reader.bool()?;
                    let started_at_epoch_ms = reader.u128()?;
                    let last_seen_epoch_ms = reader.u128()?;
                    let location = if reader.bool()? {
                        Some(ClientLocation {
                            latitude_e7: reader.i32()?,
                            longitude_e7: reader.i32()?,
                            accuracy_meters: reader.u32()?,
                            source: reader.string()?,
                            label: reader.string()?,
                            updated_at_epoch_ms: reader.u128()?,
                        })
                    } else {
                        None
                    };
                    clients.push(ClientInfo {
                        id,
                        fingerprint,
                        peer_addr,
                        hostname,
                        os,
                        username,
                        gui_available,
                        started_at_epoch_ms,
                        last_seen_epoch_ms,
                        location,
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
            10 => Self::DesktopControl {
                target_id: reader.string()?,
                payload: reader.string()?,
            },
            11 => Self::DesktopInput {
                target_id: reader.string()?,
                payload: reader.string()?,
            },
            12 => Self::DesktopFrame {
                client_id: reader.string()?,
                payload: reader.string()?,
            },
            13 => Self::VideoControl {
                target_id: reader.string()?,
                source: VideoSource::from_code(reader.u8()?)?,
                payload: reader.string()?,
            },
            14 => Self::VideoFrame {
                client_id: reader.string()?,
                source: VideoSource::from_code(reader.u8()?)?,
                seq: reader.u64()?,
                source_width: reader.u32()?,
                source_height: reader.u32()?,
                image_width: reader.u32()?,
                image_height: reader.u32()?,
                format: reader.string()?,
                bytes: reader.byte_vec()?,
            },
            15 => Self::CommandOutput {
                client_id: reader.string()?,
                command: CommandKind::from_code(reader.u16()?)?,
                stream_id: reader.u64()?,
                sequence: reader.u64()?,
                stream: CommandOutputStream::from_code(reader.u8()?)?,
                chunk: reader.string()?,
                current_dir: reader.string()?,
                finished: reader.bool()?,
                success: reader.bool()?,
            },
            16 => Self::FileTransfer {
                target_id: reader.string()?,
                transfer_id: reader.u64()?,
                direction: FileTransferDirection::from_code(reader.u8()?)?,
                action: FileTransferAction::from_code(reader.u8()?)?,
                path: reader.string()?,
                relative_path: reader.string()?,
                total_bytes: reader.u64()?,
                transferred_bytes: reader.u64()?,
                file_size: reader.u64()?,
                offset: reader.u64()?,
                bytes: reader.byte_vec()?,
                message: reader.string()?,
            },
            17 => Self::AudioControl {
                target_id: reader.string()?,
                source: AudioSource::from_code(reader.u8()?)?,
                payload: reader.string()?,
            },
            18 => Self::ProxyOpen {
                target_id: reader.string()?,
                stream_id: reader.u64()?,
                host: reader.string()?,
                port: reader.u16()?,
            },
            19 => Self::ProxyOpenResult {
                client_id: reader.string()?,
                stream_id: reader.u64()?,
                accepted: reader.bool()?,
                detail: reader.string()?,
            },
            20 => Self::ProxyData {
                client_id: reader.string()?,
                stream_id: reader.u64()?,
                bytes: reader.byte_vec()?,
            },
            21 => Self::ProxyClose {
                client_id: reader.string()?,
                stream_id: reader.u64()?,
                reason: reader.string()?,
            },
            22 => Self::P2pControl {
                target_id: reader.string()?,
                session_id: reader.u64()?,
                nonce: reader.u64()?,
                action: P2pAction::from_code(reader.u8()?)?,
                server_udp_addr: reader.string()?,
                peer_udp_addr: reader.string()?,
                detail: reader.string()?,
            },
            23 => Self::P2pResult {
                client_id: reader.string()?,
                session_id: reader.u64()?,
                success: reader.bool()?,
                finished: reader.bool()?,
                endpoint: reader.string()?,
                rtt_ms: reader.u32()?,
                detail: reader.string()?,
            },
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
    let message_kind = envelope.message.kind_code();

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
    frame.extend_from_slice(&message_kind.to_be_bytes());
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

pub fn write_envelope_with_version(
    writer: &mut impl Write,
    version: u16,
    role: Role,
    message_id: u64,
    correlation_id: Option<u64>,
    message: Message,
) -> io::Result<()> {
    write_envelope_with_token_and_version(
        writer,
        version,
        role,
        message_id,
        correlation_id,
        "",
        message,
    )
}

pub fn write_envelope_with_token(
    writer: &mut impl Write,
    role: Role,
    message_id: u64,
    correlation_id: Option<u64>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    write_envelope_with_token_and_version(
        writer,
        PROTOCOL_VERSION,
        role,
        message_id,
        correlation_id,
        session_token,
        message,
    )
}

pub fn write_envelope_with_token_and_version(
    writer: &mut impl Write,
    version: u16,
    role: Role,
    message_id: u64,
    correlation_id: Option<u64>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    let envelope = Envelope {
        version,
        message_id,
        correlation_id,
        role,
        session_token: session_token.to_string(),
        message,
    };
    let frame = encode_envelope(&envelope).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    writer.write_all(&frame)
}

pub fn read_envelope(reader: &mut impl Read) -> io::Result<Envelope> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header)?;
    if header[0..4] != FRAME_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, ProtocolError::InvalidMagic));
    }
    let remaining_len = u32::from_be_bytes([header[6], header[7], header[8], header[9]]);
    if remaining_len > MAX_FRAME_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, ProtocolError::FrameTooLarge));
    }

    let mut frame = Vec::with_capacity(HEADER_LEN + remaining_len as usize);
    frame.extend_from_slice(&header);
    frame.resize(HEADER_LEN + remaining_len as usize, 0);
    reader.read_exact(&mut frame[HEADER_LEN..])?;
    decode_envelope(&frame).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub struct EnvelopeDecoder {
    buffer: Vec<u8>,
}

impl EnvelopeDecoder {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(64 * 1024),
        }
    }

    pub fn read_next(&mut self, reader: &mut impl Read) -> io::Result<Option<Envelope>> {
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(ErrorKind::UnexpectedEof, "peer closed")),
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        self.try_decode_one()
    }

    fn try_decode_one(&mut self) -> io::Result<Option<Envelope>> {
        if self.buffer.len() < HEADER_LEN {
            return Ok(None);
        }
        if self.buffer[0..4] != FRAME_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, ProtocolError::InvalidMagic));
        }
        let remaining_len = u32::from_be_bytes([
            self.buffer[6],
            self.buffer[7],
            self.buffer[8],
            self.buffer[9],
        ]);
        if remaining_len > MAX_FRAME_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, ProtocolError::FrameTooLarge));
        }
        let frame_len = HEADER_LEN + remaining_len as usize;
        if self.buffer.len() < frame_len {
            return Ok(None);
        }
        let frame = self.buffer[..frame_len].to_vec();
        self.buffer.drain(..frame_len);
        decode_envelope(&frame).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

impl Default for EnvelopeDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_PROTOCOL_VERSION: u16 = 2;

    #[test]
    fn accepts_v2_hello_and_writes_v2_session() {
        let hello = Envelope {
            version: OLD_PROTOCOL_VERSION,
            message_id: 1,
            correlation_id: None,
            role: Role::Client,
            session_token: String::new(),
            message: Message::Hello {
                role: Role::Client,
                auth_token: "token".to_string(),
                id: "client-1".to_string(),
                fingerprint: "fp".to_string(),
                hostname: "host".to_string(),
                os: "windows".to_string(),
                username: "user".to_string(),
                gui_available: true,
            },
        };

        let frame = encode_envelope(&hello).expect("v2 hello should encode");
        let decoded = decode_envelope(&frame).expect("v2 hello should decode");
        assert_eq!(decoded.version, OLD_PROTOCOL_VERSION);
        assert_eq!(decoded.message, hello.message);

        let mut response = Vec::new();
        write_envelope_with_version(
            &mut response,
            OLD_PROTOCOL_VERSION,
            Role::Server,
            1,
            None,
            Message::Session {
                token: "session".to_string(),
            },
        )
        .expect("v2 session should write");
        let decoded_response = decode_envelope(&response).expect("v2 session should decode");
        assert_eq!(decoded_response.version, OLD_PROTOCOL_VERSION);
        assert_eq!(
            decoded_response.message,
            Message::Session {
                token: "session".to_string()
            }
        );
    }
}

pub fn now_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}


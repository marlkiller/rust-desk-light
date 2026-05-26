use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidMagic,
    InvalidFrameLength,
    TruncatedFrame,
    FrameTooLarge,
    InvalidRole,
    InvalidCommand,
    InvalidVideoSource,
    InvalidAudioSource,
    InvalidCommandOutputStream,
    InvalidFileTransferDirection,
    InvalidFileTransferAction,
    InvalidP2pAction,
    InvalidMessageKind(u16),
    InvalidBool(u8),
    InvalidUtf8,
    TrailingBytes(usize),
    UnexpectedEof,
    AudioUdpProtocol(&'static str),
    P2pUdpProtocol(&'static str),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "invalid frame magic"),
            Self::InvalidFrameLength => write!(f, "invalid frame length"),
            Self::TruncatedFrame => write!(f, "truncated frame"),
            Self::FrameTooLarge => write!(f, "frame too large"),
            Self::InvalidRole => write!(f, "invalid role"),
            Self::InvalidCommand => write!(f, "invalid command"),
            Self::InvalidVideoSource => write!(f, "invalid video source"),
            Self::InvalidAudioSource => write!(f, "invalid audio source"),
            Self::InvalidCommandOutputStream => write!(f, "invalid command output stream"),
            Self::InvalidFileTransferDirection => write!(f, "invalid file transfer direction"),
            Self::InvalidFileTransferAction => write!(f, "invalid file transfer action"),
            Self::InvalidP2pAction => write!(f, "invalid p2p action"),
            Self::InvalidMessageKind(kind) => write!(f, "invalid message kind: {kind}"),
            Self::InvalidBool(value) => write!(f, "invalid bool byte: {value}"),
            Self::InvalidUtf8 => write!(f, "invalid utf-8 string"),
            Self::TrailingBytes(count) => write!(f, "payload has {count} trailing bytes"),
            Self::UnexpectedEof => write!(f, "unexpected end of payload"),
            Self::AudioUdpProtocol(msg) => write!(f, "audio udp protocol error: {msg}"),
            Self::P2pUdpProtocol(msg) => write!(f, "p2p udp error: {msg}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

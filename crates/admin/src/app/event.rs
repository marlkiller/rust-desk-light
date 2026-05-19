use crate::live_control;
use eframe::egui;
use rdl_protocol::{
    AudioSource, ClientInfo, CommandKind, CommandOutputStream, Message, VideoSource,
};
use std::sync::{mpsc::Sender, Arc, Mutex};

#[derive(Clone)]
pub(crate) struct ReconnectEndpoint {
    pub(super) ip: String,
    pub(super) port: u16,
    pub(super) auth_token: String,
}

pub(crate) enum AdminInput {
    Command {
        target_id: String,
        command: CommandKind,
        payload: String,
    },
    DesktopControl {
        target_id: String,
        payload: String,
    },
    DesktopInput {
        target_id: String,
        payload: String,
    },
    VideoControl {
        target_id: String,
        source: VideoSource,
        payload: String,
    },
    AudioControl {
        target_id: String,
        source: AudioSource,
        payload: String,
    },
    FileTransfer(Message),
    Proxy(Message),
    Reconnect {
        reason: String,
        endpoint: Option<ReconnectEndpoint>,
    },
}

pub(super) enum AdminEvent {
    Connected,
    Disconnected,
    ConnectionFailed {
        ip: String,
        port: u16,
        auth_token: String,
        detail: String,
    },
    AuthTokenRequired,
    AuthTokenRejected(String),
    Clients(Vec<ClientInfo>),
    Ack {
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
    DesktopFrame {
        client_id: String,
        payload: String,
    },
    DecodedDesktopFrame {
        client_id: String,
        result: Result<live_control::remote_desktop::DesktopFrame, String>,
        decode_ms: Option<u128>,
    },
    DecodedCameraFrame {
        client_id: String,
        result: Result<live_control::camera::CameraFrame, String>,
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
    AudioFrame {
        client_id: String,
        source: AudioSource,
        seq: u64,
        sample_rate: u32,
        channels: u16,
        format: String,
        bytes: Vec<u8>,
    },
    FileTransfer(Message),
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
    Log(String),
}

#[derive(Clone)]
pub(super) struct AdminEventSink {
    tx: Sender<AdminEvent>,
    repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
    audio_playback_registry: Option<live_control::audio_listen::AudioPlaybackRegistry>,
}

impl AdminEventSink {
    pub(super) fn new(
        tx: Sender<AdminEvent>,
        repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
        audio_playback_registry: Option<live_control::audio_listen::AudioPlaybackRegistry>,
    ) -> Self {
        Self {
            tx,
            repaint_handle,
            audio_playback_registry,
        }
    }

    pub(super) fn send(&self, event: AdminEvent) {
        if let (
            Some(registry),
            AdminEvent::AudioFrame {
                client_id,
                source: AudioSource::AudioListen,
                seq,
                sample_rate,
                channels,
                format,
                bytes,
            },
        ) = (&self.audio_playback_registry, &event)
        {
            registry.push_frame(client_id, *seq, *sample_rate, *channels, format, bytes);
        }
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

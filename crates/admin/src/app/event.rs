use super::video_pipeline::{PendingVideoFrame, VideoFrameCoalescer};
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
    ListClients,
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
    P2p(Message),
    Reconnect {
        reason: String,
        endpoint: Option<ReconnectEndpoint>,
    },
}

pub(crate) enum AdminEvent {
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
        decode_ms: Option<u128>,
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
    VideoFrameReady {
        client_id: String,
        source: VideoSource,
        coalescer: Arc<VideoFrameCoalescer>,
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
    P2pControl {
        target_id: String,
        session_id: u64,
        nonce: u64,
        action: rdl_protocol::P2pAction,
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
    Log(String),
}

#[derive(Clone)]
pub(super) struct AdminEventSink {
    tx: Sender<AdminEvent>,
    repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
    audio_playback_registry: Option<live_control::audio_listen::AudioPlaybackRegistry>,
    video_frame_coalescer: Option<Arc<VideoFrameCoalescer>>,
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
            video_frame_coalescer: None,
        }
    }

    pub(super) fn with_video_frame_coalescer(
        mut self,
        coalescer: Arc<VideoFrameCoalescer>,
    ) -> Self {
        self.video_frame_coalescer = Some(coalescer);
        self
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
        let Some(event) = self.coalesce_video_frame(event) else {
            return;
        };
        let _ = self.tx.send(event);
        self.request_repaint();
    }

    fn coalesce_video_frame(&self, event: AdminEvent) -> Option<AdminEvent> {
        let Some(coalescer) = self.video_frame_coalescer.as_ref() else {
            return Some(event);
        };
        match event {
            AdminEvent::VideoFrame {
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
                let frame = PendingVideoFrame {
                    seq,
                    source_width,
                    source_height,
                    image_width,
                    image_height,
                    format,
                    bytes,
                };
                if coalescer.push(client_id.clone(), source.clone(), frame) {
                    Some(AdminEvent::VideoFrameReady {
                        client_id,
                        source,
                        coalescer: coalescer.clone(),
                    })
                } else {
                    None
                }
            }
            _ => Some(event),
        }
    }

    fn request_repaint(&self) {
        if let Some(ctx) = self
            .repaint_handle
            .as_ref()
            .and_then(|handle| handle.lock().ok().and_then(|ctx| ctx.clone()))
        {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

use crate::live_control;
use eframe::egui;
use rdl_protocol::{
    AudioSource, ClientInfo, CommandKind, CommandOutputStream, Message, VideoSource,
};
use std::sync::{mpsc::Sender, Arc, Mutex};

pub(super) enum AdminInput {
    List,
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
    AudioFrame {
        target_id: String,
        source: AudioSource,
        seq: u64,
        sample_rate: u32,
        channels: u16,
        format: String,
        bytes: Vec<u8>,
    },
    FileTransfer(Message),
    Reconnect {
        reason: String,
    },
    Quit,
}

pub(super) enum AdminEvent {
    Connected,
    Disconnected,
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
    Log(String),
}

#[derive(Clone)]
pub(super) struct AdminEventSink {
    tx: Sender<AdminEvent>,
    repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
}

impl AdminEventSink {
    pub(super) fn new(
        tx: Sender<AdminEvent>,
        repaint_handle: Option<Arc<Mutex<Option<egui::Context>>>>,
    ) -> Self {
        Self { tx, repaint_handle }
    }

    pub(super) fn send(&self, event: AdminEvent) {
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

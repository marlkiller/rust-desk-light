use crate::windowing;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);
const MAX_AUDIO_BUFFER_MS: usize = 300;
const MIN_AUDIO_PREBUFFER_MS: usize = 60;
const MAX_CAPTURE_QUEUE_MS: u64 = 240;
const MAX_CAPTURE_QUEUE_FRAMES: usize = 24;
const CAPTURE_QUEUE_CAPACITY: usize = 64;
const CAPTURE_OUTPUT_CHANNELS: u16 = 1;
const CAPTURE_FRAME_MS: u32 = 10;
const VOICE_CHAT_REPAINT_MS: u64 = 20;
const VOICE_CHAT_CAPTURE_REPORT_MS: u64 = 1_000;

pub(crate) struct VoiceChatWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    status: VoiceChatStatus,
    notice: String,
    started_at: Option<Instant>,
    running: Arc<AtomicBool>,
    mic_muted: Arc<AtomicBool>,
    speaker_muted: Arc<AtomicBool>,
    call_requested: Arc<AtomicBool>,
    end_requested: Arc<AtomicBool>,
    close_requested: Arc<AtomicBool>,
    open: bool,
    outbound: Vec<OutboundCommand>,
    input_stream: Option<AudioInputStream>,
    capture_active: bool,
    capture_generation: Arc<AtomicU64>,
    player: Option<AudioPlayer>,
    seq: u64,
    call_generation: u64,
    inbound_generation: Option<u64>,
    last_incoming_seq: u64,
    stats: VoiceStats,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum VoiceChatStatus {
    Ready,
    Ringing,
    Live,
    Ended,
    Failed,
}

#[derive(Clone, Default)]
struct VoiceStats {
    incoming_peak: f32,
    outgoing_peak: f32,
    last_frame_at: Option<Instant>,
}

pub(crate) struct AudioFrame {
    seq: u64,
    sample_rate: u32,
    channels: u16,
    format: String,
    bytes: Vec<u8>,
}

pub(crate) enum OutboundCommand {
    Command {
        client_id: String,
        payload: String,
    },
    AudioControl {
        client_id: String,
        payload: String,
    },
    AudioFrame {
        client_id: String,
        seq: u64,
        sample_rate: u32,
        channels: u16,
        format: String,
        bytes: Vec<u8>,
    },
}

struct CapturedAudioFrame {
    sample_rate: u32,
    channels: u16,
    format: String,
    bytes: Vec<u8>,
}

struct AudioInputStream {
    _stream: cpal::Stream,
}

struct AudioPlayer {
    buffer: Arc<Mutex<AudioPlaybackState>>,
    output_sample_rate: u32,
    output_channels: u16,
    _stream: cpal::Stream,
}

struct AudioPlaybackState {
    samples: VecDeque<f32>,
    started: bool,
    prebuffer_samples: usize,
    max_samples: usize,
}

#[derive(Default)]
struct AudioFramePacketizer {
    sample_rate: u32,
    channels: u16,
    format: String,
    frame_bytes: usize,
    pending: Vec<u8>,
}

impl AudioFramePacketizer {
    fn clear_pending(&mut self) {
        self.pending.clear();
    }

    fn push(&mut self, frame: CapturedAudioFrame) -> Vec<CapturedAudioFrame> {
        if frame.bytes.is_empty() {
            return Vec::new();
        }
        if self.sample_rate != frame.sample_rate
            || self.channels != frame.channels
            || self.format != frame.format
        {
            self.sample_rate = frame.sample_rate;
            self.channels = frame.channels;
            self.format = frame.format.clone();
            self.frame_bytes = capture_frame_bytes(frame.sample_rate, frame.channels);
            self.pending.clear();
        }
        self.pending.extend(frame.bytes);

        let mut frames = Vec::new();
        while self.pending.len() >= self.frame_bytes {
            let bytes: Vec<u8> = self.pending.drain(..self.frame_bytes).collect();
            frames.push(CapturedAudioFrame {
                sample_rate: self.sample_rate,
                channels: self.channels,
                format: self.format.clone(),
                bytes,
            });
        }
        frames
    }
}

pub(crate) fn open_window(
    windows: &mut Vec<VoiceChatWindow>,
    client_id: &str,
    hostname: String,
    username: String,
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    {
        window.hostname = hostname;
        window.username = username;
        window.open = true;
        window.close_requested.store(false, Ordering::Relaxed);
        return;
    }

    windows.push(VoiceChatWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        status: VoiceChatStatus::Ready,
        notice: "Ready to call".to_string(),
        started_at: None,
        running: Arc::new(AtomicBool::new(false)),
        mic_muted: Arc::new(AtomicBool::new(false)),
        speaker_muted: Arc::new(AtomicBool::new(false)),
        call_requested: Arc::new(AtomicBool::new(false)),
        end_requested: Arc::new(AtomicBool::new(false)),
        close_requested: Arc::new(AtomicBool::new(false)),
        open: true,
        outbound: Vec::new(),
        input_stream: None,
        capture_active: false,
        capture_generation: Arc::new(AtomicU64::new(0)),
        player: None,
        seq: 1,
        call_generation: 0,
        inbound_generation: None,
        last_incoming_seq: 0,
        stats: VoiceStats::default(),
    });
}

pub(crate) fn handle_ack(
    windows: &mut Vec<VoiceChatWindow>,
    client_id: &str,
    hostname: String,
    username: String,
    accepted: bool,
    detail: String,
    audio_tx: SyncSender<OutboundCommand>,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    window.hostname = hostname;
    window.username = username;
    match VoiceChatResponse::parse(&detail, accepted) {
        VoiceChatResponse::Accepted { generation } => {
            window.running.store(true, Ordering::Relaxed);
            window.inbound_generation = generation;
            match start_local_audio(window, audio_tx) {
                Ok(()) => {
                    window.status = VoiceChatStatus::Live;
                    window.notice = "Voice chat connected".to_string();
                    window.started_at = Some(Instant::now());
                    window.last_incoming_seq = 0;
                }
                Err(error) => {
                    stop_call(window, &format!("Local audio failed: {error}"));
                    window.status = VoiceChatStatus::Failed;
                }
            }
        }
        VoiceChatResponse::Declined(message) => {
            stop_call(window, &message);
            window.status = VoiceChatStatus::Ended;
        }
        VoiceChatResponse::Ended(message) => {
            stop_call(window, &message);
            window.status = VoiceChatStatus::Ended;
        }
        VoiceChatResponse::Error(message) => {
            stop_call(window, &message);
            window.status = VoiceChatStatus::Failed;
        }
        VoiceChatResponse::Other(message) => {
            window.notice = message;
        }
    }
}

pub(crate) fn decode_audio_frame(
    seq: u64,
    sample_rate: u32,
    channels: u16,
    format: String,
    bytes: Vec<u8>,
) -> Result<AudioFrame, String> {
    if sample_rate == 0 || channels == 0 {
        return Err("invalid voice chat audio metadata".to_string());
    }
    if format != "pcm_s16le" {
        return Err(format!("unsupported voice chat audio format: {format}"));
    }
    if bytes.len() < 2 || bytes.len() % 2 != 0 {
        return Err("invalid pcm_s16le voice chat frame size".to_string());
    }
    Ok(AudioFrame {
        seq,
        sample_rate,
        channels,
        format,
        bytes,
    })
}

pub(crate) fn handle_audio_frame(
    windows: &mut Vec<VoiceChatWindow>,
    client_id: &str,
    frame: AudioFrame,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if !matches!(window.status, VoiceChatStatus::Live) {
        return;
    }
    if let Some(generation) = window.inbound_generation {
        if sequence_generation(frame.seq) != generation {
            return;
        }
    }
    if frame.seq <= window.last_incoming_seq {
        return;
    }
    window.last_incoming_seq = frame.seq;
    let samples = pcm_s16le_to_f32(&frame.bytes);
    window.stats.incoming_peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    window.stats.last_frame_at = Some(Instant::now());
    window.notice = format!("Receiving voice frame {}", frame.seq);
    if !window.speaker_muted.load(Ordering::Relaxed) {
        if let Some(player) = &window.player {
            player.push_frame(&frame);
        }
    }
}

pub(crate) fn has_active_windows(windows: &[VoiceChatWindow]) -> bool {
    windows.iter().any(|window| {
        matches!(
            window.status,
            VoiceChatStatus::Ringing | VoiceChatStatus::Live
        )
    })
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<VoiceChatWindow>,
) -> Vec<OutboundCommand> {
    let mut outbound = Vec::new();
    for window in windows.iter_mut() {
        if window.close_requested.load(Ordering::Relaxed) {
            if matches!(
                window.status,
                VoiceChatStatus::Ringing | VoiceChatStatus::Live
            ) {
                window.queue_outbound(OutboundCommand::AudioControl {
                    client_id: window.client_id.clone(),
                    payload: "action=stop".to_string(),
                });
                stop_call(window, "Call ended");
                window.status = VoiceChatStatus::Ended;
            } else {
                window.open = false;
            }
        }
        if !window.open {
            continue;
        }

        render_window(ctx, window);

        if window.call_requested.swap(false, Ordering::Relaxed) {
            window.status = VoiceChatStatus::Ringing;
            window.notice = "Calling client".to_string();
            window.mic_muted.store(false, Ordering::Relaxed);
            window.speaker_muted.store(false, Ordering::Relaxed);
            window.call_generation = window.call_generation.saturating_add(1).max(1);
            window.seq = stream_sequence_base(window.call_generation);
            window.inbound_generation = None;
            window.last_incoming_seq = 0;
            window.queue_outbound(OutboundCommand::Command {
                client_id: window.client_id.clone(),
                payload: format!("action=invite\ngeneration={}", window.call_generation),
            });
        }
        if window.end_requested.swap(false, Ordering::Relaxed) {
            if matches!(
                window.status,
                VoiceChatStatus::Ringing | VoiceChatStatus::Live
            ) {
                window.queue_outbound(OutboundCommand::AudioControl {
                    client_id: window.client_id.clone(),
                    payload: "action=stop".to_string(),
                });
            }
            stop_call(window, "Call ended");
            window.status = VoiceChatStatus::Ended;
        }
        while let Some(message) = window.outbound.pop() {
            outbound.push(message);
        }
    }
    windows.retain(|window| window.open);
    outbound
}

impl VoiceChatWindow {
    fn queue_outbound(&mut self, command: OutboundCommand) {
        self.outbound.insert(0, command);
    }
}

fn render_window(ctx: &egui::Context, window: &mut VoiceChatWindow) {
    let title = format!(
        "Voice Chat - {}",
        identity_title(&window.hostname, &window.username)
    );
    let viewport_id = egui::ViewportId::from_hash_of(("voice_chat", &window.client_id));
    let builder = windowing::child_viewport_builder(title, [380.0, 520.0], [320.0, 420.0]);
    let status = window.status;
    let notice = window.notice.clone();
    let started_at = window.started_at;
    let mic_muted = window.mic_muted.clone();
    let speaker_muted = window.speaker_muted.clone();
    let call_requested = window.call_requested.clone();
    let end_requested = window.end_requested.clone();
    let close_requested = window.close_requested.clone();
    let stats = window.stats.clone();

    ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
        if ui.ctx().input(|input| input.viewport().close_requested()) {
            close_requested.store(true, Ordering::Relaxed);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(16.0))
            .show_inside(ui, |ui| {
                windowing::render_child_window_controls(ui);
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    render_avatar(ui, status);
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new(status_title(status))
                            .size(22.0)
                            .strong()
                            .color(COLOR_TEXT),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(notice.as_str())
                            .size(13.0)
                            .color(COLOR_MUTED),
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(duration_label(status, started_at))
                            .size(18.0)
                            .color(COLOR_TEXT),
                    );
                    ui.add_space(18.0);
                    render_meters(ui, &stats);
                    ui.add_space(22.0);
                    render_controls(
                        ui,
                        status,
                        &mic_muted,
                        &speaker_muted,
                        &call_requested,
                        &end_requested,
                    );
                });
            });
        if matches!(status, VoiceChatStatus::Live | VoiceChatStatus::Ringing) {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(VOICE_CHAT_REPAINT_MS));
        }
    });
}

fn render_avatar(ui: &mut egui::Ui, status: VoiceChatStatus) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(112.0, 112.0), egui::Sense::hover());
    let color = match status {
        VoiceChatStatus::Live => COLOR_GOOD,
        VoiceChatStatus::Failed => COLOR_BAD,
        VoiceChatStatus::Ended => COLOR_MUTED,
        _ => COLOR_WARN,
    };
    ui.painter().circle_filled(rect.center(), 54.0, COLOR_PANEL);
    ui.painter()
        .circle_stroke(rect.center(), 54.0, egui::Stroke::new(1.0, COLOR_BORDER));
    ui.painter()
        .circle_filled(rect.center(), 34.0, color.gamma_multiply(0.16));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Voice",
        egui::FontId::proportional(16.0),
        color,
    );
}

fn render_meters(ui: &mut egui::Ui, stats: &VoiceStats) {
    meter(ui, "You", stats.outgoing_peak);
    ui.add_space(6.0);
    meter(ui, "Client", stats.incoming_peak);
}

fn meter(ui: &mut egui::Ui, label: &str, peak: f32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(12.0).color(COLOR_MUTED));
        let desired = egui::vec2((ui.available_width() - 8.0).max(80.0), 10.0);
        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 4.0, egui::Color32::from_rgb(232, 237, 244));
        let fill = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width() * peak.clamp(0.0, 1.0), rect.height()),
        );
        ui.painter().rect_filled(fill, 4.0, COLOR_GOOD);
    });
}

fn render_controls(
    ui: &mut egui::Ui,
    status: VoiceChatStatus,
    mic_muted: &Arc<AtomicBool>,
    speaker_muted: &Arc<AtomicBool>,
    call_requested: &Arc<AtomicBool>,
    end_requested: &Arc<AtomicBool>,
) {
    match status {
        VoiceChatStatus::Ready | VoiceChatStatus::Ended | VoiceChatStatus::Failed => {
            if call_button(ui, "Call", COLOR_GOOD).clicked() {
                call_requested.store(true, Ordering::Relaxed);
            }
        }
        VoiceChatStatus::Ringing | VoiceChatStatus::Live => {
            ui.horizontal(|ui| {
                let mut mic = mic_muted.load(Ordering::Relaxed);
                if ui.checkbox(&mut mic, "Mute").changed() {
                    mic_muted.store(mic, Ordering::Relaxed);
                }
                let mut speaker = speaker_muted.load(Ordering::Relaxed);
                if ui.checkbox(&mut speaker, "Speaker off").changed() {
                    speaker_muted.store(speaker, Ordering::Relaxed);
                }
            });
            ui.add_space(20.0);
            if call_button(ui, "Hang Up", COLOR_BAD).clicked() {
                end_requested.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn call_button(ui: &mut egui::Ui, label: &str, color: egui::Color32) -> egui::Response {
    let fill = color.gamma_multiply(0.92);
    let text = egui::RichText::new(label)
        .color(egui::Color32::WHITE)
        .strong();
    ui.add_sized(
        [132.0, 42.0],
        egui::Button::new(text)
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.65))),
    )
}

fn start_local_audio(
    window: &mut VoiceChatWindow,
    audio_tx: SyncSender<OutboundCommand>,
) -> Result<(), String> {
    if window.player.is_none() {
        window.player = Some(AudioPlayer::start()?);
    }
    if !window.capture_active {
        let (frame_tx, frame_rx) = mpsc::sync_channel(CAPTURE_QUEUE_CAPACITY);
        let input_stream = start_input_stream(frame_tx)?;
        let capture_generation = window
            .capture_generation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let capture_state = window.capture_generation.clone();
        let client_id = window.client_id.clone();
        let running = window.running.clone();
        let mic_muted = window.mic_muted.clone();
        let seq = window.seq;
        thread::spawn(move || {
            voice_capture_loop(
                client_id,
                frame_rx,
                audio_tx,
                running,
                capture_state,
                capture_generation,
                mic_muted,
                seq,
            );
        });
        window.input_stream = Some(input_stream);
        window.capture_active = true;
    }
    Ok(())
}

fn stop_call(window: &mut VoiceChatWindow, notice: &str) {
    window.running.store(false, Ordering::Relaxed);
    window.capture_generation.fetch_add(1, Ordering::Relaxed);
    window.input_stream = None;
    window.capture_active = false;
    window.player = None;
    window.started_at = None;
    window.inbound_generation = None;
    window.last_incoming_seq = 0;
    window.stats = VoiceStats::default();
    window.notice = notice.to_string();
}

fn voice_capture_loop(
    client_id: String,
    frame_rx: mpsc::Receiver<CapturedAudioFrame>,
    audio_tx: SyncSender<OutboundCommand>,
    running: Arc<AtomicBool>,
    capture_state: Arc<AtomicU64>,
    capture_generation: u64,
    mic_muted: Arc<AtomicBool>,
    mut seq: u64,
) {
    let mut pending = VecDeque::new();
    let mut packetizer = AudioFramePacketizer::default();
    let mut queued_packets = 0_u64;
    let mut queued_bytes = 0_u64;
    let mut muted_frames = 0_u64;
    let mut queue_drops = 0_u64;
    let mut last_report = Instant::now();
    while running.load(Ordering::Relaxed)
        && capture_state.load(Ordering::Relaxed) == capture_generation
    {
        match frame_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(frame) => push_capture_frame(&mut pending, frame),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        while let Ok(frame) = frame_rx.try_recv() {
            push_capture_frame(&mut pending, frame);
        }
        for frame in pending.drain(..) {
            if mic_muted.load(Ordering::Relaxed) {
                muted_frames = muted_frames.saturating_add(1);
                packetizer.clear_pending();
                continue;
            }
            for frame in packetizer.push(frame) {
                let frame_bytes = frame.bytes.len() as u64;
                let command = OutboundCommand::AudioFrame {
                    client_id: client_id.clone(),
                    seq,
                    sample_rate: frame.sample_rate,
                    channels: frame.channels,
                    format: frame.format,
                    bytes: frame.bytes,
                };
                match audio_tx.try_send(command) {
                    Ok(()) => {
                        queued_packets = queued_packets.saturating_add(1);
                        queued_bytes = queued_bytes.saturating_add(frame_bytes);
                    }
                    Err(mpsc::TrySendError::Full(_)) => {
                        queue_drops = queue_drops.saturating_add(1);
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => return,
                }
                seq = seq.saturating_add(1);
            }
        }
        if last_report.elapsed() >= Duration::from_millis(VOICE_CHAT_CAPTURE_REPORT_MS) {
            eprintln!(
                "debug event=voice_chat_capture client={} packets={} bytes={} muted_frames={} queue_drops={} pending_frames={}",
                client_id,
                queued_packets,
                queued_bytes,
                muted_frames,
                queue_drops,
                pending.len()
            );
            last_report = Instant::now();
        }
    }
}

fn start_input_stream(
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<AudioInputStream, String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| "no default audio input device found".to_string())?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| format!("default input config failed: {error}"))?;
    let sample_format = supported_config.sample_format();
    let config = supported_config.config();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_f32_input_stream(&device, &config, frame_tx),
        cpal::SampleFormat::I16 => build_i16_input_stream(&device, &config, frame_tx),
        cpal::SampleFormat::U16 => build_u16_input_stream(&device, &config, frame_tx),
        other => Err(format!("unsupported input sample format: {other:?}")),
    }?;
    stream
        .play()
        .map_err(|error| format!("start input stream failed: {error}"))?;
    Ok(AudioInputStream { _stream: stream })
}

fn build_f32_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<cpal::Stream, String> {
    let sample_rate = config.sample_rate.0;
    let input_channels = config.channels.max(1) as usize;
    device
        .build_input_stream(
            config,
            move |data: &[f32], _| {
                send_frame(
                    &frame_tx,
                    sample_rate,
                    CAPTURE_OUTPUT_CHANNELS,
                    f32_to_mono_pcm_s16(data, input_channels),
                )
            },
            |error| eprintln!("voice chat input stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build input stream failed: {error}"))
}

fn build_i16_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<cpal::Stream, String> {
    let sample_rate = config.sample_rate.0;
    let input_channels = config.channels.max(1) as usize;
    device
        .build_input_stream(
            config,
            move |data: &[i16], _| {
                send_frame(
                    &frame_tx,
                    sample_rate,
                    CAPTURE_OUTPUT_CHANNELS,
                    i16_to_mono_pcm_s16(data, input_channels),
                )
            },
            |error| eprintln!("voice chat input stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build input stream failed: {error}"))
}

fn build_u16_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<cpal::Stream, String> {
    let sample_rate = config.sample_rate.0;
    let input_channels = config.channels.max(1) as usize;
    device
        .build_input_stream(
            config,
            move |data: &[u16], _| {
                send_frame(
                    &frame_tx,
                    sample_rate,
                    CAPTURE_OUTPUT_CHANNELS,
                    u16_to_mono_pcm_s16(data, input_channels),
                )
            },
            |error| eprintln!("voice chat input stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build input stream failed: {error}"))
}

impl AudioPlayer {
    fn start() -> Result<Self, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| "no default audio output device found".to_string())?;
        let supported_config = device
            .default_output_config()
            .map_err(|error| format!("default output config failed: {error}"))?;
        let sample_format = supported_config.sample_format();
        let config = supported_config.config();
        let output_sample_rate = config.sample_rate.0;
        let output_channels = config.channels;
        let buffer = Arc::new(Mutex::new(AudioPlaybackState::new(
            output_sample_rate,
            output_channels,
        )));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_f32_output_stream(&device, &config, buffer.clone()),
            cpal::SampleFormat::I16 => build_i16_output_stream(&device, &config, buffer.clone()),
            cpal::SampleFormat::U16 => build_u16_output_stream(&device, &config, buffer.clone()),
            other => Err(format!("unsupported output sample format: {other:?}")),
        }?;
        stream
            .play()
            .map_err(|error| format!("start output stream failed: {error}"))?;
        Ok(Self {
            buffer,
            output_sample_rate,
            output_channels,
            _stream: stream,
        })
    }

    fn push_frame(&self, frame: &AudioFrame) {
        if frame.format != "pcm_s16le" {
            return;
        }
        let samples = pcm_s16le_to_f32(&frame.bytes);
        let converted = resample_and_map_channels(
            &samples,
            frame.sample_rate,
            frame.channels,
            self.output_sample_rate,
            self.output_channels,
        );
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push_samples(converted);
        }
    }
}

impl AudioPlaybackState {
    fn new(sample_rate: u32, channels: u16) -> Self {
        let samples_per_ms = sample_rate as usize * channels.max(1) as usize;
        Self {
            samples: VecDeque::new(),
            started: false,
            prebuffer_samples: (samples_per_ms * MIN_AUDIO_PREBUFFER_MS / 1000).max(1),
            max_samples: (samples_per_ms * MAX_AUDIO_BUFFER_MS / 1000).max(1),
        }
    }

    fn push_samples(&mut self, samples: Vec<f32>) {
        self.samples.extend(samples);
        while self.samples.len() > self.max_samples {
            let _ = self.samples.pop_front();
            self.started = true;
        }
    }

    fn next_sample(&mut self) -> f32 {
        if !self.started {
            if self.samples.len() >= self.prebuffer_samples {
                self.started = true;
            } else {
                return 0.0;
            }
        }
        match self.samples.pop_front() {
            Some(sample) => sample,
            None => {
                self.started = false;
                0.0
            }
        }
    }
}

fn build_f32_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| fill_f32_output(data, &buffer),
            |error| eprintln!("voice chat output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn build_i16_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [i16], _| fill_i16_output(data, &buffer),
            |error| eprintln!("voice chat output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn build_u16_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [u16], _| fill_u16_output(data, &buffer),
            |error| eprintln!("voice chat output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn send_frame(
    frame_tx: &SyncSender<CapturedAudioFrame>,
    sample_rate: u32,
    channels: u16,
    bytes: Vec<u8>,
) {
    if bytes.is_empty() {
        return;
    }
    let _ = frame_tx.try_send(CapturedAudioFrame {
        sample_rate,
        channels,
        format: "pcm_s16le".to_string(),
        bytes,
    });
}

fn push_capture_frame(queue: &mut VecDeque<CapturedAudioFrame>, frame: CapturedAudioFrame) {
    queue.push_back(frame);
    while queue.len() > MAX_CAPTURE_QUEUE_FRAMES
        || capture_queue_duration_ms(queue) > MAX_CAPTURE_QUEUE_MS
    {
        if queue.len() <= 1 {
            break;
        }
        let _ = queue.pop_front();
    }
}

fn capture_queue_duration_ms(queue: &VecDeque<CapturedAudioFrame>) -> u64 {
    queue.iter().map(captured_frame_duration_ms).sum()
}

fn captured_frame_duration_ms(frame: &CapturedAudioFrame) -> u64 {
    let channels = frame.channels.max(1) as usize;
    let sample_rate = frame.sample_rate.max(1) as u64;
    let frames = frame.bytes.len() / 2 / channels;
    ((frames as u64 * 1000) / sample_rate).max(1)
}

fn capture_frame_bytes(sample_rate: u32, channels: u16) -> usize {
    let samples_per_channel =
        ((sample_rate.max(1) as u64 * CAPTURE_FRAME_MS as u64) / 1000).max(1) as usize;
    samples_per_channel * channels.max(1) as usize * 2
}

fn f32_to_mono_pcm_s16(data: &[f32], channels: usize) -> Vec<u8> {
    let channels = channels.max(1);
    let mut bytes = Vec::with_capacity((data.len() / channels) * 2);
    for frame in data.chunks_exact(channels) {
        let sample = frame.iter().copied().sum::<f32>() / channels as f32;
        let sample = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn i16_to_mono_pcm_s16(data: &[i16], channels: usize) -> Vec<u8> {
    let channels = channels.max(1);
    let mut bytes = Vec::with_capacity((data.len() / channels) * 2);
    for frame in data.chunks_exact(channels) {
        let sum: i64 = frame.iter().map(|sample| *sample as i64).sum();
        let sample = (sum / channels as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn u16_to_mono_pcm_s16(data: &[u16], channels: usize) -> Vec<u8> {
    let channels = channels.max(1);
    let mut bytes = Vec::with_capacity((data.len() / channels) * 2);
    for frame in data.chunks_exact(channels) {
        let sum: i64 = frame.iter().map(|sample| *sample as i64 - 32768).sum();
        let sample = (sum / channels as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn fill_f32_output(data: &mut [f32], buffer: &Arc<Mutex<AudioPlaybackState>>) {
    if let Ok(mut buffer) = buffer.lock() {
        for sample in data {
            *sample = buffer.next_sample();
        }
    } else {
        data.fill(0.0);
    }
}

fn fill_i16_output(data: &mut [i16], buffer: &Arc<Mutex<AudioPlaybackState>>) {
    if let Ok(mut buffer) = buffer.lock() {
        for sample in data {
            let value = buffer.next_sample().clamp(-1.0, 1.0);
            *sample = (value * i16::MAX as f32).round() as i16;
        }
    } else {
        data.fill(0);
    }
}

fn fill_u16_output(data: &mut [u16], buffer: &Arc<Mutex<AudioPlaybackState>>) {
    if let Ok(mut buffer) = buffer.lock() {
        for sample in data {
            let value = buffer.next_sample().clamp(-1.0, 1.0);
            *sample =
                ((value * i16::MAX as f32).round() as i32 + 32768).clamp(0, u16::MAX as i32) as u16;
        }
    } else {
        data.fill(32768);
    }
}

fn pcm_s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
        .collect()
}

fn resample_and_map_channels(
    input: &[f32],
    input_rate: u32,
    input_channels: u16,
    output_rate: u32,
    output_channels: u16,
) -> Vec<f32> {
    let input_channels = input_channels.max(1) as usize;
    let output_channels = output_channels.max(1) as usize;
    let input_frames = input.len() / input_channels;
    if input_frames == 0 || input_rate == 0 || output_rate == 0 {
        return Vec::new();
    }
    let output_frames =
        ((input_frames as f64 * output_rate as f64) / input_rate as f64).ceil() as usize;
    let mut output = Vec::with_capacity(output_frames * output_channels);
    let rate_ratio = input_rate as f64 / output_rate as f64;
    for output_frame in 0..output_frames {
        let source_pos = output_frame as f64 * rate_ratio;
        let input_frame = (source_pos.floor() as usize).min(input_frames - 1);
        let next_frame = input_frame.saturating_add(1).min(input_frames - 1);
        let mix = (source_pos - input_frame as f64) as f32;
        for output_channel in 0..output_channels {
            let current = mapped_channel_sample(input, input_frame, input_channels, output_channel);
            let next = mapped_channel_sample(input, next_frame, input_channels, output_channel);
            output.push(current + (next - current) * mix);
        }
    }
    output
}

fn mapped_channel_sample(
    input: &[f32],
    frame: usize,
    input_channels: usize,
    output_channel: usize,
) -> f32 {
    if input_channels == 1 {
        return input[frame * input_channels];
    }
    if output_channel < input_channels {
        return input[frame * input_channels + output_channel];
    }
    let start = frame * input_channels;
    let sum: f32 = input[start..start + input_channels].iter().copied().sum();
    sum / input_channels as f32
}

enum VoiceChatResponse {
    Accepted { generation: Option<u64> },
    Declined(String),
    Ended(String),
    Error(String),
    Other(String),
}

impl VoiceChatResponse {
    fn parse(detail: &str, accepted: bool) -> Self {
        let mut lines = detail.lines();
        match lines.next().unwrap_or_default().trim() {
            "voice_chat_accepted" if accepted => Self::Accepted {
                generation: payload_field(detail, "generation")
                    .and_then(|value| value.parse::<u64>().ok()),
            },
            "voice_chat_declined" => Self::Declined(
                payload_field(detail, "message").unwrap_or_else(|| "Declined".to_string()),
            ),
            "voice_chat_ended" => Self::Ended(
                payload_field(detail, "message").unwrap_or_else(|| "Call ended".to_string()),
            ),
            "voice_chat_error" => Self::Error(
                payload_field(detail, "message").unwrap_or_else(|| "voice chat failed".to_string()),
            ),
            other if !accepted => Self::Error(if other.is_empty() {
                detail.to_string()
            } else {
                other.to_string()
            }),
            other => Self::Other(other.to_string()),
        }
    }
}

fn stream_sequence_base(generation: u64) -> u64 {
    generation.saturating_mul(1_u64 << 32).max(1)
}

fn sequence_generation(seq: u64) -> u64 {
    seq >> 32
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

fn status_title(status: VoiceChatStatus) -> &'static str {
    match status {
        VoiceChatStatus::Ready => "Voice Chat",
        VoiceChatStatus::Ringing => "Calling",
        VoiceChatStatus::Live => "Voice Chat",
        VoiceChatStatus::Ended => "Call Ended",
        VoiceChatStatus::Failed => "Call Failed",
    }
}

fn duration_label(status: VoiceChatStatus, started_at: Option<Instant>) -> String {
    if status != VoiceChatStatus::Live {
        return "--:--".to_string();
    }
    let elapsed = started_at
        .map(|started_at| started_at.elapsed())
        .unwrap_or_default()
        .as_secs();
    format!("{:02}:{:02}", elapsed / 60, elapsed % 60)
}

fn identity_title(hostname: &str, username: &str) -> String {
    if username.trim().is_empty() {
        hostname.to_string()
    } else {
        format!("{hostname} / {username}")
    }
}

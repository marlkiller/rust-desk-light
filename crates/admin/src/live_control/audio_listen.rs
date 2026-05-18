use crate::{
    theme::{COLOR_BAD, COLOR_BORDER, COLOR_GOOD, COLOR_MUTED, COLOR_PANEL, COLOR_WARN},
    windowing,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::COMPACT_CONTROL_HEIGHT;
const MAX_AUDIO_BUFFER_MS: usize = 220;
const MIN_AUDIO_PREBUFFER_MS: usize = 40;
const MAX_AUDIO_PREBUFFER_MS: usize = 120;
const PREBUFFER_ADJUST_STEP_MS: usize = 10;
const PREBUFFER_DECAY_AFTER_MS: usize = 1_000;
const PLAYBACK_TARGET_EXTRA_MS: usize = 20;
const PLAYBACK_DRIFT_WINDOW_MS: usize = 60;
const MAX_PLAYBACK_STRETCH: f64 = 0.010;
const AUDIO_STREAM_RELEASE_SETTLE_MS: u64 = 40;

pub(crate) struct AudioListenWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    devices: Vec<AudioDevice>,
    selected_device: Arc<Mutex<usize>>,
    status: AudioStatus,
    notice: String,
    stats: AudioStats,
    running: Arc<AtomicBool>,
    outbound: Vec<String>,
    pending_since: Option<Instant>,
    open: bool,
    close_requested: Arc<AtomicBool>,
    playback_registry: AudioPlaybackRegistry,
    player: Option<AudioPlayer>,
    inbound_generation: Option<u64>,
    last_incoming_seq: u64,
}

#[derive(Clone, Default)]
pub(crate) struct AudioPlaybackRegistry {
    sinks: Arc<Mutex<HashMap<String, AudioPlaybackSink>>>,
}

#[derive(Clone)]
struct AudioPlaybackSink {
    generation: Option<u64>,
    last_seq: u64,
    buffer: Arc<Mutex<AudioPlaybackState>>,
    output_sample_rate: u32,
    output_channels: u16,
}

#[derive(Clone)]
struct AudioDevice {
    index: usize,
    name: String,
    description: String,
}

pub(crate) struct AudioFrame {
    seq: u64,
    sample_rate: u32,
    channels: u16,
    format: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Default)]
struct AudioStats {
    frame_count: u64,
    encoded_bytes: usize,
    sample_rate: u32,
    channels: u16,
    format: String,
    peak: f32,
    missing_frames: u64,
    buffered_ms: u64,
    underflows: u64,
    dropped_ms: u64,
    concealed_ms: u64,
    prebuffer_ms: u64,
    stretch_ppm: i32,
    output_misses: u64,
    last_frame_at: Option<Instant>,
}

#[derive(Clone, Copy)]
enum AudioStatus {
    Ready,
    Pending,
    Live,
    Failed,
}

pub(crate) struct OutboundCommand {
    pub(crate) client_id: String,
    pub(crate) payload: String,
}

pub(crate) fn decode_audio_frame(
    seq: u64,
    sample_rate: u32,
    channels: u16,
    format: String,
    bytes: Vec<u8>,
) -> Result<AudioFrame, String> {
    if sample_rate == 0 || channels == 0 {
        return Err("invalid audio frame metadata".to_string());
    }
    if format != "pcm_s16le" {
        return Err(format!("unsupported audio frame format: {format}"));
    }
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return Err("invalid pcm_s16le audio frame size".to_string());
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
    windows: &mut [AudioListenWindow],
    client_id: &str,
    frame: AudioFrame,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if !window.running.load(Ordering::Relaxed) {
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
    if window.last_incoming_seq != 0 && frame.seq > window.last_incoming_seq.saturating_add(1) {
        window.stats.missing_frames = window
            .stats
            .missing_frames
            .saturating_add(frame.seq - window.last_incoming_seq - 1);
    }
    window.last_incoming_seq = frame.seq;
    let playback = window.player.as_ref().and_then(AudioPlayer::snapshot);
    if let Some(playback) = playback {
        window.stats.buffered_ms = playback.buffered_ms;
        window.stats.underflows = playback.underflows;
        window.stats.dropped_ms = playback.dropped_ms;
        window.stats.concealed_ms = playback.concealed_ms;
        window.stats.prebuffer_ms = playback.prebuffer_ms;
        window.stats.stretch_ppm = playback.stretch_ppm;
        window.stats.output_misses = playback.output_misses;
    }
    handle_frame(window, frame);
}

pub(crate) fn has_active_windows(windows: &[AudioListenWindow]) -> bool {
    windows
        .iter()
        .any(|window| window.running.load(Ordering::Relaxed) || window.pending_since.is_some())
}

pub(crate) fn open_window(
    windows: &mut Vec<AudioListenWindow>,
    client_id: &str,
    hostname: String,
    username: String,
    playback_registry: AudioPlaybackRegistry,
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    {
        window.open = true;
        window.hostname = hostname;
        window.username = username;
        window.playback_registry = playback_registry;
        window.close_requested.store(false, Ordering::Relaxed);
        window.queue_devices();
        return;
    }

    let mut window = AudioListenWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        devices: Vec::new(),
        selected_device: Arc::new(Mutex::new(0)),
        status: AudioStatus::Ready,
        notice: "Select an input device and click Start".to_string(),
        stats: AudioStats::default(),
        running: Arc::new(AtomicBool::new(false)),
        outbound: Vec::new(),
        pending_since: None,
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
        playback_registry,
        player: None,
        inbound_generation: None,
        last_incoming_seq: 0,
    };
    window.queue_devices();
    windows.push(window);
}

pub(crate) fn handle_ack(
    windows: &mut [AudioListenWindow],
    client_id: &str,
    hostname: String,
    username: String,
    accepted: bool,
    detail: String,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    window.hostname = hostname;
    window.username = username;
    window.pending_since = None;
    if !accepted {
        stop_listen(window, &detail);
        window.status = AudioStatus::Failed;
        return;
    }

    match AudioResponse::parse(&detail) {
        AudioResponse::Devices(devices) => {
            window.devices = devices;
            if let Ok(mut selected) = window.selected_device.lock() {
                if !window
                    .devices
                    .iter()
                    .any(|device| device.index == *selected)
                {
                    *selected = window
                        .devices
                        .first()
                        .map(|device| device.index)
                        .unwrap_or_default();
                }
            }
            window.status = AudioStatus::Ready;
            window.notice = if window.devices.is_empty() {
                "No audio input devices found".to_string()
            } else {
                "Select an input device and click Start".to_string()
            };
        }
        AudioResponse::Started {
            message,
            generation,
        } => {
            window.status = AudioStatus::Pending;
            window.notice = message;
            window.inbound_generation = generation;
            window.last_incoming_seq = 0;
            window.register_playback();
        }
        AudioResponse::Stopped => stop_listen(window, "Stopped"),
        AudioResponse::Error(message) => {
            stop_listen(window, &message);
            window.status = AudioStatus::Failed;
        }
        AudioResponse::Other(message) => {
            window.notice = message;
        }
    }
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<AudioListenWindow>,
) -> Vec<OutboundCommand> {
    let mut outbound = Vec::new();
    for window in windows.iter_mut() {
        if window.close_requested.load(Ordering::Relaxed) {
            if window.running.load(Ordering::Relaxed) || window.pending_since.is_some() {
                outbound.push(OutboundCommand {
                    client_id: window.client_id.clone(),
                    payload: "action=stop".to_string(),
                });
            }
            stop_listen(window, "Stopped");
            window.open = false;
        }
        if !window.open {
            continue;
        }
        if window
            .pending_since
            .is_some_and(|pending_since| pending_since.elapsed() > Duration::from_secs(20))
        {
            stop_listen(window, "Timed out waiting for audio result");
            window.status = AudioStatus::Failed;
        }

        let title = format!(
            "Audio Listen - {}",
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("audio_listen", &window.client_id));
        let builder = windowing::child_viewport_builder(title, [640.0, 320.0], [460.0, 260.0]);

        let client_id = window.client_id.clone();
        let close_requested = window.close_requested.clone();
        let devices = window.devices.clone();
        let selected_device = window.selected_device.clone();
        let running = window.running.clone();
        let notice = window.notice.clone();
        let status = window.status;
        let stats = window.stats.clone();
        let queued = Arc::new(Mutex::new(Vec::new()));
        let queued_for_ui = queued.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(crate::theme::page_frame())
                .show_inside(ui, |ui| {
                    windowing::render_child_window_controls(ui);
                    render_toolbar(ui, &devices, &selected_device, &running, &queued_for_ui);
                    ui.add_space(10.0);
                    render_meter(ui, stats.peak, &notice);
                    ui.add_space(10.0);
                    render_status_bar(ui, status, &notice, &stats);
                });
            if running.load(Ordering::Relaxed) {
                ui.ctx().request_repaint_after(Duration::from_millis(33));
            }
        });

        if let Ok(mut queued) = queued.lock() {
            for payload in queued.drain(..) {
                if payload.trim() == "action=stop" {
                    stop_listen(window, "Stopped");
                }
                window.queue_payload(payload);
            }
        }
        while let Some(payload) = window.outbound.pop() {
            let action = payload_action(&payload);
            if action.as_deref() == Some("start") {
                match AudioPlayer::start() {
                    Ok(player) => {
                        window.player = Some(player);
                        window.register_playback();
                    }
                    Err(error) => {
                        window.running.store(false, Ordering::Relaxed);
                        window.status = AudioStatus::Failed;
                        window.notice = format!("Audio output failed: {error}");
                        continue;
                    }
                }
            } else if action.as_deref() == Some("stop") {
                window.player = None;
            }

            window.status = AudioStatus::Pending;
            window.notice = match action.as_deref() {
                Some("devices") => "Loading audio input devices".to_string(),
                Some("start") => "Waiting for client audio".to_string(),
                Some("stop") => "Stopping audio listen".to_string(),
                _ => "Waiting for client result".to_string(),
            };
            window.pending_since = Some(Instant::now());
            outbound.push(OutboundCommand {
                client_id: client_id.clone(),
                payload,
            });
        }
    }
    windows.retain(|window| window.open);
    outbound
}

impl AudioListenWindow {
    fn queue_devices(&mut self) {
        self.queue_payload("action=devices".to_string());
    }

    fn queue_payload(&mut self, payload: String) {
        self.outbound.insert(0, payload);
    }

    fn register_playback(&self) {
        if let Some(player) = &self.player {
            self.playback_registry.register_player(
                &self.client_id,
                self.inbound_generation,
                player,
            );
        }
    }
}

fn render_toolbar(
    ui: &mut egui::Ui,
    devices: &[AudioDevice],
    selected_device: &Arc<Mutex<usize>>,
    running: &Arc<AtomicBool>,
    queued: &Arc<Mutex<Vec<String>>>,
) {
    ui.vertical(|ui| {
        let is_running = running.load(Ordering::Relaxed);
        toolbar_row(ui, |ui| {
            let mut selected = selected_device
                .lock()
                .map(|value| *value)
                .unwrap_or_default();
            ui.label(egui::RichText::new("Device").size(12.0).color(COLOR_MUTED));
            let combo_width = (ui.available_width() - 12.0).max(180.0);
            toolbar_dropdown(
                ui,
                "audio_listen_device_select",
                device_label(devices, selected),
                combo_width,
                !is_running,
                |ui| {
                    ui.set_min_width(combo_width);
                    for device in devices {
                        let response = ui.selectable_value(
                            &mut selected,
                            device.index,
                            device_label_one(device),
                        );
                        let clicked = response.clicked();
                        if !device.description.trim().is_empty() {
                            response.on_hover_text(device.description.trim());
                        }
                        if clicked {
                            ui.close();
                        }
                    }
                },
            );
            if let Ok(mut value) = selected_device.lock() {
                *value = selected;
            }
        });
        toolbar_row(ui, |ui| {
            if ui
                .add_enabled(!is_running, egui::Button::new("Reload Devices"))
                .clicked()
            {
                queue_ui_payload(queued, "action=devices\nscan=full".to_string());
            }
            let selected = selected_device
                .lock()
                .map(|value| *value)
                .unwrap_or_default();
            if ui
                .add_enabled(
                    !devices.is_empty(),
                    egui::Button::new(if is_running { "Stop" } else { "Start" }),
                )
                .clicked()
            {
                if is_running {
                    running.store(false, Ordering::Relaxed);
                    queue_ui_payload(queued, "action=stop".to_string());
                } else {
                    running.store(true, Ordering::Relaxed);
                    queue_ui_payload(queued, format!("action=start\ndevice={selected}"));
                }
            }
        });
    });
}

fn toolbar_row(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        ui.horizontal(add_contents);
    });
}

fn toolbar_dropdown(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    label: impl Into<String>,
    width: f32,
    enabled: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    ui.push_id(id_salt, |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            let button = egui::Button::new(label.into())
                .right_text("  ")
                .wrap_mode(egui::TextWrapMode::Truncate)
                .min_size(egui::vec2(width, TOOLBAR_CONTROL_HEIGHT));
            let (response, _) =
                egui::containers::menu::MenuButton::from_button(button).ui(ui, add_contents);
            paint_dropdown_icon(ui, &response);
        });
    });
}

fn paint_dropdown_icon(ui: &egui::Ui, response: &egui::Response) {
    let visuals = ui.style().interact(response);
    let center = egui::pos2(response.rect.right() - 12.0, response.rect.center().y + 1.0);
    let points = vec![
        egui::pos2(center.x - 4.0, center.y - 2.0),
        egui::pos2(center.x + 4.0, center.y - 2.0),
        egui::pos2(center.x, center.y + 3.0),
    ];
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        visuals.fg_stroke.color,
        egui::Stroke::NONE,
    ));
}

fn render_meter(ui: &mut egui::Ui, peak: f32, notice: &str) {
    let desired = egui::vec2(ui.available_width(), 86.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    ui.painter().rect_filled(rect, 6.0, COLOR_PANEL);
    ui.painter().rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, COLOR_BORDER),
        egui::StrokeKind::Inside,
    );
    let meter_rect = rect.shrink2(egui::vec2(18.0, 28.0));
    ui.painter()
        .rect_filled(meter_rect, 4.0, crate::theme::COLOR_METER_BG);
    let fill_width = meter_rect.width() * peak.clamp(0.0, 1.0);
    let fill_rect =
        egui::Rect::from_min_size(meter_rect.min, egui::vec2(fill_width, meter_rect.height()));
    ui.painter().rect_filled(fill_rect, 4.0, COLOR_GOOD);
    ui.painter().text(
        rect.center_top() + egui::vec2(0.0, 9.0),
        egui::Align2::CENTER_TOP,
        notice,
        egui::FontId::proportional(13.0),
        COLOR_MUTED,
    );
}

fn render_status_bar(ui: &mut egui::Ui, status: AudioStatus, notice: &str, stats: &AudioStats) {
    ui.horizontal(|ui| {
        status_pill(ui, status);
        ui.separator();
        ui.label(egui::RichText::new(notice).size(12.0).color(COLOR_MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let meta = if stats.sample_rate == 0 {
                "no audio".to_string()
            } else {
                format!(
                    "{} {}ch {}B",
                    compact_sample_rate(stats.sample_rate),
                    stats.channels,
                    stats.encoded_bytes
                )
            };
            let meta = if stats.sample_rate == 0 {
                meta
            } else {
                format!(
                    "{meta} buf{} pb{} dr{} plc{} drop{} gap{} uf{} m{}",
                    stats.buffered_ms,
                    stats.prebuffer_ms,
                    format_drift(stats.stretch_ppm),
                    stats.concealed_ms,
                    stats.dropped_ms,
                    stats.missing_frames,
                    stats.underflows,
                    stats.output_misses
                )
            };
            ui.label(egui::RichText::new(meta).size(12.0).color(COLOR_MUTED));
        });
    });
}

fn status_pill(ui: &mut egui::Ui, status: AudioStatus) {
    let (label, color) = match status {
        AudioStatus::Ready => ("Ready", COLOR_MUTED),
        AudioStatus::Pending => ("Pending", COLOR_WARN),
        AudioStatus::Live => ("Live", COLOR_GOOD),
        AudioStatus::Failed => ("Failed", COLOR_BAD),
    };
    egui::Frame::default()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(999.0)
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(12.0).color(color).strong());
        });
}

fn format_drift(stretch_ppm: i32) -> String {
    if stretch_ppm == 0 {
        "0".to_string()
    } else {
        format!("{:+.1}", stretch_ppm as f32 / 10_000.0)
    }
}

fn compact_sample_rate(sample_rate: u32) -> String {
    if sample_rate >= 1000 && sample_rate.is_multiple_of(1000) {
        format!("{}k", sample_rate / 1000)
    } else {
        format!("{sample_rate}Hz")
    }
}

fn handle_frame(window: &mut AudioListenWindow, frame: AudioFrame) {
    let samples = pcm_s16le_to_f32(&frame.bytes);
    let now = Instant::now();
    window.stats.frame_count = window.stats.frame_count.saturating_add(1);
    window.stats.encoded_bytes = frame.bytes.len();
    window.stats.sample_rate = frame.sample_rate;
    window.stats.channels = frame.channels;
    window.stats.format = frame.format.clone();
    window.stats.peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    window.stats.last_frame_at = Some(now);
    window.status = AudioStatus::Live;
    window.notice = format!("Receiving audio frame {}", frame.seq);
}

fn stop_listen(window: &mut AudioListenWindow, notice: &str) {
    window.running.store(false, Ordering::Relaxed);
    window.playback_registry.stop(&window.client_id);
    window.outbound.clear();
    window.pending_since = None;
    window.player = None;
    window.inbound_generation = None;
    window.last_incoming_seq = 0;
    window.stats = AudioStats::default();
    window.status = AudioStatus::Ready;
    window.notice = notice.to_string();
}

struct AudioPlayer {
    buffer: Arc<Mutex<AudioPlaybackState>>,
    output_misses: Arc<AtomicU64>,
    output_sample_rate: u32,
    output_channels: u16,
    _stream: cpal::Stream,
}

struct AudioPlaybackState {
    samples: VecDeque<f32>,
    started: bool,
    prebuffer_samples: usize,
    min_prebuffer_samples: usize,
    max_prebuffer_samples: usize,
    prebuffer_step_samples: usize,
    prebuffer_decay_samples: usize,
    samples_since_underflow: usize,
    max_samples: usize,
    sample_rate: u32,
    channels: u16,
    underflows: u64,
    dropped_samples: u64,
    concealed_samples: u64,
    stretch_ppm: i32,
    last_output_frame: Vec<f32>,
}

#[derive(Clone, Copy)]
struct AudioPlaybackSnapshot {
    buffered_ms: u64,
    underflows: u64,
    dropped_ms: u64,
    concealed_ms: u64,
    prebuffer_ms: u64,
    stretch_ppm: i32,
    output_misses: u64,
}

impl AudioPlaybackRegistry {
    pub(crate) fn push_frame(
        &self,
        client_id: &str,
        seq: u64,
        sample_rate: u32,
        channels: u16,
        format: &str,
        bytes: &[u8],
    ) {
        if format != "pcm_s16le" {
            return;
        }
        let Some((sink, missing_packets)) = self.active_sink(client_id, seq) else {
            return;
        };
        let samples = pcm_s16le_to_f32(bytes);
        let stretch_ratio = sink
            .buffer
            .lock()
            .map(|mut buffer| buffer.playback_stretch_ratio())
            .unwrap_or(1.0);
        let input_frame_count = samples.len() / channels.max(1) as usize;
        let converted = resample_and_map_channels(
            &samples,
            sample_rate,
            channels,
            sink.output_sample_rate,
            sink.output_channels,
            stretch_ratio,
        );
        let buffer_arc = sink.buffer.clone();
        if let Ok(mut buffer) = buffer_arc.lock() {
            if missing_packets > 0 {
                let concealed_frames = concealed_output_frames(
                    missing_packets,
                    input_frame_count,
                    sample_rate,
                    sink.output_sample_rate,
                );
                buffer.push_concealment_frames(concealed_frames);
            }
            buffer.push_samples(converted);
        };
    }

    fn register_player(&self, client_id: &str, generation: Option<u64>, player: &AudioPlayer) {
        if let Ok(mut sinks) = self.sinks.lock() {
            sinks.insert(
                client_id.to_string(),
                AudioPlaybackSink {
                    generation,
                    last_seq: 0,
                    buffer: player.buffer.clone(),
                    output_sample_rate: player.output_sample_rate,
                    output_channels: player.output_channels,
                },
            );
        }
        debug_log!(
            "debug event=audio_listen_playback client={} generation={} output_rate={} output_channels={} prebuffer_ms={} max_buffer_ms={}",
            client_id,
            generation.map(|value| value.to_string()).unwrap_or_else(|| "none".to_string()),
            player.output_sample_rate,
            player.output_channels,
            MIN_AUDIO_PREBUFFER_MS,
            MAX_AUDIO_BUFFER_MS
        );
    }

    fn stop(&self, client_id: &str) {
        if let Ok(mut sinks) = self.sinks.lock() {
            sinks.remove(client_id);
        }
    }

    fn active_sink(&self, client_id: &str, seq: u64) -> Option<(AudioPlaybackSink, u64)> {
        let mut sinks = self.sinks.lock().ok()?;
        let sink = sinks.get_mut(client_id)?;
        if let Some(generation) = sink.generation {
            if sequence_generation(seq) != generation {
                return None;
            }
        }
        if seq <= sink.last_seq {
            return None;
        }
        let missing_packets = if sink.last_seq == 0 {
            0
        } else {
            seq.saturating_sub(sink.last_seq.saturating_add(1))
        };
        sink.last_seq = seq;
        Some((sink.clone(), missing_packets))
    }
}

impl AudioPlayer {
    fn start() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
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
        let output_misses = Arc::new(AtomicU64::new(0));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                build_f32_output_stream(&device, &config, buffer.clone(), output_misses.clone())
            }
            cpal::SampleFormat::I16 => {
                build_i16_output_stream(&device, &config, buffer.clone(), output_misses.clone())
            }
            cpal::SampleFormat::U16 => {
                build_u16_output_stream(&device, &config, buffer.clone(), output_misses.clone())
            }
            other => Err(format!("unsupported output sample format: {other:?}")),
        }?;
        stream
            .play()
            .map_err(|error| format!("start output stream failed: {error}"))?;
        Ok(Self {
            buffer,
            output_misses,
            output_sample_rate,
            output_channels,
            _stream: stream,
        })
    }

    fn snapshot(&self) -> Option<AudioPlaybackSnapshot> {
        self.buffer
            .lock()
            .ok()
            .map(|buffer| buffer.snapshot(self.output_misses.load(Ordering::Relaxed)))
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        let _ = self._stream.pause();
        thread::sleep(Duration::from_millis(AUDIO_STREAM_RELEASE_SETTLE_MS));
    }
}

impl AudioPlaybackState {
    fn new(sample_rate: u32, channels: u16) -> Self {
        let samples_per_ms = sample_rate as usize * channels.max(1) as usize;
        let min_prebuffer_samples = (samples_per_ms * MIN_AUDIO_PREBUFFER_MS / 1000).max(1);
        let max_prebuffer_samples =
            (samples_per_ms * MAX_AUDIO_PREBUFFER_MS / 1000).max(min_prebuffer_samples);
        Self {
            samples: VecDeque::new(),
            started: false,
            prebuffer_samples: min_prebuffer_samples,
            min_prebuffer_samples,
            max_prebuffer_samples,
            prebuffer_step_samples: (samples_per_ms * PREBUFFER_ADJUST_STEP_MS / 1000).max(1),
            prebuffer_decay_samples: (samples_per_ms * PREBUFFER_DECAY_AFTER_MS / 1000).max(1),
            samples_since_underflow: 0,
            max_samples: (samples_per_ms * MAX_AUDIO_BUFFER_MS / 1000).max(1),
            sample_rate,
            channels,
            underflows: 0,
            dropped_samples: 0,
            concealed_samples: 0,
            stretch_ppm: 0,
            last_output_frame: vec![0.0; channels.max(1) as usize],
        }
    }

    fn playback_stretch_ratio(&mut self) -> f64 {
        let channels = self.channels.max(1) as usize;
        let target_extra_samples =
            (self.sample_rate as usize * channels * PLAYBACK_TARGET_EXTRA_MS / 1000).max(1);
        let drift_window_samples =
            (self.sample_rate as usize * channels * PLAYBACK_DRIFT_WINDOW_MS / 1000).max(1);
        let target_samples = self.prebuffer_samples.saturating_add(target_extra_samples);
        let error_samples = target_samples as isize - self.samples.len() as isize;
        let correction = (error_samples as f64 / drift_window_samples as f64).clamp(-1.0, 1.0)
            * MAX_PLAYBACK_STRETCH;
        self.stretch_ppm = (correction * 1_000_000.0).round() as i32;
        1.0 + correction
    }

    fn push_samples(&mut self, samples: Vec<f32>) {
        self.remember_last_frame(&samples);
        self.samples.extend(samples);
        let excess = self.samples.len().saturating_sub(self.max_samples);
        if excess > 0 {
            self.samples.drain(..excess);
            self.dropped_samples = self.dropped_samples.saturating_add(excess as u64);
            self.started = true;
        }
    }

    fn push_concealment_frames(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        let channels = self.channels.max(1) as usize;
        if self.last_output_frame.len() != channels {
            self.last_output_frame.resize(channels, 0.0);
        }
        for frame in 0..frames {
            let gain = 1.0 - (frame as f32 + 1.0) / frames as f32;
            for channel in 0..channels {
                self.samples
                    .push_back(self.last_output_frame[channel] * gain.max(0.0));
            }
        }
        self.last_output_frame.fill(0.0);
        let added = frames.saturating_mul(channels);
        self.concealed_samples = self.concealed_samples.saturating_add(added as u64);
        let excess = self.samples.len().saturating_sub(self.max_samples);
        if excess > 0 {
            self.samples.drain(..excess);
            self.dropped_samples = self.dropped_samples.saturating_add(excess as u64);
            self.started = true;
        }
    }

    fn remember_last_frame(&mut self, samples: &[f32]) {
        let channels = self.channels.max(1) as usize;
        if samples.len() < channels {
            return;
        }
        self.last_output_frame.clear();
        self.last_output_frame
            .extend_from_slice(&samples[samples.len() - channels..]);
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
            Some(sample) => {
                self.record_played_sample();
                sample
            }
            None => {
                self.started = false;
                self.underflows = self.underflows.saturating_add(1);
                self.samples_since_underflow = 0;
                self.prebuffer_samples = self
                    .prebuffer_samples
                    .saturating_add(self.prebuffer_step_samples)
                    .min(self.max_prebuffer_samples);
                0.0
            }
        }
    }

    fn record_played_sample(&mut self) {
        self.samples_since_underflow = self.samples_since_underflow.saturating_add(1);
        if self.prebuffer_samples <= self.min_prebuffer_samples
            || self.samples_since_underflow < self.prebuffer_decay_samples
        {
            return;
        }
        self.prebuffer_samples = self
            .prebuffer_samples
            .saturating_sub(self.prebuffer_step_samples)
            .max(self.min_prebuffer_samples);
        self.samples_since_underflow = 0;
    }

    fn snapshot(&self, output_misses: u64) -> AudioPlaybackSnapshot {
        AudioPlaybackSnapshot {
            buffered_ms: self.buffered_ms(),
            underflows: self.underflows,
            dropped_ms: self.dropped_ms(),
            concealed_ms: self.concealed_ms(),
            prebuffer_ms: self.prebuffer_ms(),
            stretch_ppm: self.stretch_ppm,
            output_misses,
        }
    }

    fn buffered_ms(&self) -> u64 {
        let channels = self.channels.max(1) as usize;
        let frames = self.samples.len() / channels;
        frames as u64 * 1000 / self.sample_rate.max(1) as u64
    }

    fn prebuffer_ms(&self) -> u64 {
        let channels = self.channels.max(1) as usize;
        let frames = self.prebuffer_samples / channels;
        frames as u64 * 1000 / self.sample_rate.max(1) as u64
    }

    fn dropped_ms(&self) -> u64 {
        let channels = self.channels.max(1) as u64;
        let frames = self.dropped_samples / channels;
        frames * 1000 / self.sample_rate.max(1) as u64
    }

    fn concealed_ms(&self) -> u64 {
        let channels = self.channels.max(1) as u64;
        let frames = self.concealed_samples / channels;
        frames * 1000 / self.sample_rate.max(1) as u64
    }
}

fn build_f32_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
    output_misses: Arc<AtomicU64>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| fill_f32_output(data, &buffer, &output_misses),
            |error| eprintln!("audio output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn build_i16_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
    output_misses: Arc<AtomicU64>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [i16], _| fill_i16_output(data, &buffer, &output_misses),
            |error| eprintln!("audio output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn build_u16_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
    output_misses: Arc<AtomicU64>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [u16], _| fill_u16_output(data, &buffer, &output_misses),
            |error| eprintln!("audio output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn fill_f32_output(
    data: &mut [f32],
    buffer: &Arc<Mutex<AudioPlaybackState>>,
    output_misses: &Arc<AtomicU64>,
) {
    if let Ok(mut buffer) = buffer.try_lock() {
        for sample in data {
            *sample = buffer.next_sample();
        }
    } else {
        output_misses.fetch_add(1, Ordering::Relaxed);
        data.fill(0.0);
    }
}

fn fill_i16_output(
    data: &mut [i16],
    buffer: &Arc<Mutex<AudioPlaybackState>>,
    output_misses: &Arc<AtomicU64>,
) {
    if let Ok(mut buffer) = buffer.try_lock() {
        for sample in data {
            let value = buffer.next_sample().clamp(-1.0, 1.0);
            *sample = (value * i16::MAX as f32).round() as i16;
        }
    } else {
        output_misses.fetch_add(1, Ordering::Relaxed);
        data.fill(0);
    }
}

fn fill_u16_output(
    data: &mut [u16],
    buffer: &Arc<Mutex<AudioPlaybackState>>,
    output_misses: &Arc<AtomicU64>,
) {
    if let Ok(mut buffer) = buffer.try_lock() {
        for sample in data {
            let value = buffer.next_sample().clamp(-1.0, 1.0);
            *sample =
                ((value * i16::MAX as f32).round() as i32 + 32768).clamp(0, u16::MAX as i32) as u16;
        }
    } else {
        output_misses.fetch_add(1, Ordering::Relaxed);
        data.fill(32768);
    }
}

fn pcm_s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
        .collect()
}

fn concealed_output_frames(
    missing_packets: u64,
    input_frames: usize,
    input_rate: u32,
    output_rate: u32,
) -> usize {
    if missing_packets == 0 || input_frames == 0 || input_rate == 0 || output_rate == 0 {
        return 0;
    }
    let frames_per_packet =
        ((input_frames as f64 * output_rate as f64) / input_rate as f64).round() as usize;
    frames_per_packet.saturating_mul(missing_packets as usize)
}

fn resample_and_map_channels(
    input: &[f32],
    input_rate: u32,
    input_channels: u16,
    output_rate: u32,
    output_channels: u16,
    stretch_ratio: f64,
) -> Vec<f32> {
    let input_channels = input_channels.max(1) as usize;
    let output_channels = output_channels.max(1) as usize;
    let input_frames = input.len() / input_channels;
    if input_frames == 0 || input_rate == 0 || output_rate == 0 {
        return Vec::new();
    }
    let base_output_frames = (input_frames as f64 * output_rate as f64) / input_rate as f64;
    let output_frames = (base_output_frames * stretch_ratio.clamp(0.98, 1.02))
        .round()
        .max(1.0) as usize;
    let mut output = Vec::with_capacity(output_frames * output_channels);
    let rate_ratio = if output_frames > 1 && input_frames > 1 {
        (input_frames - 1) as f64 / (output_frames - 1) as f64
    } else {
        1.0
    };
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

fn sequence_generation(seq: u64) -> u64 {
    seq >> 32
}

enum AudioResponse {
    Devices(Vec<AudioDevice>),
    Started {
        message: String,
        generation: Option<u64>,
    },
    Stopped,
    Error(String),
    Other(String),
}

impl AudioResponse {
    fn parse(detail: &str) -> Self {
        let mut lines = detail.lines();
        match lines.next().unwrap_or_default().trim() {
            "audio_listen_devices" => Self::Devices(parse_devices(lines)),
            "audio_listen_started" => Self::Started {
                message: "Client accepted audio listen".to_string(),
                generation: payload_field(detail, "generation")
                    .and_then(|value| value.parse::<u64>().ok()),
            },
            "audio_listen_stopped" => Self::Stopped,
            "audio_listen_error" => Self::Error(
                payload_field(detail, "message")
                    .unwrap_or_else(|| "audio listen failed".to_string()),
            ),
            other => Self::Other(other.to_string()),
        }
    }
}

fn parse_devices<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<AudioDevice> {
    let mut devices = Vec::new();
    for line in lines {
        let parts = line.split('\t').collect::<Vec<_>>();
        if parts.len() < 3 || parts[0] != "device" {
            continue;
        }
        let Some(index) = parts[1].parse::<usize>().ok() else {
            continue;
        };
        devices.push(AudioDevice {
            index,
            name: parts[2].to_string(),
            description: parts.get(3).copied().unwrap_or_default().to_string(),
        });
    }
    devices
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

fn payload_action(payload: &str) -> Option<String> {
    payload_field(payload, "action").map(|value| value.to_ascii_lowercase())
}

fn queue_ui_payload(queued: &Arc<Mutex<Vec<String>>>, payload: String) {
    if let Ok(mut queued) = queued.lock() {
        queued.push(payload);
    }
}

fn device_label(devices: &[AudioDevice], selected: usize) -> String {
    devices
        .iter()
        .find(|device| device.index == selected)
        .map(device_label_one)
        .unwrap_or_else(|| "No input devices".to_string())
}

fn device_label_one(device: &AudioDevice) -> String {
    format!("{}: {}", device.index, device.name)
}

fn identity_title(hostname: &str, username: &str) -> String {
    if username.trim().is_empty() {
        hostname.to_string()
    } else {
        format!("{hostname} / {username}")
    }
}

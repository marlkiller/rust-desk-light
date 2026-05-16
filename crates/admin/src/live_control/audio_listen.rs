use crate::windowing;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);
const TOOLBAR_CONTROL_HEIGHT: f32 = 24.0;
const MAX_AUDIO_BUFFER_MS: usize = 500;
const MIN_AUDIO_PREBUFFER_MS: usize = 80;

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
    player: Option<AudioPlayer>,
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
    if bytes.len() < 2 || bytes.len() % 2 != 0 {
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
    windows: &mut Vec<AudioListenWindow>,
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
    if let Some(player) = &window.player {
        player.push_frame(&frame);
    }
    handle_frame(window, frame);
}

pub(crate) fn open_window(
    windows: &mut Vec<AudioListenWindow>,
    client_id: &str,
    hostname: String,
    username: String,
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    {
        window.open = true;
        window.hostname = hostname;
        window.username = username;
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
        player: None,
    };
    window.queue_devices();
    windows.push(window);
}

pub(crate) fn handle_ack(
    windows: &mut Vec<AudioListenWindow>,
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
        AudioResponse::Started(message) => {
            window.status = AudioStatus::Pending;
            window.notice = message;
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
                .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
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
                    Ok(player) => window.player = Some(player),
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
        .rect_filled(meter_rect, 4.0, egui::Color32::from_rgb(232, 237, 244));
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
                    "{} Hz | {} ch | {} | {} bytes",
                    stats.sample_rate, stats.channels, stats.format, stats.encoded_bytes
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
    window.outbound.clear();
    window.pending_since = None;
    window.player = None;
    window.stats.peak = 0.0;
    window.status = AudioStatus::Ready;
    window.notice = notice.to_string();
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
            |error| eprintln!("audio output stream error: {error}"),
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
            |error| eprintln!("audio output stream error: {error}"),
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
            |error| eprintln!("audio output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
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

enum AudioResponse {
    Devices(Vec<AudioDevice>),
    Started(String),
    Stopped,
    Error(String),
    Other(String),
}

impl AudioResponse {
    fn parse(detail: &str) -> Self {
        let mut lines = detail.lines();
        match lines.next().unwrap_or_default().trim() {
            "audio_listen_devices" => Self::Devices(parse_devices(lines)),
            "audio_listen_started" => Self::Started("Client accepted audio listen".to_string()),
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

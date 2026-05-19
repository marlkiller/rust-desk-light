use crate::{
    i18n::t,
    theme::{COLOR_BAD, COLOR_GOOD, COLOR_WARN},
    windowing,
};
use base64::Engine;
use eframe::egui;
use rfd::FileDialog;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const DEFAULT_QUALITY: &str = "medium";
const DEFAULT_TARGET_FPS: u32 = 5;
const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::COMPACT_CONTROL_HEIGHT;
const QUALITY_DROPDOWN_WIDTH: f32 = 92.0;
const FPS_DROPDOWN_WIDTH: f32 = 74.0;

pub(crate) struct CameraWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    devices: Vec<CameraDevice>,
    selected_device: Arc<Mutex<usize>>,
    quality: Arc<Mutex<String>>,
    target_fps: Arc<Mutex<u32>>,
    frame: Option<CameraFrame>,
    texture: Option<egui::TextureHandle>,
    texture_seq: u64,
    save_path: Arc<Mutex<String>>,
    save_requested: Arc<AtomicBool>,
    status: CameraStatus,
    notice: String,
    stats: CameraStats,
    running: Arc<AtomicBool>,
    outbound: Vec<String>,
    pending_since: Option<Instant>,
    last_request_at: Option<Instant>,
    open: bool,
    close_requested: Arc<AtomicBool>,
}

#[derive(Clone)]
struct CameraDevice {
    index: usize,
    name: String,
    description: String,
}

pub(crate) struct CameraFrame {
    seq: u64,
    width: u32,
    height: u32,
    encoded_bytes: usize,
    format: String,
    image: egui::ColorImage,
    bytes: Vec<u8>,
}

#[derive(Clone, Default)]
struct CameraStats {
    fps: f32,
    frame_count: u64,
    encoded_bytes: usize,
    format: String,
    latency_ms: Option<u128>,
    decode_ms: Option<u128>,
    upload_ms: Option<u128>,
    last_frame_at: Option<Instant>,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
enum CameraStatus {
    Ready,
    Pending,
    Live,
    Failed,
}

pub(crate) struct OutboundCommand {
    pub(crate) client_id: String,
    pub(crate) payload: String,
}

pub(crate) fn decode_frame_payload(detail: &str) -> Result<CameraFrame, String> {
    let mut lines = detail.lines();
    if lines.next().unwrap_or_default().trim() != "camera_frame" {
        return Err("not a camera frame payload".to_string());
    }
    match parse_frame(lines.collect::<Vec<_>>().as_slice()) {
        CameraResponse::Frame(frame) => Ok(frame),
        CameraResponse::Error(message) => Err(message),
        _ => Err("camera payload did not contain a frame".to_string()),
    }
}

pub(crate) fn decode_video_frame(
    seq: u64,
    image_width: u32,
    image_height: u32,
    format: String,
    bytes: Vec<u8>,
) -> Result<CameraFrame, String> {
    if image_width == 0 || image_height == 0 {
        return Err("invalid camera frame metadata".to_string());
    }
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("load camera frame failed: {error}"))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Ok(CameraFrame {
        seq,
        width: image.width(),
        height: image.height(),
        encoded_bytes: bytes.len(),
        format,
        image: color_image,
        bytes,
    })
}

pub(crate) fn handle_decoded_frame(
    windows: &mut [CameraWindow],
    client_id: &str,
    frame: CameraFrame,
    decode_ms: Option<u128>,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if window
        .frame
        .as_ref()
        .map(|current| frame.seq <= current.seq)
        .unwrap_or(false)
    {
        return;
    }
    let latency_ms = window
        .pending_since
        .map(|pending_since| pending_since.elapsed().as_millis());
    window.pending_since = None;
    handle_frame(window, frame, latency_ms, decode_ms);
}

pub(crate) fn open_window(
    windows: &mut Vec<CameraWindow>,
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

    let mut window = CameraWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        devices: Vec::new(),
        selected_device: Arc::new(Mutex::new(0)),
        quality: Arc::new(Mutex::new(DEFAULT_QUALITY.to_string())),
        target_fps: Arc::new(Mutex::new(DEFAULT_TARGET_FPS)),
        frame: None,
        texture: None,
        texture_seq: 0,
        save_path: Arc::new(Mutex::new(default_save_path(client_id))),
        save_requested: Arc::new(AtomicBool::new(false)),
        status: CameraStatus::Ready,
        notice: "Select a camera and click Start".to_string(),
        stats: CameraStats::default(),
        running: Arc::new(AtomicBool::new(false)),
        outbound: Vec::new(),
        pending_since: None,
        last_request_at: None,
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
    };
    window.queue_devices();
    windows.push(window);
}

pub(crate) fn handle_ack(
    windows: &mut [CameraWindow],
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
    let latency_ms = window
        .pending_since
        .map(|pending_since| pending_since.elapsed().as_millis());
    window.pending_since = None;
    if !accepted {
        stop_capture(window, &detail);
        window.status = CameraStatus::Failed;
        return;
    }

    match CameraResponse::parse(&detail) {
        CameraResponse::Devices(devices) => {
            window.devices = devices;
            window.status = CameraStatus::Ready;
            window.notice = if window.devices.is_empty() {
                "No camera devices found".to_string()
            } else {
                "Select a camera and click Start".to_string()
            };
        }
        CameraResponse::Frame(frame) => handle_frame(window, frame, latency_ms, None),
        CameraResponse::Stopped => stop_capture(window, "Stopped"),
        CameraResponse::Error(message) => {
            stop_capture(window, &message);
            window.status = CameraStatus::Failed;
        }
    }
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<CameraWindow>,
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
            stop_capture(window, "Stopped");
            window.open = false;
        }
        if !window.open {
            continue;
        }
        if window
            .pending_since
            .is_some_and(|pending_since| pending_since.elapsed() > Duration::from_secs(10))
        {
            stop_capture(window, t("Timed out waiting for camera result"));
            window.status = CameraStatus::Failed;
        }
        if let Some(frame) = &window.frame {
            if window.texture_seq != frame.seq {
                let upload_started = Instant::now();
                if let Some(texture) = &mut window.texture {
                    texture.set(frame.image.clone(), egui::TextureOptions::LINEAR);
                } else {
                    window.texture = Some(ctx.load_texture(
                        format!("camera:{}", window.client_id),
                        frame.image.clone(),
                        egui::TextureOptions::LINEAR,
                    ));
                }
                window.stats.upload_ms = Some(upload_started.elapsed().as_millis());
                window.texture_seq = frame.seq;
            }
        }
        let title = format!(
            "{} - {}",
            t("Camera"),
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("camera", &window.client_id));
        let builder = windowing::child_viewport_builder(title, [860.0, 640.0], [680.0, 500.0]);

        let client_id = window.client_id.clone();
        let close_requested = window.close_requested.clone();
        let devices = window.devices.clone();
        let selected_device = window.selected_device.clone();
        let quality = window.quality.clone();
        let target_fps = window.target_fps.clone();
        let running = window.running.clone();
        let save_requested = window.save_requested.clone();
        let has_frame = window.frame.is_some();
        let status = window.status;
        let notice = window.notice.clone();
        let stats = window.stats.clone();
        let texture = window.texture.clone();
        let frame_info = window
            .frame
            .as_ref()
            .map(|frame| (frame.width, frame.height));
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
                    render_toolbar(
                        ui,
                        &devices,
                        &selected_device,
                        &quality,
                        &target_fps,
                        &running,
                        &save_requested,
                        has_frame,
                        &queued_for_ui,
                    );
                    ui.add_space(8.0);
                    let frame_height = (ui.available_height() - 84.0).max(160.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), frame_height),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| render_frame(ui, texture.as_ref(), frame_info, &notice),
                    );
                    ui.add_space(8.0);
                    render_status_bar(ui, status, &notice, &stats);
                });
            if running.load(Ordering::Relaxed) {
                ui.ctx().request_repaint_after(frame_interval(
                    target_fps
                        .lock()
                        .map(|value| *value)
                        .unwrap_or(DEFAULT_TARGET_FPS),
                ));
            }
        });

        if let Ok(mut queued) = queued.lock() {
            for payload in queued.drain(..) {
                if payload.trim() == "action=stop" {
                    stop_capture(window, t("Stopped"));
                }
                window.queue_payload(payload);
            }
        }
        if window.save_requested.swap(false, Ordering::Relaxed) {
            save_current_frame(window);
        }
        while let Some(payload) = window.outbound.pop() {
            let stop_payload = payload.trim() == "action=stop";
            if stop_payload {
                window.status = CameraStatus::Ready;
                window.notice = t("Stopped").to_string();
                window.pending_since = None;
            } else if !camera_payload_is_frame_refresh(&payload)
                || !window.running.load(Ordering::Relaxed)
                || window.frame.is_none()
            {
                window.status = CameraStatus::Pending;
                window.notice = t("Waiting for client result").to_string();
                window.pending_since = Some(Instant::now());
            }
            window.last_request_at = Some(Instant::now());
            outbound.push(OutboundCommand {
                client_id: client_id.clone(),
                payload,
            });
        }
    }
    windows.retain(|window| window.open);
    outbound
}

impl CameraWindow {
    fn queue_devices(&mut self) {
        self.queue_payload("action=devices".to_string());
    }

    fn queue_payload(&mut self, payload: String) {
        self.outbound.insert(0, payload);
    }
}

fn render_toolbar(
    ui: &mut egui::Ui,
    devices: &[CameraDevice],
    selected_device: &Arc<Mutex<usize>>,
    quality: &Arc<Mutex<String>>,
    target_fps: &Arc<Mutex<u32>>,
    running: &Arc<AtomicBool>,
    save_requested: &Arc<AtomicBool>,
    has_frame: bool,
    queued: &Arc<Mutex<Vec<String>>>,
) {
    let is_running = running.load(Ordering::Relaxed);
    ui.vertical(|ui| {
        toolbar_row(ui, |ui| {
            let mut selected = selected_device
                .lock()
                .map(|value| *value)
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(t("Device"))
                    .size(12.0)
                    .color(crate::theme::palette().muted),
            );
            let combo_width = (ui.available_width() - 12.0).max(180.0);
            toolbar_dropdown(
                ui,
                "camera_device_select",
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
                .add_enabled(!is_running, egui::Button::new(t("Reload Devices")))
                .clicked()
            {
                queue_ui_payload(queued, "action=devices".to_string());
            }
            ui.separator();
            let mut selected_quality = quality
                .lock()
                .map(|value| value.clone())
                .unwrap_or_else(|_| DEFAULT_QUALITY.to_string());
            toolbar_dropdown(
                ui,
                "camera_quality",
                quality_label(&selected_quality),
                QUALITY_DROPDOWN_WIDTH,
                !is_running,
                |ui| {
                    ui.set_min_width(QUALITY_DROPDOWN_WIDTH);
                    for option in ["low", "medium", "high"] {
                        if ui
                            .selectable_value(
                                &mut selected_quality,
                                option.to_string(),
                                quality_label(option),
                            )
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                },
            );
            if let Ok(mut value) = quality.lock() {
                *value = selected_quality.clone();
            }
            ui.label(
                egui::RichText::new(t("FPS"))
                    .size(12.0)
                    .color(crate::theme::palette().muted),
            );
            let mut selected_fps = target_fps
                .lock()
                .map(|value| *value)
                .unwrap_or(DEFAULT_TARGET_FPS);
            toolbar_dropdown(
                ui,
                "camera_fps",
                fps_label(selected_fps),
                FPS_DROPDOWN_WIDTH,
                !is_running,
                |ui| {
                    ui.set_min_width(FPS_DROPDOWN_WIDTH);
                    for option in [2_u32, 5, 8, 10, 12] {
                        if ui
                            .selectable_value(&mut selected_fps, option, fps_label(option))
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                },
            );
            if let Ok(mut value) = target_fps.lock() {
                *value = selected_fps;
            }
            ui.separator();
            let selected = selected_device
                .lock()
                .map(|value| *value)
                .unwrap_or_default();
            if ui
                .add_enabled(
                    !devices.is_empty(),
                    egui::Button::new(if is_running {
                        t("Stop Capture")
                    } else {
                        t("Start Capture")
                    }),
                )
                .clicked()
            {
                if is_running {
                    running.store(false, Ordering::Relaxed);
                    queue_ui_payload(queued, "action=stop".to_string());
                } else {
                    running.store(true, Ordering::Relaxed);
                    queue_ui_payload(
                        queued,
                        format!(
                            "action=start\ndevice={selected}\nquality={selected_quality}\nfps={selected_fps}"
                        ),
                    );
                }
            }
            ui.separator();
            if ui
                .add_enabled(has_frame, egui::Button::new(t("Save Frame...")))
                .clicked()
            {
                save_requested.store(true, Ordering::Relaxed);
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

fn render_frame(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    frame_info: Option<(u32, u32)>,
    placeholder: &str,
) {
    crate::theme::panel_frame_with_margin(8.0).show(ui, |ui| {
        let available = ui.available_size();
        let Some(texture) = texture else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new(placeholder).color(crate::theme::palette().muted));
            });
            return;
        };
        let Some((width, height)) = frame_info else {
            return;
        };
        let scale = (available.x / width as f32)
            .min(available.y / height as f32)
            .max(0.1);
        let size = egui::vec2(width as f32 * scale, height as f32 * scale);
        ui.add(egui::Image::new(texture).fit_to_exact_size(size));
    });
}

fn render_status_bar(ui: &mut egui::Ui, status: CameraStatus, notice: &str, stats: &CameraStats) {
    let (label, color) = match status {
        CameraStatus::Ready => (t("Ready"), crate::theme::palette().muted),
        CameraStatus::Pending => (t("Pending"), COLOR_WARN),
        CameraStatus::Live => (t("Live"), COLOR_GOOD),
        CameraStatus::Failed => (t("Failed"), COLOR_BAD),
    };
    crate::theme::status_frame().show(ui, |ui| {
        crate::theme::render_status_line(ui, label, color, notice, |ui| {
            ui.separator();
            ui.label(crate::theme::muted_text(format!("FPS {:.1}", stats.fps)));
            ui.label(crate::theme::muted_text(format!(
                "{} {}",
                t("Frames"),
                stats.frame_count
            )));
            if stats.width > 0 && stats.height > 0 {
                ui.label(crate::theme::muted_text(format!(
                    "{}x{}",
                    stats.width, stats.height
                )));
            }
            if stats.encoded_bytes > 0 {
                ui.label(crate::theme::muted_text(format!(
                    "{} {}",
                    human_bytes(stats.encoded_bytes),
                    stats.format
                )));
            }
            if let Some(decode_ms) = stats.decode_ms {
                ui.label(crate::theme::muted_text(format!(
                    "{} {} ms",
                    t("Decode"),
                    decode_ms
                )));
            }
            if let Some(upload_ms) = stats.upload_ms {
                ui.label(crate::theme::muted_text(format!(
                    "{} {} ms",
                    t("Texture"),
                    upload_ms
                )));
            }
            if let Some(latency_ms) = stats.latency_ms {
                ui.label(crate::theme::muted_text(format!("RTT {} ms", latency_ms)));
            }
        });
    });
}

fn handle_frame(
    window: &mut CameraWindow,
    frame: CameraFrame,
    latency_ms: Option<u128>,
    decode_ms: Option<u128>,
) {
    if !window.running.load(Ordering::Relaxed) {
        return;
    }
    let now = Instant::now();
    window.stats.fps = window
        .stats
        .last_frame_at
        .map(|last_frame_at| {
            let elapsed = now.duration_since(last_frame_at).as_secs_f32();
            if elapsed > 0.0 {
                1.0 / elapsed
            } else {
                window.stats.fps
            }
        })
        .unwrap_or(0.0);
    window.stats.frame_count = window.stats.frame_count.saturating_add(1);
    window.stats.encoded_bytes = frame.encoded_bytes;
    window.stats.format = frame.format.clone();
    window.stats.latency_ms = latency_ms;
    window.stats.decode_ms = decode_ms;
    window.stats.last_frame_at = Some(now);
    window.stats.width = frame.width;
    window.stats.height = frame.height;
    window.frame = Some(frame);
    window.status = CameraStatus::Live;
    window.notice = t("Frame received").to_string();
}

fn stop_capture(window: &mut CameraWindow, notice: &str) {
    window.running.store(false, Ordering::Relaxed);
    window.outbound.clear();
    window.pending_since = None;
    window.last_request_at = None;
    window.stats.fps = 0.0;
    window.stats.latency_ms = None;
    window.stats.decode_ms = None;
    window.stats.upload_ms = None;
    window.status = CameraStatus::Ready;
    window.notice = notice.to_string();
}

fn save_current_frame(window: &mut CameraWindow) {
    let Some(frame) = &window.frame else {
        window.notice = t("No camera frame to save").to_string();
        window.status = CameraStatus::Failed;
        return;
    };
    let default_path = window
        .save_path
        .lock()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let default_path = if default_path.is_empty() {
        default_save_path(&window.client_id)
    } else {
        default_path
    };
    let Some(path_buf) = pick_frame_save_path(&default_path, &frame.format) else {
        window.notice = t("Save frame cancelled").to_string();
        return;
    };
    let path_buf = ensure_frame_extension(path_buf, &frame.format);
    if let Some(parent) = path_buf.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                window.notice = format!("{}: {error}", t("Create save directory failed"));
                window.status = CameraStatus::Failed;
                return;
            }
        }
    }
    match std::fs::write(&path_buf, &frame.bytes) {
        Ok(()) => {
            window.notice = format!("{} {}", t("Saved frame to"), path_buf.display());
            window.status = CameraStatus::Live;
            if let Ok(mut value) = window.save_path.lock() {
                *value = path_buf.to_string_lossy().to_string();
            }
        }
        Err(error) => {
            window.notice = format!("{}: {error}", t("Save frame failed"));
            window.status = CameraStatus::Failed;
        }
    }
}

fn pick_frame_save_path(default_path: &str, format: &str) -> Option<PathBuf> {
    let default_path = PathBuf::from(default_path);
    let mut dialog = FileDialog::new().set_title(t("Save camera frame"));
    if let Some(parent) = default_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        dialog = dialog.set_directory(parent);
    }
    if let Some(file_name) = default_path.file_name().and_then(|value| value.to_str()) {
        dialog = dialog.set_file_name(file_name);
    }
    match normalized_image_extension(format).as_str() {
        "png" => dialog.add_filter("PNG image", &["png"]).save_file(),
        _ => dialog
            .add_filter("JPEG image", &["jpg", "jpeg"])
            .save_file(),
    }
}

fn ensure_frame_extension(mut path: PathBuf, format: &str) -> PathBuf {
    if path.extension().is_none() {
        path.set_extension(normalized_image_extension(format));
    }
    path
}

fn normalized_image_extension(format: &str) -> String {
    if format.eq_ignore_ascii_case("png") {
        "png".to_string()
    } else {
        "jpg".to_string()
    }
}

enum CameraResponse {
    Devices(Vec<CameraDevice>),
    Frame(CameraFrame),
    Stopped,
    Error(String),
}

impl CameraResponse {
    fn parse(detail: &str) -> Self {
        let mut lines = detail.lines();
        match lines.next().unwrap_or_default().trim() {
            "camera_devices" => parse_devices(lines.collect::<Vec<_>>().as_slice()),
            "camera_frame" => parse_frame(lines.collect::<Vec<_>>().as_slice()),
            "camera_stopped" => Self::Stopped,
            "camera_error" => {
                let message = detail
                    .lines()
                    .find_map(|line| line.strip_prefix("message="))
                    .unwrap_or("camera error")
                    .to_string();
                Self::Error(message)
            }
            _ => Self::Error(detail.to_string()),
        }
    }
}

fn parse_devices(lines: &[&str]) -> CameraResponse {
    let mut devices = Vec::new();
    for line in lines {
        let parts = line.split('\t').collect::<Vec<_>>();
        if parts.len() < 3 || parts[0] != "device" {
            continue;
        }
        devices.push(CameraDevice {
            index: parts[1].parse().unwrap_or_default(),
            name: parts.get(2).copied().unwrap_or_default().to_string(),
            description: parts.get(3).copied().unwrap_or_default().to_string(),
        });
    }
    CameraResponse::Devices(devices)
}

fn parse_frame(lines: &[&str]) -> CameraResponse {
    let mut width = 0;
    let mut height = 0;
    let mut encoded_bytes = 0;
    let mut format = "image".to_string();
    let mut image_base64 = "";
    for line in lines {
        if let Some(rest) = line.strip_prefix("width=") {
            width = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("height=") {
            height = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("bytes=") {
            encoded_bytes = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("format=") {
            format = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("image_base64=") {
            image_base64 = rest;
        }
    }
    if width == 0 || height == 0 {
        return CameraResponse::Error("invalid camera frame metadata".to_string());
    }
    let bytes = match base64::engine::general_purpose::STANDARD.decode(image_base64) {
        Ok(bytes) => bytes,
        Err(error) => return CameraResponse::Error(format!("decode camera frame failed: {error}")),
    };
    let image = match image::load_from_memory(&bytes) {
        Ok(image) => image.to_rgba8(),
        Err(error) => return CameraResponse::Error(format!("load camera frame failed: {error}")),
    };
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    CameraResponse::Frame(CameraFrame {
        seq: rdl_protocol::now_epoch_ms() as u64,
        width: image.width(),
        height: image.height(),
        encoded_bytes,
        format,
        image: color_image,
        bytes,
    })
}

fn device_label(devices: &[CameraDevice], selected: usize) -> String {
    devices
        .iter()
        .find(|device| device.index == selected)
        .map(device_label_one)
        .unwrap_or_else(|| "No camera".to_string())
}

fn device_label_one(device: &CameraDevice) -> String {
    let name = truncate_label(device.name.trim(), 56);
    format!("Camera {} - {}", device.index, name)
}

fn truncate_label(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut shortened = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    shortened.push_str("...");
    shortened
}

fn quality_label(value: &str) -> &'static str {
    match value {
        "low" => t("Low"),
        "high" => t("High"),
        _ => t("Medium"),
    }
}

fn fps_label(value: u32) -> String {
    format!("{value} {}", t("FPS"))
}

fn frame_interval(target_fps: u32) -> Duration {
    Duration::from_millis((1000 / target_fps.clamp(1, 12) as u64).max(1))
}

fn queue_ui_payload(queue: &Arc<Mutex<Vec<String>>>, payload: String) {
    if let Ok(mut queue) = queue.lock() {
        queue.push(payload);
    }
}

fn camera_payload_is_frame_refresh(payload: &str) -> bool {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| action.trim() == "capture")
        .unwrap_or(false)
}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f32 / 1024.0 / 1024.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f32 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

fn default_save_path(client_id: &str) -> String {
    let file_name = format!(
        "camera-{}-{}.jpg",
        sanitize_file_component(client_id),
        rdl_protocol::now_epoch_ms()
    );
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(file_name)
        .to_string_lossy()
        .to_string()
}

fn sanitize_file_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "client".to_string()
    } else {
        sanitized
    }
}

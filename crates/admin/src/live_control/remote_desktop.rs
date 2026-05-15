use base64::Engine;
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);
const DEFAULT_QUALITY: &str = "medium";
const MOUSE_MOVE_INTERVAL: Duration = Duration::from_millis(33);

pub(crate) struct RemoteDesktopWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    frame: Option<DesktopFrame>,
    texture: Option<egui::TextureHandle>,
    texture_seq: u64,
    status: DesktopStatus,
    notice: String,
    stats: DesktopStats,
    screens: Vec<RemoteScreen>,
    selected_screen: Arc<Mutex<usize>>,
    quality: Arc<Mutex<String>>,
    mouse_follow: Arc<AtomicBool>,
    mouse_click: Arc<AtomicBool>,
    last_mouse_move: Arc<Mutex<Instant>>,
    running: Arc<AtomicBool>,
    outbound: Vec<String>,
    pending_since: Option<Instant>,
    open: bool,
    close_requested: Arc<AtomicBool>,
}

pub(crate) struct DesktopFrame {
    seq: u64,
    screen_width: u32,
    screen_height: u32,
    image_width: usize,
    image_height: usize,
    encoded_bytes: usize,
    format: String,
    image: egui::ColorImage,
}

pub(crate) fn decode_frame_payload(detail: &str) -> Result<DesktopFrame, String> {
    let mut lines = detail.lines();
    if lines.next().unwrap_or_default().trim() != "remote_desktop_frame" {
        return Err("not a remote desktop frame payload".to_string());
    }
    match parse_frame(lines.collect::<Vec<_>>().as_slice()) {
        DesktopResponse::Frame(frame) => Ok(frame),
        DesktopResponse::Error(message) => Err(message),
        _ => Err("remote desktop payload did not contain a frame".to_string()),
    }
}

pub(crate) fn handle_decoded_frame(
    windows: &mut Vec<RemoteDesktopWindow>,
    client_id: &str,
    frame: DesktopFrame,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    handle_frame(window, frame, None);
}

fn handle_frame(window: &mut RemoteDesktopWindow, frame: DesktopFrame, latency_ms: Option<u128>) {
    let is_running = window.running.load(Ordering::Relaxed);
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
    window.stats.last_frame_at = Some(now);
    window.stats.screen_width = frame.screen_width;
    window.stats.screen_height = frame.screen_height;
    window.frame = Some(frame);
    if is_running {
        window.status = DesktopStatus::Live;
        window.notice = "Frame received".to_string();
    } else {
        window.status = DesktopStatus::Ready;
        window.notice = "Stopped".to_string();
    }
}

#[derive(Clone, Default)]
struct DesktopStats {
    fps: f32,
    frame_count: u64,
    encoded_bytes: usize,
    format: String,
    latency_ms: Option<u128>,
    last_frame_at: Option<Instant>,
    screen_width: u32,
    screen_height: u32,
}

#[derive(Clone)]
struct RemoteScreen {
    index: usize,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    primary: bool,
    name: String,
}

#[derive(Clone, Copy)]
enum DesktopStatus {
    Ready,
    Pending,
    Live,
    Failed,
}

pub(crate) struct OutboundCommand {
    pub(crate) client_id: String,
    pub(crate) payload: String,
    pub(crate) input: bool,
}

pub(crate) fn open_window(
    windows: &mut Vec<RemoteDesktopWindow>,
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
        window.queue_screens();
        return;
    }

    let mut window = RemoteDesktopWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        frame: None,
        texture: None,
        texture_seq: 0,
        status: DesktopStatus::Ready,
        notice: "Select a screen and click Start".to_string(),
        stats: DesktopStats::default(),
        screens: Vec::new(),
        selected_screen: Arc::new(Mutex::new(0)),
        quality: Arc::new(Mutex::new(DEFAULT_QUALITY.to_string())),
        mouse_follow: Arc::new(AtomicBool::new(false)),
        mouse_click: Arc::new(AtomicBool::new(false)),
        last_mouse_move: Arc::new(Mutex::new(Instant::now())),
        running: Arc::new(AtomicBool::new(false)),
        outbound: Vec::new(),
        pending_since: None,
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
    };
    window.queue_screens();
    windows.push(window);
}

pub(crate) fn handle_ack(
    windows: &mut Vec<RemoteDesktopWindow>,
    client_id: &str,
    _hostname: String,
    _username: String,
    accepted: bool,
    detail: String,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if !accepted {
        window.status = DesktopStatus::Failed;
        window.notice = detail;
        window.pending_since = None;
        window.running.store(false, Ordering::Relaxed);
        return;
    }
    let latency_ms = window
        .pending_since
        .map(|pending_since| pending_since.elapsed().as_millis());
    window.pending_since = None;
    match DesktopResponse::parse(&detail) {
        DesktopResponse::Screens(screens) => {
            window.screens = screens;
            window.status = DesktopStatus::Ready;
            window.notice = if window.screens.is_empty() {
                "No remote screens found".to_string()
            } else {
                "Select a screen and click Start".to_string()
            };
        }
        DesktopResponse::Frame(frame) => {
            handle_frame(window, frame, latency_ms);
        }
        DesktopResponse::Input(message) => {
            window.status = DesktopStatus::Live;
            window.notice = message;
        }
        DesktopResponse::Error(message) => {
            window.status = DesktopStatus::Failed;
            window.notice = message;
            window.running.store(false, Ordering::Relaxed);
        }
        DesktopResponse::Stopped => {
            window.status = DesktopStatus::Ready;
            window.notice = "Stopped".to_string();
            window.pending_since = None;
            window.running.store(false, Ordering::Relaxed);
        }
    }
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<RemoteDesktopWindow>,
) -> Vec<OutboundCommand> {
    let mut outbound = Vec::new();
    for window in windows.iter_mut() {
        if window.close_requested.load(Ordering::Relaxed) {
            if window.running.swap(false, Ordering::Relaxed) {
                outbound.push(OutboundCommand {
                    client_id: window.client_id.clone(),
                    payload: "action=stop".to_string(),
                    input: false,
                });
            }
            window.outbound.clear();
            window.pending_since = None;
            window.open = false;
        }
        if !window.open {
            continue;
        }
        if matches!(window.status, DesktopStatus::Pending)
            && window
                .pending_since
                .is_some_and(|pending_since| pending_since.elapsed() > Duration::from_secs(10))
        {
            window.status = DesktopStatus::Failed;
            window.notice = "Timed out waiting for remote desktop result".to_string();
            window.pending_since = None;
            window.running.store(false, Ordering::Relaxed);
        }
        if let Some(frame) = &window.frame {
            if window.texture_seq != frame.seq {
                if let Some(texture) = &mut window.texture {
                    texture.set(frame.image.clone(), egui::TextureOptions::LINEAR);
                } else {
                    window.texture = Some(ctx.load_texture(
                        format!("remote_desktop:{}", window.client_id),
                        frame.image.clone(),
                        egui::TextureOptions::LINEAR,
                    ));
                }
                window.texture_seq = frame.seq;
            }
        }

        let title = format!(
            "Remote Desktop - {}",
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("remote_desktop", &window.client_id));
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([760.0, 520.0])
            .with_resizable(true);

        let client_id = window.client_id.clone();
        let close_requested = window.close_requested.clone();
        let texture = window.texture.clone();
        let frame_info = window.frame.as_ref().map(|frame| {
            (
                frame.screen_width,
                frame.screen_height,
                frame.image_width,
                frame.image_height,
            )
        });
        let selected_index = window
            .selected_screen
            .lock()
            .map(|value| *value)
            .unwrap_or_default();
        let screen_origin = window
            .screens
            .iter()
            .find(|screen| screen.index == selected_index)
            .map(|screen| (screen.x, screen.y))
            .unwrap_or((0, 0));
        let status = window.status;
        let notice = window.notice.clone();
        let stats = window.stats.clone();
        let screens = window.screens.clone();
        let selected_screen = window.selected_screen.clone();
        let quality = window.quality.clone();
        let mouse_follow = window.mouse_follow.clone();
        let mouse_click = window.mouse_click.clone();
        let last_mouse_move = window.last_mouse_move.clone();
        let running = window.running.clone();
        let queued = Arc::new(Mutex::new(Vec::new()));
        let queued_for_ui = queued.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
                .show_inside(ui, |ui| {
                    render_toolbar(
                        ui,
                        &screens,
                        &selected_screen,
                        &quality,
                        &mouse_follow,
                        &mouse_click,
                        &running,
                        &queued_for_ui,
                    );
                    ui.add_space(8.0);
                    let frame_height = (ui.available_height() - 52.0).max(160.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), frame_height),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            render_frame(
                                ui,
                                texture.as_ref(),
                                frame_info,
                                screen_origin,
                                &mouse_follow,
                                &mouse_click,
                                &last_mouse_move,
                                &notice,
                                &queued_for_ui,
                            );
                        },
                    );
                    ui.add_space(8.0);
                    render_status_bar(ui, status, &notice, &stats);
                });
            if running.load(Ordering::Relaxed) {
                ui.ctx().request_repaint_after(frame_interval(
                    quality
                        .lock()
                        .map(|value| quality_fps(&value))
                        .unwrap_or_else(|_| quality_fps(DEFAULT_QUALITY)),
                ));
            }
        });

        if let Ok(mut queued) = queued.lock() {
            for payload in queued.drain(..) {
                if payload.trim() == "action=stop" {
                    window.outbound.clear();
                    window.pending_since = None;
                    window.status = DesktopStatus::Ready;
                    window.notice = "Stopping".to_string();
                    window.running.store(false, Ordering::Relaxed);
                    window.mouse_follow.store(false, Ordering::Relaxed);
                    window.mouse_click.store(false, Ordering::Relaxed);
                }
                window.queue_payload(payload);
            }
        }
        while let Some(payload) = window.outbound.pop() {
            let input = remote_desktop_payload_is_input(&payload);
            if !input {
                window.status = DesktopStatus::Pending;
                window.notice = if payload.trim() == "action=stop" {
                    "Stopping remote desktop".to_string()
                } else {
                    "Waiting for client result".to_string()
                };
                window.pending_since = Some(Instant::now());
            }
            outbound.push(OutboundCommand {
                client_id: client_id.clone(),
                payload,
                input,
            });
        }
    }
    windows.retain(|window| window.open);
    outbound
}

impl RemoteDesktopWindow {
    fn queue_screens(&mut self) {
        self.queue_payload("action=screens".to_string());
    }

    fn queue_payload(&mut self, payload: String) {
        self.outbound.insert(0, payload);
    }
}

fn render_toolbar(
    ui: &mut egui::Ui,
    screens: &[RemoteScreen],
    selected_screen: &Arc<Mutex<usize>>,
    quality: &Arc<Mutex<String>>,
    mouse_follow: &Arc<AtomicBool>,
    mouse_click: &Arc<AtomicBool>,
    running: &Arc<AtomicBool>,
    queued: &Arc<Mutex<Vec<String>>>,
) {
    ui.horizontal(|ui| {
        let mut selected = selected_screen
            .lock()
            .map(|value| *value)
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("remote_desktop_screen_select")
            .selected_text(screen_label(screens, selected))
            .show_ui(ui, |ui| {
                for screen in screens {
                    ui.selectable_value(&mut selected, screen.index, screen_label_one(screen));
                }
            });
        if let Ok(mut value) = selected_screen.lock() {
            *value = selected;
        }
        if ui.button("Reload Screens").clicked() {
            queue_ui_payload(queued, "action=screens".to_string());
        }
        let mut selected_quality = quality
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| DEFAULT_QUALITY.to_string());
        egui::ComboBox::from_id_salt("remote_desktop_quality")
            .selected_text(quality_label(&selected_quality))
            .show_ui(ui, |ui| {
                for option in ["low", "medium", "high"] {
                    ui.selectable_value(
                        &mut selected_quality,
                        option.to_string(),
                        quality_label(option),
                    );
                }
            });
        if let Ok(mut value) = quality.lock() {
            *value = selected_quality.clone();
        }
        let is_running = running.load(Ordering::Relaxed);
        if ui
            .add_enabled(
                !screens.is_empty(),
                egui::Button::new(if is_running {
                    "Stop Capture"
                } else {
                    "Start Capture"
                }),
            )
            .clicked()
        {
            if is_running {
                running.store(false, Ordering::Relaxed);
                mouse_follow.store(false, Ordering::Relaxed);
                mouse_click.store(false, Ordering::Relaxed);
                queue_ui_payload(queued, "action=stop".to_string());
            } else {
                running.store(true, Ordering::Relaxed);
                queue_ui_payload(
                    queued,
                    format!("action=start\nscreen={selected}\nquality={selected_quality}"),
                );
            }
        }
        let mut follow = mouse_follow.load(Ordering::Relaxed);
        if ui
            .add_enabled(is_running, egui::Checkbox::new(&mut follow, "Mouse Move"))
            .changed()
        {
            mouse_follow.store(follow, Ordering::Relaxed);
        }
        let mut click = mouse_click.load(Ordering::Relaxed);
        if ui
            .add_enabled(is_running, egui::Checkbox::new(&mut click, "Mouse Click"))
            .changed()
        {
            mouse_click.store(click, Ordering::Relaxed);
        }
    });
}

fn screen_label(screens: &[RemoteScreen], selected: usize) -> String {
    screens
        .iter()
        .find(|screen| screen.index == selected)
        .map(screen_label_one)
        .unwrap_or_else(|| "No screen".to_string())
}

fn screen_label_one(screen: &RemoteScreen) -> String {
    let suffix = if screen.primary { " primary" } else { "" };
    let name = if screen.name.trim().is_empty() {
        String::new()
    } else {
        format!(" {}", screen.name.trim())
    };
    format!(
        "Screen {}{} - {}x{}{}",
        screen.index, name, screen.width, screen.height, suffix
    )
}

fn render_frame(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    frame_info: Option<(u32, u32, usize, usize)>,
    screen_origin: (i32, i32),
    mouse_follow: &Arc<AtomicBool>,
    mouse_click: &Arc<AtomicBool>,
    last_mouse_move: &Arc<Mutex<Instant>>,
    placeholder: &str,
    queued: &Arc<Mutex<Vec<String>>>,
) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(8.0)
        .show(ui, |ui| {
            let available = ui.available_size();
            let Some(texture) = texture else {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new(placeholder).color(COLOR_MUTED));
                });
                return;
            };
            let Some((screen_w, screen_h, image_w, image_h)) = frame_info else {
                return;
            };
            let scale = (available.x / image_w as f32)
                .min(available.y / image_h as f32)
                .max(0.1);
            let size = egui::vec2(image_w as f32 * scale, image_h as f32 * scale);
            let image = egui::Image::new(texture)
                .fit_to_exact_size(size)
                .sense(egui::Sense::click_and_drag());
            let response = ui.add(image);
            if mouse_follow.load(Ordering::Relaxed) && response.hovered() {
                let should_send = last_mouse_move
                    .lock()
                    .map(|mut last| {
                        if last.elapsed() >= MOUSE_MOVE_INTERVAL {
                            *last = Instant::now();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(true);
                if should_send {
                    if let Some(pos) = response.hover_pos() {
                        let rel_x = ((pos.x - response.rect.left()) / response.rect.width())
                            .clamp(0.0, 1.0);
                        let rel_y = ((pos.y - response.rect.top()) / response.rect.height())
                            .clamp(0.0, 1.0);
                        let x = screen_origin.0 + (rel_x * screen_w as f32).round() as i32;
                        let y = screen_origin.1 + (rel_y * screen_h as f32).round() as i32;
                        queue_ui_payload(queued, format!("action=move\nx={x}\ny={y}"));
                    }
                }
            }
            if mouse_click.load(Ordering::Relaxed)
                && response.clicked_by(egui::PointerButton::Primary)
            {
                if let Some(pos) = response.interact_pointer_pos() {
                    let rel_x =
                        ((pos.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0);
                    let rel_y =
                        ((pos.y - response.rect.top()) / response.rect.height()).clamp(0.0, 1.0);
                    let x = screen_origin.0 + (rel_x * screen_w as f32).round() as i32;
                    let y = screen_origin.1 + (rel_y * screen_h as f32).round() as i32;
                    queue_ui_payload(queued, format!("action=click\nbutton=left\nx={x}\ny={y}"));
                }
            }
            if mouse_click.load(Ordering::Relaxed)
                && response.clicked_by(egui::PointerButton::Secondary)
            {
                if let Some(pos) = response.interact_pointer_pos() {
                    let rel_x =
                        ((pos.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0);
                    let rel_y =
                        ((pos.y - response.rect.top()) / response.rect.height()).clamp(0.0, 1.0);
                    let x = screen_origin.0 + (rel_x * screen_w as f32).round() as i32;
                    let y = screen_origin.1 + (rel_y * screen_h as f32).round() as i32;
                    queue_ui_payload(queued, format!("action=click\nbutton=right\nx={x}\ny={y}"));
                }
            }
        });
}

fn render_status_bar(ui: &mut egui::Ui, status: DesktopStatus, notice: &str, stats: &DesktopStats) {
    let (label, color) = match status {
        DesktopStatus::Ready => ("Ready", COLOR_MUTED),
        DesktopStatus::Pending => ("Pending", COLOR_WARN),
        DesktopStatus::Live => ("Live", COLOR_GOOD),
        DesktopStatus::Failed => ("Failed", COLOR_BAD),
    };
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(
                    egui::RichText::new(label)
                        .size(12.0)
                        .color(COLOR_TEXT)
                        .strong(),
                );
                ui.label(egui::RichText::new(notice).size(12.0).color(COLOR_MUTED));
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("FPS {:.1}", stats.fps))
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.label(
                    egui::RichText::new(format!("Frames {}", stats.frame_count))
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                if stats.screen_width > 0 && stats.screen_height > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}x{}",
                            stats.screen_width, stats.screen_height
                        ))
                        .size(12.0)
                        .color(COLOR_MUTED),
                    );
                }
                if stats.encoded_bytes > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}",
                            human_bytes(stats.encoded_bytes),
                            stats.format
                        ))
                        .size(12.0)
                        .color(COLOR_MUTED),
                    );
                }
                if let Some(latency_ms) = stats.latency_ms {
                    ui.label(
                        egui::RichText::new(format!("RTT {} ms", latency_ms))
                            .size(12.0)
                            .color(COLOR_MUTED),
                    );
                }
            });
        });
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

fn frame_interval(target_fps: u32) -> Duration {
    let fps = target_fps.clamp(1, 12);
    Duration::from_millis((1000 / fps as u64).max(1))
}

fn quality_label(value: &str) -> &'static str {
    match value {
        "low" => "Low",
        "high" => "High",
        _ => "Medium",
    }
}

fn quality_fps(value: &str) -> u32 {
    match value {
        "low" => 8,
        "high" => 3,
        _ => 5,
    }
}

fn remote_desktop_payload_is_input(payload: &str) -> bool {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| matches!(action.trim(), "click" | "move" | "text"))
        .unwrap_or(false)
}

fn queue_ui_payload(queue: &Arc<Mutex<Vec<String>>>, payload: String) {
    if let Ok(mut queue) = queue.lock() {
        queue.push(payload);
    }
}

enum DesktopResponse {
    Screens(Vec<RemoteScreen>),
    Frame(DesktopFrame),
    Input(String),
    Error(String),
    Stopped,
}

impl DesktopResponse {
    fn parse(detail: &str) -> Self {
        let mut lines = detail.lines();
        match lines.next().unwrap_or_default().trim() {
            "remote_desktop_screens" => parse_screens(lines.collect::<Vec<_>>().as_slice()),
            "remote_desktop_frame" => parse_frame(lines.collect::<Vec<_>>().as_slice()),
            "remote_desktop_input" => {
                let message = detail
                    .lines()
                    .find_map(|line| line.strip_prefix("message="))
                    .unwrap_or("input accepted")
                    .to_string();
                Self::Input(message)
            }
            "remote_desktop_stopped" => Self::Stopped,
            "remote_desktop_error" => {
                let message = detail
                    .lines()
                    .find_map(|line| line.strip_prefix("message="))
                    .unwrap_or("remote desktop error")
                    .to_string();
                Self::Error(message)
            }
            _ => Self::Error(detail.to_string()),
        }
    }
}

fn parse_screens(lines: &[&str]) -> DesktopResponse {
    let mut screens = Vec::new();
    for line in lines {
        let parts = line.split('\t').collect::<Vec<_>>();
        if parts.len() < 7 || parts[0] != "screen" {
            continue;
        }
        screens.push(RemoteScreen {
            index: parts[1].parse().unwrap_or_default(),
            x: parts[2].parse().unwrap_or_default(),
            y: parts[3].parse().unwrap_or_default(),
            width: parts[4].parse().unwrap_or_default(),
            height: parts[5].parse().unwrap_or_default(),
            primary: parts[6] == "true",
            name: parts.get(7).copied().unwrap_or_default().to_string(),
        });
    }
    DesktopResponse::Screens(screens)
}

fn parse_frame(lines: &[&str]) -> DesktopResponse {
    let mut screen_width = 0;
    let mut screen_height = 0;
    let mut image_width = 0;
    let mut image_height = 0;
    let mut encoded_bytes = 0;
    let mut format = "image".to_string();
    let mut png_base64 = "";
    for line in lines {
        if let Some(rest) = line.strip_prefix("screen_width=") {
            screen_width = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("screen_height=") {
            screen_height = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("image_width=") {
            image_width = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("image_height=") {
            image_height = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("bytes=") {
            encoded_bytes = rest.parse().unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("format=") {
            format = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("png_base64=") {
            png_base64 = rest;
        }
    }
    if screen_width == 0 || screen_height == 0 || image_width == 0 || image_height == 0 {
        return DesktopResponse::Error("invalid remote frame metadata".to_string());
    }
    let bytes = match base64::engine::general_purpose::STANDARD.decode(png_base64) {
        Ok(bytes) => bytes,
        Err(error) => return DesktopResponse::Error(format!("decode frame failed: {error}")),
    };
    let image = match image::load_from_memory(&bytes) {
        Ok(image) => image.to_rgba8(),
        Err(error) => return DesktopResponse::Error(format!("load frame failed: {error}")),
    };
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    DesktopResponse::Frame(DesktopFrame {
        seq: rdl_protocol::now_epoch_ms() as u64,
        screen_width,
        screen_height,
        image_width: size[0],
        image_height: size[1],
        encoded_bytes,
        format,
        image: color_image,
    })
}

fn identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

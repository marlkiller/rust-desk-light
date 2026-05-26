use crate::{
    i18n::t,
    theme::{color_bad, color_good, color_warn},
    windowing,
};
use eframe::egui;
use std::path::Path;
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
    target_fps: Arc<Mutex<u32>>,
    mouse_control: Arc<AtomicBool>,
    mouse_drag: Arc<Mutex<MouseDragState>>,
    keyboard_control: Arc<AtomicBool>,
    keyboard_shortcuts: Arc<Mutex<KeyboardShortcutState>>,
    last_mouse_move: Arc<Mutex<Instant>>,
    last_mouse_target: Arc<Mutex<Option<(i32, i32)>>>,
    running: Arc<AtomicBool>,
    outbound: Vec<String>,
    pending_since: Option<Instant>,
    open: bool,
    close_requested: Arc<AtomicBool>,
}

#[derive(Clone)]
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

pub(crate) type DesktopFrameDecodeState = crate::live_control::tile_frame::DecodeState;

#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_video_frame_with_state(
    state: &mut DesktopFrameDecodeState,
    seq: u64,
    source_width: u32,
    source_height: u32,
    image_width: u32,
    image_height: u32,
    format: String,
    bytes: Vec<u8>,
) -> Result<Option<DesktopFrame>, String> {
    if source_width == 0 || source_height == 0 || image_width == 0 || image_height == 0 {
        return Err("invalid remote frame metadata".to_string());
    }
    if format == crate::live_control::tile_frame::FORMAT {
        return crate::live_control::tile_frame::decode_frame(
            state,
            image_width,
            image_height,
            &bytes,
        )
        .map(|decoded| {
            decoded.map(|decoded| {
                let size = [decoded.width as usize, decoded.height as usize];
                DesktopFrame {
                    seq,
                    screen_width: source_width,
                    screen_height: source_height,
                    image_width: size[0],
                    image_height: size[1],
                    encoded_bytes: bytes.len(),
                    format,
                    image: egui::ColorImage::from_rgba_unmultiplied(size, &decoded.rgba),
                }
            })
        });
    }
    state.reset();
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("load frame failed: {error}"))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Ok(Some(DesktopFrame {
        seq,
        screen_width: source_width,
        screen_height: source_height,
        image_width: size[0],
        image_height: size[1],
        encoded_bytes: bytes.len(),
        format,
        image: color_image,
    }))
}

pub(crate) fn handle_decoded_frame(
    windows: &mut [RemoteDesktopWindow],
    client_id: &str,
    frame: DesktopFrame,
    decode_ms: Option<u128>,
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
    if window
        .frame
        .as_ref()
        .map(|current| frame.seq <= current.seq)
        .unwrap_or(false)
    {
        return;
    }
    handle_frame(window, frame, None, decode_ms);
}

fn handle_frame(
    window: &mut RemoteDesktopWindow,
    frame: DesktopFrame,
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
    window.stats.screen_width = frame.screen_width;
    window.stats.screen_height = frame.screen_height;
    window.stats.image_width = frame.image_width;
    window.stats.image_height = frame.image_height;
    window.frame = Some(frame);
    window.status = DesktopStatus::Live;
    window.notice = t("Frame received").to_string();
}

fn stop_capture(window: &mut RemoteDesktopWindow, notice: &str) {
    window.running.store(false, Ordering::Relaxed);
    window.outbound.clear();
    if let Ok(mut target) = window.last_mouse_target.lock() {
        *target = None;
    }
    if let Ok(mut drag) = window.mouse_drag.lock() {
        *drag = MouseDragState::default();
    }
    if let Ok(mut shortcuts) = window.keyboard_shortcuts.lock() {
        *shortcuts = KeyboardShortcutState::default();
    }
    window.pending_since = None;
    window.stats.fps = 0.0;
    window.stats.latency_ms = None;
    window.stats.decode_ms = None;
    window.stats.upload_ms = None;
    window.status = DesktopStatus::Ready;
    window.notice = notice.to_string();
}

#[derive(Clone, Default)]
struct DesktopStats {
    fps: f32,
    frame_count: u64,
    encoded_bytes: usize,
    format: String,
    latency_ms: Option<u128>,
    decode_ms: Option<u128>,
    upload_ms: Option<u128>,
    last_frame_at: Option<Instant>,
    screen_width: u32,
    screen_height: u32,
    image_width: usize,
    image_height: usize,
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

#[derive(Clone, Copy, Default)]
struct MouseDragState {
    active: bool,
    last_point: Option<(i32, i32)>,
}

#[derive(Default)]
struct KeyboardShortcutState {
    copy_active: bool,
    cut_active: bool,
}

impl MouseDragState {
    fn start(&mut self, point: (i32, i32)) {
        self.active = true;
        self.last_point = Some(point);
    }

    fn update(&mut self, point: (i32, i32)) {
        self.last_point = Some(point);
    }

    fn stop(&mut self) -> Option<(i32, i32)> {
        if !self.active {
            return None;
        }
        self.active = false;
        self.last_point
    }
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
        target_fps: Arc::new(Mutex::new(DEFAULT_TARGET_FPS)),
        mouse_control: Arc::new(AtomicBool::new(false)),
        mouse_drag: Arc::new(Mutex::new(MouseDragState::default())),
        keyboard_control: Arc::new(AtomicBool::new(false)),
        keyboard_shortcuts: Arc::new(Mutex::new(KeyboardShortcutState::default())),
        last_mouse_move: Arc::new(Mutex::new(Instant::now())),
        last_mouse_target: Arc::new(Mutex::new(None)),
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
    windows: &mut [RemoteDesktopWindow],
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
        stop_capture(window, &detail);
        window.status = DesktopStatus::Failed;
        return;
    }
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
        DesktopResponse::Input(message) => {
            window.status = DesktopStatus::Live;
            window.notice = message;
        }
        DesktopResponse::Error(message) => {
            stop_capture(window, &message);
            window.status = DesktopStatus::Failed;
        }
        DesktopResponse::Stopped => {
            stop_capture(window, t("Stopped"));
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
            if let Some(payload) = take_drag_release_payload(window) {
                outbound.push(OutboundCommand {
                    client_id: window.client_id.clone(),
                    payload,
                    input: true,
                });
            }
            if window.running.load(Ordering::Relaxed) || window.pending_since.is_some() {
                outbound.push(OutboundCommand {
                    client_id: window.client_id.clone(),
                    payload: "action=stop".to_string(),
                    input: false,
                });
            }
            stop_capture(window, t("Stopped"));
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
            window.notice = t("Timed out waiting for remote desktop result").to_string();
            window.pending_since = None;
            stop_capture(window, t("Timed out waiting for remote desktop result"));
            window.status = DesktopStatus::Failed;
        }
        if let Some(frame) = &window.frame {
            if window.texture_seq != frame.seq {
                let upload_started = Instant::now();
                if let Some(texture) = &mut window.texture {
                    texture.set(frame.image.clone(), egui::TextureOptions::LINEAR);
                } else {
                    window.texture = Some(ctx.load_texture(
                        format!("remote_desktop:{}", window.client_id),
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
            t("Remote Desktop"),
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("remote_desktop", &window.client_id));
        let builder = windowing::child_viewport_builder(title, [980.0, 680.0], [760.0, 520.0]);

        let client_id = window.client_id.clone();
        let close_requested = window.close_requested.clone();
        let texture = window.texture.clone();
        let frame_for_save = window.frame.clone();
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
        let target_fps = window.target_fps.clone();
        let mouse_control = window.mouse_control.clone();
        let mouse_drag = window.mouse_drag.clone();
        let keyboard_control = window.keyboard_control.clone();
        let keyboard_shortcuts = window.keyboard_shortcuts.clone();
        let last_mouse_move = window.last_mouse_move.clone();
        let last_mouse_target = window.last_mouse_target.clone();
        let running = window.running.clone();
        let queued = Arc::new(Mutex::new(Vec::new()));
        let queued_for_ui = queued.clone();
        let save_notice = Arc::new(Mutex::new(None::<String>));
        let save_notice_for_ui = save_notice.clone();

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
                        &screens,
                        &selected_screen,
                        &quality,
                        &target_fps,
                        &mouse_control,
                        &keyboard_control,
                        &running,
                        &queued_for_ui,
                        frame_for_save.as_ref(),
                        &save_notice_for_ui,
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
                                &mouse_control,
                                &mouse_drag,
                                &keyboard_control,
                                &keyboard_shortcuts,
                                &last_mouse_move,
                                &last_mouse_target,
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
                    let release_payload = take_drag_release_payload(window);
                    stop_capture(window, t("Stopped"));
                    if let Some(release_payload) = release_payload {
                        window.queue_payload(release_payload);
                    }
                }
                window.queue_payload(payload);
            }
        }
        if let Some(notice) = save_notice.lock().ok().and_then(|mut value| value.take()) {
            window.notice = notice;
        }
        while let Some(payload) = window.outbound.pop() {
            let input = remote_desktop_payload_is_input(&payload);
            let stop_payload = payload.trim() == "action=stop";
            if stop_payload {
                window.status = DesktopStatus::Ready;
                window.notice = t("Stopped").to_string();
                window.pending_since = None;
            } else if !input {
                window.status = DesktopStatus::Pending;
                window.notice = t("Waiting for client result").to_string();
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

fn take_drag_release_payload(window: &RemoteDesktopWindow) -> Option<String> {
    window
        .mouse_drag
        .lock()
        .ok()
        .and_then(|mut drag| drag.stop())
        .map(|(x, y)| format!("action=mouse_up\nbutton=left\nx={x}\ny={y}"))
}

fn render_toolbar(
    ui: &mut egui::Ui,
    screens: &[RemoteScreen],
    selected_screen: &Arc<Mutex<usize>>,
    quality: &Arc<Mutex<String>>,
    target_fps: &Arc<Mutex<u32>>,
    mouse_control: &Arc<AtomicBool>,
    keyboard_control: &Arc<AtomicBool>,
    running: &Arc<AtomicBool>,
    queued: &Arc<Mutex<Vec<String>>>,
    latest_frame: Option<&DesktopFrame>,
    save_notice: &Arc<Mutex<Option<String>>>,
) {
    ui.vertical(|ui| {
        let is_running = running.load(Ordering::Relaxed);
        toolbar_row(ui, |ui| {
            let mut selected = selected_screen
                .lock()
                .map(|value| *value)
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(t("Screen"))
                    .size(12.0)
                    .color(crate::theme::palette().muted),
            );
            let combo_width = (ui.available_width() - 12.0).max(180.0);
            toolbar_dropdown(
                ui,
                "remote_desktop_screen_select",
                screen_label(screens, selected),
                combo_width,
                !is_running,
                |ui| {
                    ui.set_min_width(combo_width);
                    for screen in screens {
                        if ui
                            .selectable_value(&mut selected, screen.index, screen_label_one(screen))
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                },
            );
            if let Ok(mut value) = selected_screen.lock() {
                *value = selected;
            }
        });
        toolbar_row(ui, |ui| {
            if ui.button(t("Reload Screens")).clicked() {
                queue_ui_payload(queued, "action=screens".to_string());
            }
            if ui
                .add_enabled(latest_frame.is_some(), egui::Button::new(t("Save Frame")))
                .clicked()
            {
                if let Some(frame) = latest_frame {
                    if let Some(message) = save_frame_dialog(frame) {
                        if let Ok(mut notice) = save_notice.lock() {
                            *notice = Some(message);
                        }
                    }
                }
            }
            ui.separator();
            let mut selected_quality = quality
                .lock()
                .map(|value| value.clone())
                .unwrap_or_else(|_| DEFAULT_QUALITY.to_string());
            toolbar_dropdown(
                ui,
                "remote_desktop_quality",
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
                "remote_desktop_fps",
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
            let selected = selected_screen
                .lock()
                .map(|value| *value)
                .unwrap_or_default();
            if ui
                .add_enabled(
                    !screens.is_empty(),
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
                            "action=start\nscreen={selected}\nquality={selected_quality}\nfps={selected_fps}\ndiff=tiles_v1"
                        ),
                    );
                }
            }
            let mut mouse = mouse_control.load(Ordering::Relaxed);
            if ui
                .add_enabled(is_running, egui::Checkbox::new(&mut mouse, t("Mouse")))
                .changed()
            {
                mouse_control.store(mouse, Ordering::Relaxed);
            }
            let mut keyboard = keyboard_control.load(Ordering::Relaxed);
            if ui
                .add_enabled(
                    is_running,
                    egui::Checkbox::new(&mut keyboard, t("Keyboard")),
                )
                .changed()
            {
                keyboard_control.store(keyboard, Ordering::Relaxed);
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

fn screen_label(screens: &[RemoteScreen], selected: usize) -> String {
    screens
        .iter()
        .find(|screen| screen.index == selected)
        .map(screen_label_one)
        .unwrap_or_else(|| t("No screen").to_string())
}

fn screen_label_one(screen: &RemoteScreen) -> String {
    let suffix = if screen.primary {
        format!(" {}", t("primary"))
    } else {
        String::new()
    };
    let name = if screen.name.trim().is_empty() {
        String::new()
    } else {
        format!(" {}", screen.name.trim())
    };
    format!(
        "{} {}{} - {}x{}{}",
        t("Screen"),
        screen.index,
        name,
        screen.width,
        screen.height,
        suffix
    )
}

fn render_frame(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    frame_info: Option<(u32, u32, usize, usize)>,
    screen_origin: (i32, i32),
    mouse_control: &Arc<AtomicBool>,
    mouse_drag: &Arc<Mutex<MouseDragState>>,
    keyboard_control: &Arc<AtomicBool>,
    keyboard_shortcuts: &Arc<Mutex<KeyboardShortcutState>>,
    last_mouse_move: &Arc<Mutex<Instant>>,
    last_mouse_target: &Arc<Mutex<Option<(i32, i32)>>>,
    placeholder: &str,
    queued: &Arc<Mutex<Vec<String>>>,
) {
    crate::theme::panel_frame_with_margin(8.0).show(ui, |ui| {
        let available = ui.available_size();
        let Some(texture) = texture else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new(placeholder).color(crate::theme::palette().muted));
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
        if response.clicked() && keyboard_control.load(Ordering::Relaxed) {
            response.request_focus();
        }
        queue_keyboard_events(ui, &response, keyboard_control, keyboard_shortcuts, queued);
        queue_mouse_events(
            ui,
            &response,
            screen_origin,
            screen_w,
            screen_h,
            mouse_control,
            mouse_drag,
            last_mouse_move,
            last_mouse_target,
            queued,
        );
    });
}

fn queue_mouse_events(
    ui: &egui::Ui,
    response: &egui::Response,
    screen_origin: (i32, i32),
    screen_w: u32,
    screen_h: u32,
    mouse_control: &Arc<AtomicBool>,
    mouse_drag: &Arc<Mutex<MouseDragState>>,
    last_mouse_move: &Arc<Mutex<Instant>>,
    last_mouse_target: &Arc<Mutex<Option<(i32, i32)>>>,
    queued: &Arc<Mutex<Vec<String>>>,
) {
    let mouse_enabled = mouse_control.load(Ordering::Relaxed);
    if !mouse_enabled {
        release_mouse_drag(mouse_drag, queued);
        set_last_mouse_target(last_mouse_target, None);
        return;
    }

    let pointer_pos = ui.input(|input| input.pointer.interact_pos());
    let primary_down = ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
    let primary_started_here = primary_down && response.is_pointer_button_down_on();

    if primary_started_here {
        if let Some(pos) = pointer_pos {
            let (x, y) = pointer_remote_point(response, pos, screen_origin, screen_w, screen_h);
            let mut start_drag = false;
            if let Ok(mut drag) = mouse_drag.lock() {
                if !drag.active {
                    drag.start((x, y));
                    start_drag = true;
                }
            }
            if start_drag {
                set_last_mouse_target(last_mouse_target, Some((x, y)));
                queue_ui_payload(
                    queued,
                    format!("action=mouse_down\nbutton=left\nx={x}\ny={y}"),
                );
            }
        }
    }

    let dragging = mouse_drag.lock().map(|drag| drag.active).unwrap_or(false);
    if dragging {
        if primary_down {
            if let Some(pos) = pointer_pos {
                let (x, y) = pointer_remote_point(response, pos, screen_origin, screen_w, screen_h);
                if mouse_target_changed(last_mouse_target, (x, y))
                    && mouse_move_due(last_mouse_move)
                {
                    if let Ok(mut drag) = mouse_drag.lock() {
                        drag.update((x, y));
                    }
                    set_last_mouse_target(last_mouse_target, Some((x, y)));
                    queue_ui_payload(queued, format!("action=move\nx={x}\ny={y}"));
                }
            }
        } else {
            if let Some(pos) = pointer_pos {
                let (x, y) = pointer_remote_point(response, pos, screen_origin, screen_w, screen_h);
                if let Ok(mut drag) = mouse_drag.lock() {
                    drag.update((x, y));
                }
            }
            release_mouse_drag(mouse_drag, queued);
        }
        return;
    }

    if response.hovered() {
        if let Some((x, y)) = hovered_remote_point(response, screen_origin, screen_w, screen_h) {
            if mouse_target_changed(last_mouse_target, (x, y)) && mouse_move_due(last_mouse_move) {
                set_last_mouse_target(last_mouse_target, Some((x, y)));
                queue_ui_payload(queued, format!("action=move\nx={x}\ny={y}"));
            }
        }
    } else {
        set_last_mouse_target(last_mouse_target, None);
    }

    if response.clicked_by(egui::PointerButton::Primary) {
        if let Some(pos) = response.interact_pointer_pos() {
            let (x, y) = pointer_remote_point(response, pos, screen_origin, screen_w, screen_h);
            queue_ui_payload(queued, format!("action=click\nbutton=left\nx={x}\ny={y}"));
        }
    }
    if response.clicked_by(egui::PointerButton::Secondary) {
        if let Some(pos) = response.interact_pointer_pos() {
            let (x, y) = pointer_remote_point(response, pos, screen_origin, screen_w, screen_h);
            queue_ui_payload(queued, format!("action=click\nbutton=right\nx={x}\ny={y}"));
        }
    }
}

fn release_mouse_drag(mouse_drag: &Arc<Mutex<MouseDragState>>, queued: &Arc<Mutex<Vec<String>>>) {
    let point = mouse_drag.lock().ok().and_then(|mut drag| drag.stop());
    if let Some((x, y)) = point {
        queue_ui_payload(
            queued,
            format!("action=mouse_up\nbutton=left\nx={x}\ny={y}"),
        );
    }
}

fn hovered_remote_point(
    response: &egui::Response,
    screen_origin: (i32, i32),
    screen_w: u32,
    screen_h: u32,
) -> Option<(i32, i32)> {
    let pos = response.hover_pos()?;
    Some(pointer_remote_point(
        response,
        pos,
        screen_origin,
        screen_w,
        screen_h,
    ))
}

fn pointer_remote_point(
    response: &egui::Response,
    pos: egui::Pos2,
    screen_origin: (i32, i32),
    screen_w: u32,
    screen_h: u32,
) -> (i32, i32) {
    let rel_x = ((pos.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0);
    let rel_y = ((pos.y - response.rect.top()) / response.rect.height()).clamp(0.0, 1.0);
    let x = screen_origin.0 + (rel_x * screen_w as f32).round() as i32;
    let y = screen_origin.1 + (rel_y * screen_h as f32).round() as i32;
    (x, y)
}

fn queue_keyboard_events(
    ui: &egui::Ui,
    response: &egui::Response,
    keyboard_control: &Arc<AtomicBool>,
    keyboard_shortcuts: &Arc<Mutex<KeyboardShortcutState>>,
    queued: &Arc<Mutex<Vec<String>>>,
) {
    if !keyboard_control.load(Ordering::Relaxed) || !response.has_focus() {
        clear_keyboard_shortcuts(keyboard_shortcuts);
        return;
    }
    let events = ui.input(|input| input.events.clone());
    let modifier_down = ui.input(|input| input.modifiers.ctrl || input.modifiers.command);
    if !modifier_down {
        clear_keyboard_shortcuts(keyboard_shortcuts);
    }
    let has_raw_copy_key = events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::C,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command
        )
    });
    let has_raw_cut_key = events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: egui::Key::X,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command
        )
    });
    for event in events {
        match event {
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                if !text.is_empty() {
                    queue_ui_payload(queued, keyboard_text_payload(&text));
                }
            }
            egui::Event::Paste(text) => {
                if !text.is_empty() {
                    queue_ui_payload(queued, keyboard_text_payload(&text));
                }
            }
            egui::Event::Copy => {
                if !has_raw_copy_key && take_semantic_shortcut(keyboard_shortcuts, "copy") {
                    queue_ui_payload(
                        queued,
                        keyboard_key_payload("c", false, egui::Modifiers::COMMAND),
                    );
                }
            }
            egui::Event::Cut => {
                if !has_raw_cut_key && take_semantic_shortcut(keyboard_shortcuts, "cut") {
                    queue_ui_payload(
                        queued,
                        keyboard_key_payload("x", false, egui::Modifiers::COMMAND),
                    );
                }
            }
            egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                if !pressed || repeat || !keyboard_key_should_send(key, modifiers) {
                    continue;
                }
                if let Some(name) = keyboard_key_name(key) {
                    queue_ui_payload(queued, keyboard_key_payload(name, repeat, modifiers));
                }
            }
            _ => {}
        }
    }
}

fn clear_keyboard_shortcuts(keyboard_shortcuts: &Arc<Mutex<KeyboardShortcutState>>) {
    if let Ok(mut shortcuts) = keyboard_shortcuts.lock() {
        *shortcuts = KeyboardShortcutState::default();
    }
}

fn take_semantic_shortcut(
    keyboard_shortcuts: &Arc<Mutex<KeyboardShortcutState>>,
    shortcut: &str,
) -> bool {
    let Ok(mut shortcuts) = keyboard_shortcuts.lock() else {
        return true;
    };
    match shortcut {
        "copy" if !shortcuts.copy_active => {
            shortcuts.copy_active = true;
            true
        }
        "cut" if !shortcuts.cut_active => {
            shortcuts.cut_active = true;
            true
        }
        _ => false,
    }
}

fn keyboard_text_payload(text: &str) -> String {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, text);
    format!("action=text\nvalue_b64={encoded}")
}

fn keyboard_key_payload(name: &str, repeat: bool, modifiers: egui::Modifiers) -> String {
    let ctrl = modifiers.ctrl && !modifiers.command;
    format!(
        "action=key\nkey={name}\npressed=true\nrepeat={repeat}\nshift={}\nctrl={ctrl}\nalt={}\ncommand={}",
        modifiers.shift, modifiers.alt, modifiers.command
    )
}

fn keyboard_key_should_send(key: egui::Key, modifiers: egui::Modifiers) -> bool {
    !keyboard_key_is_printable(key) || modifiers.ctrl || modifiers.alt || modifiers.command
}

fn keyboard_key_is_printable(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::Space
            | egui::Key::Colon
            | egui::Key::Comma
            | egui::Key::Backslash
            | egui::Key::Slash
            | egui::Key::Pipe
            | egui::Key::Questionmark
            | egui::Key::Exclamationmark
            | egui::Key::OpenBracket
            | egui::Key::CloseBracket
            | egui::Key::OpenCurlyBracket
            | egui::Key::CloseCurlyBracket
            | egui::Key::Backtick
            | egui::Key::Minus
            | egui::Key::Period
            | egui::Key::Plus
            | egui::Key::Equals
            | egui::Key::Semicolon
            | egui::Key::Quote
            | egui::Key::Num0
            | egui::Key::Num1
            | egui::Key::Num2
            | egui::Key::Num3
            | egui::Key::Num4
            | egui::Key::Num5
            | egui::Key::Num6
            | egui::Key::Num7
            | egui::Key::Num8
            | egui::Key::Num9
            | egui::Key::A
            | egui::Key::B
            | egui::Key::C
            | egui::Key::D
            | egui::Key::E
            | egui::Key::F
            | egui::Key::G
            | egui::Key::H
            | egui::Key::I
            | egui::Key::J
            | egui::Key::K
            | egui::Key::L
            | egui::Key::M
            | egui::Key::N
            | egui::Key::O
            | egui::Key::P
            | egui::Key::Q
            | egui::Key::R
            | egui::Key::S
            | egui::Key::T
            | egui::Key::U
            | egui::Key::V
            | egui::Key::W
            | egui::Key::X
            | egui::Key::Y
            | egui::Key::Z
    )
}

fn keyboard_key_name(key: egui::Key) -> Option<&'static str> {
    Some(match key {
        egui::Key::ArrowDown => "arrow_down",
        egui::Key::ArrowLeft => "arrow_left",
        egui::Key::ArrowRight => "arrow_right",
        egui::Key::ArrowUp => "arrow_up",
        egui::Key::Escape => "escape",
        egui::Key::Tab => "tab",
        egui::Key::Backspace => "backspace",
        egui::Key::Enter => "enter",
        egui::Key::Space => "space",
        egui::Key::Insert => "insert",
        egui::Key::Delete => "delete",
        egui::Key::Home => "home",
        egui::Key::End => "end",
        egui::Key::PageUp => "page_up",
        egui::Key::PageDown => "page_down",
        egui::Key::Colon => "colon",
        egui::Key::Comma => "comma",
        egui::Key::Backslash => "backslash",
        egui::Key::Slash => "slash",
        egui::Key::Pipe => "pipe",
        egui::Key::Questionmark => "questionmark",
        egui::Key::Exclamationmark => "exclamationmark",
        egui::Key::OpenBracket => "open_bracket",
        egui::Key::CloseBracket => "close_bracket",
        egui::Key::OpenCurlyBracket => "open_curly_bracket",
        egui::Key::CloseCurlyBracket => "close_curly_bracket",
        egui::Key::Backtick => "backtick",
        egui::Key::Minus => "minus",
        egui::Key::Period => "period",
        egui::Key::Plus => "plus",
        egui::Key::Equals => "equals",
        egui::Key::Semicolon => "semicolon",
        egui::Key::Quote => "quote",
        egui::Key::Num0 => "0",
        egui::Key::Num1 => "1",
        egui::Key::Num2 => "2",
        egui::Key::Num3 => "3",
        egui::Key::Num4 => "4",
        egui::Key::Num5 => "5",
        egui::Key::Num6 => "6",
        egui::Key::Num7 => "7",
        egui::Key::Num8 => "8",
        egui::Key::Num9 => "9",
        egui::Key::A => "a",
        egui::Key::B => "b",
        egui::Key::C => "c",
        egui::Key::D => "d",
        egui::Key::E => "e",
        egui::Key::F => "f",
        egui::Key::G => "g",
        egui::Key::H => "h",
        egui::Key::I => "i",
        egui::Key::J => "j",
        egui::Key::K => "k",
        egui::Key::L => "l",
        egui::Key::M => "m",
        egui::Key::N => "n",
        egui::Key::O => "o",
        egui::Key::P => "p",
        egui::Key::Q => "q",
        egui::Key::R => "r",
        egui::Key::S => "s",
        egui::Key::T => "t",
        egui::Key::U => "u",
        egui::Key::V => "v",
        egui::Key::W => "w",
        egui::Key::X => "x",
        egui::Key::Y => "y",
        egui::Key::Z => "z",
        egui::Key::F1 => "f1",
        egui::Key::F2 => "f2",
        egui::Key::F3 => "f3",
        egui::Key::F4 => "f4",
        egui::Key::F5 => "f5",
        egui::Key::F6 => "f6",
        egui::Key::F7 => "f7",
        egui::Key::F8 => "f8",
        egui::Key::F9 => "f9",
        egui::Key::F10 => "f10",
        egui::Key::F11 => "f11",
        egui::Key::F12 => "f12",
        egui::Key::F13 => "f13",
        egui::Key::F14 => "f14",
        egui::Key::F15 => "f15",
        egui::Key::F16 => "f16",
        egui::Key::F17 => "f17",
        egui::Key::F18 => "f18",
        egui::Key::F19 => "f19",
        egui::Key::F20 => "f20",
        egui::Key::F21 => "f21",
        egui::Key::F22 => "f22",
        egui::Key::F23 => "f23",
        egui::Key::F24 => "f24",
        egui::Key::F25 => "f25",
        egui::Key::F26 => "f26",
        egui::Key::F27 => "f27",
        egui::Key::F28 => "f28",
        egui::Key::F29 => "f29",
        egui::Key::F30 => "f30",
        egui::Key::F31 => "f31",
        egui::Key::F32 => "f32",
        egui::Key::F33 => "f33",
        egui::Key::F34 => "f34",
        egui::Key::F35 => "f35",
        egui::Key::BrowserBack => "browser_back",
        egui::Key::Copy | egui::Key::Cut | egui::Key::Paste => return None,
    })
}

fn mouse_target_changed(
    last_mouse_target: &Arc<Mutex<Option<(i32, i32)>>>,
    target: (i32, i32),
) -> bool {
    last_mouse_target
        .lock()
        .map(|last| last.map(|value| value != target).unwrap_or(true))
        .unwrap_or(true)
}

fn mouse_move_due(last_mouse_move: &Arc<Mutex<Instant>>) -> bool {
    last_mouse_move
        .lock()
        .map(|mut last| {
            if last.elapsed() >= MOUSE_MOVE_INTERVAL {
                *last = Instant::now();
                true
            } else {
                false
            }
        })
        .unwrap_or(true)
}

fn set_last_mouse_target(
    last_mouse_target: &Arc<Mutex<Option<(i32, i32)>>>,
    target: Option<(i32, i32)>,
) {
    if let Ok(mut last) = last_mouse_target.lock() {
        *last = target;
    }
}

fn render_status_bar(ui: &mut egui::Ui, status: DesktopStatus, notice: &str, stats: &DesktopStats) {
    let (label, color) = match status {
        DesktopStatus::Ready => (t("Ready"), crate::theme::palette().muted),
        DesktopStatus::Pending => (t("Pending"), color_warn()),
        DesktopStatus::Live => (t("Live"), color_good()),
        DesktopStatus::Failed => (t("Failed"), color_bad()),
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
            if stats.screen_width > 0 && stats.screen_height > 0 {
                let size = if stats.image_width > 0
                    && stats.image_height > 0
                    && (stats.image_width as u32 != stats.screen_width
                        || stats.image_height as u32 != stats.screen_height)
                {
                    format!(
                        "{}x{} -> {}x{}",
                        stats.screen_width,
                        stats.screen_height,
                        stats.image_width,
                        stats.image_height
                    )
                } else {
                    format!("{}x{}", stats.screen_width, stats.screen_height)
                };
                ui.label(crate::theme::muted_text(size));
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
        "low" => t("Low"),
        "high" => t("High"),
        _ => t("Medium"),
    }
}

fn fps_label(value: u32) -> String {
    format!("{value} {}", t("FPS"))
}

fn remote_desktop_payload_is_input(payload: &str) -> bool {
    payload
        .lines()
        .find_map(|line| line.strip_prefix("action="))
        .map(|action| {
            matches!(
                action.trim(),
                "click" | "move" | "mouse_down" | "mouse_up" | "text" | "key"
            )
        })
        .unwrap_or(false)
}

fn queue_ui_payload(queue: &Arc<Mutex<Vec<String>>>, payload: String) {
    if let Ok(mut queue) = queue.lock() {
        queue.push(payload);
    }
}

fn save_frame_dialog(frame: &DesktopFrame) -> Option<String> {
    let path = rfd::FileDialog::new()
        .set_title("Save Remote Desktop Frame")
        .add_filter("PNG image", &["png"])
        .set_file_name(format!("remote-desktop-frame-{}.png", frame.seq))
        .save_file()?;

    match save_frame_to_path(frame, &path) {
        Ok(()) => Some(format!("{} {}", t("Saved frame to"), path.display())),
        Err(error) => Some(format!("{}: {error}", t("Save frame failed"))),
    }
}

fn save_frame_to_path(frame: &DesktopFrame, path: &Path) -> Result<(), String> {
    let mut rgba = Vec::with_capacity(frame.image.pixels.len() * 4);
    for pixel in &frame.image.pixels {
        rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    let image =
        image::RgbaImage::from_raw(frame.image_width as u32, frame.image_height as u32, rgba)
            .ok_or_else(|| "invalid frame image buffer".to_string())?;
    image
        .save(path)
        .map_err(|error| format!("{}: {error}", path.display()))
}

enum DesktopResponse {
    Screens(Vec<RemoteScreen>),
    Input(String),
    Error(String),
    Stopped,
}

impl DesktopResponse {
    fn parse(detail: &str) -> Self {
        let mut lines = detail.lines();
        match lines.next().unwrap_or_default().trim() {
            "remote_desktop_screens" => parse_screens(lines.collect::<Vec<_>>().as_slice()),
            "remote_desktop_frame" => {
                Self::Error("legacy remote desktop frame payload is not supported".to_string())
            }
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

fn identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

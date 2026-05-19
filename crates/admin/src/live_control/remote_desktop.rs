use crate::{
    i18n::t,
    theme::{COLOR_BAD, COLOR_GOOD, COLOR_WARN},
    windowing,
};
use base64::Engine;
use eframe::egui;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const DEFAULT_QUALITY: &str = "medium";
const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::COMPACT_CONTROL_HEIGHT;
const QUALITY_DROPDOWN_WIDTH: f32 = 92.0;
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

pub(crate) fn decode_video_frame(
    seq: u64,
    source_width: u32,
    source_height: u32,
    image_width: u32,
    image_height: u32,
    format: String,
    bytes: Vec<u8>,
) -> Result<DesktopFrame, String> {
    if source_width == 0 || source_height == 0 || image_width == 0 || image_height == 0 {
        return Err("invalid remote frame metadata".to_string());
    }
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("load frame failed: {error}"))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Ok(DesktopFrame {
        seq,
        screen_width: source_width,
        screen_height: source_height,
        image_width: size[0],
        image_height: size[1],
        encoded_bytes: bytes.len(),
        format,
        image: color_image,
    })
}

pub(crate) fn handle_decoded_frame(
    windows: &mut [RemoteDesktopWindow],
    client_id: &str,
    frame: DesktopFrame,
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
    handle_frame(window, frame, None);
}

fn handle_frame(window: &mut RemoteDesktopWindow, frame: DesktopFrame, latency_ms: Option<u128>) {
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
    window.stats.last_frame_at = Some(now);
    window.stats.screen_width = frame.screen_width;
    window.stats.screen_height = frame.screen_height;
    window.frame = Some(frame);
    window.status = DesktopStatus::Live;
    window.notice = "Frame received".to_string();
}

fn stop_capture(window: &mut RemoteDesktopWindow, notice: &str) {
    window.running.store(false, Ordering::Relaxed);
    window.mouse_follow.store(false, Ordering::Relaxed);
    window.mouse_click.store(false, Ordering::Relaxed);
    window.outbound.clear();
    if let Ok(mut target) = window.last_mouse_target.lock() {
        *target = None;
    }
    window.pending_since = None;
    window.stats.fps = 0.0;
    window.stats.latency_ms = None;
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
        let mouse_follow = window.mouse_follow.clone();
        let mouse_click = window.mouse_click.clone();
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
                        &mouse_follow,
                        &mouse_click,
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
                                &mouse_follow,
                                &mouse_click,
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
                    stop_capture(window, t("Stopped"));
                }
                window.queue_payload(payload);
            }
        }
        if let Some(notice) = save_notice.lock().ok().and_then(|mut value| value.take()) {
            window.notice = notice;
        }
        while let Some(payload) = window.outbound.pop() {
            let input = remote_desktop_payload_is_input(&payload);
            if !input {
                window.status = DesktopStatus::Pending;
                window.notice = if payload.trim() == "action=stop" {
                    t("Stopping remote desktop").to_string()
                } else {
                    t("Waiting for client result").to_string()
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
                .add_enabled(
                    is_running,
                    egui::Checkbox::new(&mut follow, t("Mouse Move")),
                )
                .changed()
            {
                mouse_follow.store(follow, Ordering::Relaxed);
            }
            let mut click = mouse_click.load(Ordering::Relaxed);
            if ui
                .add_enabled(
                    is_running,
                    egui::Checkbox::new(&mut click, t("Mouse Click")),
                )
                .changed()
            {
                mouse_click.store(click, Ordering::Relaxed);
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
    mouse_follow: &Arc<AtomicBool>,
    mouse_click: &Arc<AtomicBool>,
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
        if mouse_follow.load(Ordering::Relaxed) && response.hovered() {
            if let Some((x, y)) = hovered_remote_point(&response, screen_origin, screen_w, screen_h)
            {
                if mouse_target_changed(last_mouse_target, (x, y))
                    && mouse_move_due(last_mouse_move)
                {
                    set_last_mouse_target(last_mouse_target, Some((x, y)));
                    queue_ui_payload(queued, format!("action=move\nx={x}\ny={y}"));
                }
            }
        } else {
            set_last_mouse_target(last_mouse_target, None);
        }
        if mouse_click.load(Ordering::Relaxed) && response.clicked_by(egui::PointerButton::Primary)
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

fn hovered_remote_point(
    response: &egui::Response,
    screen_origin: (i32, i32),
    screen_w: u32,
    screen_h: u32,
) -> Option<(i32, i32)> {
    let pos = response.hover_pos()?;
    let rel_x = ((pos.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0);
    let rel_y = ((pos.y - response.rect.top()) / response.rect.height()).clamp(0.0, 1.0);
    let x = screen_origin.0 + (rel_x * screen_w as f32).round() as i32;
    let y = screen_origin.1 + (rel_y * screen_h as f32).round() as i32;
    Some((x, y))
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
        DesktopStatus::Pending => (t("Pending"), COLOR_WARN),
        DesktopStatus::Live => (t("Live"), COLOR_GOOD),
        DesktopStatus::Failed => (t("Failed"), COLOR_BAD),
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
                ui.label(crate::theme::muted_text(format!(
                    "{}x{}",
                    stats.screen_width, stats.screen_height
                )));
            }
            if stats.encoded_bytes > 0 {
                ui.label(crate::theme::muted_text(format!(
                    "{} {}",
                    human_bytes(stats.encoded_bytes),
                    stats.format
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

fn quality_fps(value: &str) -> u32 {
    match value {
        "low" => 10,
        "high" => 2,
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

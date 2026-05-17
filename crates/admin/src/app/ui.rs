use eframe::egui;
use std::sync::Arc;

pub(super) const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(247, 249, 252);
pub(super) const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
pub(super) const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(228, 233, 241);
pub(super) const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
pub(super) const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(98, 111, 130);
pub(super) const COLOR_ACCENT: egui::Color32 = egui::Color32::from_rgb(35, 99, 188);
pub(super) const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
pub(super) const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
pub(super) const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);
pub(super) const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;
const ACTIVITY_LOG_LIMIT: usize = 300;

pub(super) fn apply_admin_theme(ctx: &egui::Context) {
    install_cjk_font(ctx);

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.visuals = egui::Visuals::light();
    style.visuals.window_fill = COLOR_PANEL;
    style.visuals.panel_fill = COLOR_BG;
    style.visuals.widgets.noninteractive.fg_stroke.color = COLOR_TEXT;
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(243, 246, 250);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(235, 241, 248);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(226, 235, 247);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(235, 244, 255);
    style.visuals.selection.stroke.color = COLOR_ACCENT;
    ctx.set_global_style(style);
}

fn install_cjk_font(ctx: &egui::Context) {
    let Some(font_bytes) = load_system_cjk_font() else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    let font_name = "rdl_cjk_fallback".to_string();
    fonts.font_data.insert(
        font_name.clone(),
        Arc::new(egui::FontData::from_owned(font_bytes)),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, font_name.clone());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push(font_name);
    ctx.set_fonts(fonts);
}

fn load_system_cjk_font() -> Option<Vec<u8>> {
    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];

    candidates.iter().find_map(|path| std::fs::read(path).ok())
}

pub(super) fn panel(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(6.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), add_contents);
        });
}

pub(super) fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(14.0)
            .color(COLOR_TEXT)
            .strong(),
    );
}

pub(super) fn table_header(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(12.0)
            .color(COLOR_MUTED)
            .strong(),
    );
}

pub(super) fn centered_cell(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(
        egui::Layout::left_to_right(egui::Align::Center),
        add_contents,
    );
}

pub(super) fn cell_label(ui: &mut egui::Ui, text: impl Into<String>) {
    let text = text.into();
    cell_label_with_hover(ui, text.clone(), text);
}

pub(super) fn cell_label_with_hover(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    hover_text: impl Into<String>,
) {
    let text = text.into();
    let hover_text = hover_text.into();
    let response = ui.add(
        egui::Label::new(egui::RichText::new(text.clone()).size(12.0))
            .selectable(false)
            .sense(egui::Sense::hover()),
    );
    if response.hovered() {
        response.on_hover_text(hover_text);
    }
}

pub(super) fn connection_status_pill(ui: &mut egui::Ui, connected: bool) {
    let (text, color) = if connected {
        ("Online", COLOR_GOOD)
    } else {
        ("Reconnecting", COLOR_BAD)
    };
    status_badge(ui, text, color);
}

pub(super) fn timestamped_log(line: impl Into<String>) -> String {
    format!("[{}] {}", activity_time_label(), line.into())
}

pub(super) fn prune_activity_logs(log_lines: &mut Vec<String>) {
    if log_lines.len() > ACTIVITY_LOG_LIMIT {
        log_lines.drain(0..log_lines.len() - ACTIVITY_LOG_LIMIT);
    }
}

pub(super) fn activity_context_menu(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    log_lines: &mut Vec<String>,
) {
    ui.interact(rect, id.with("activity_context_menu"), egui::Sense::click())
        .context_menu(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(log_lines.join("\n"));
                ui.close();
            }
            if ui.button("Clear").clicked() {
                log_lines.clear();
                ui.close();
            }
        });
}

fn activity_time_label() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let china_time = now + 8 * 60 * 60;
    let seconds_today = china_time % (24 * 60 * 60);
    let hour = seconds_today / 3600;
    let minute = (seconds_today % 3600) / 60;
    let second = seconds_today % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

fn status_badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::default()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(999.0)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(color).strong());
        });
}

pub(super) fn compact_id(value: &str) -> String {
    let value = value.trim();
    let value = value.strip_prefix("client-").unwrap_or(value);
    compact_middle(value, 12, 6)
}

fn compact_middle(value: &str, head: usize, tail: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() > head + tail + 3 {
        let prefix = chars.iter().take(head).copied().collect::<String>();
        let suffix = chars
            .iter()
            .skip(chars.len().saturating_sub(tail))
            .copied()
            .collect::<String>();
        format!("{prefix}...{suffix}")
    } else {
        value.to_string()
    }
}

pub(super) fn empty_state(ui: &mut egui::Ui) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("No clients online")
                .size(16.0)
                .color(COLOR_TEXT),
        );
        ui.label(
            egui::RichText::new("Start a client or refresh after it connects.")
                .size(13.0)
                .color(COLOR_MUTED),
        );
    });
    ui.add_space(48.0);
}

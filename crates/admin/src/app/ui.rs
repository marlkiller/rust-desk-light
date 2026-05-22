use crate::i18n::t;
use eframe::egui;
use std::sync::Arc;

pub(super) use crate::theme::{
    ResolvedTheme, ThemeKind, COLOR_ACCENT, COLOR_BAD, COLOR_GOOD, COLOR_MUTED, COLOR_TEXT,
    COLOR_WARN,
};
pub(super) const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::CONTROL_HEIGHT;
const ACTIVITY_LOG_LIMIT: usize = 300;

pub(super) fn apply_admin_theme(ctx: &egui::Context, theme: ThemeKind) -> ResolvedTheme {
    ctx.set_theme(crate::theme::theme_preference(theme));
    apply_native_viewport_theme(ctx, theme);
    let resolved_theme = crate::theme::resolve_theme(ctx, theme);
    crate::theme::set_resolved_theme(resolved_theme);
    install_cjk_font(ctx);

    let palette = crate::theme::palette();
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.visuals = match resolved_theme {
        ResolvedTheme::Light => egui::Visuals::light(),
        ResolvedTheme::Dark => egui::Visuals::dark(),
    };
    style.visuals.window_fill = palette.panel;
    style.visuals.panel_fill = palette.bg;
    style.visuals.extreme_bg_color = palette.panel_subtle;
    style.visuals.widgets.noninteractive.fg_stroke.color = palette.text;
    style.visuals.widgets.inactive.fg_stroke.color = palette.text;
    style.visuals.widgets.hovered.fg_stroke.color = palette.text;
    style.visuals.widgets.active.fg_stroke.color = palette.text;
    style.visuals.widgets.inactive.bg_fill = palette.widget_idle;
    style.visuals.widgets.hovered.bg_fill = palette.widget_hovered;
    style.visuals.widgets.active.bg_fill = palette.widget_active;
    style.visuals.window_stroke.color = palette.border;
    style.visuals.selection.bg_fill = palette.selection_bg;
    style.visuals.selection.stroke.color = palette.accent;
    #[cfg(debug_assertions)]
    {
        style.debug.warn_if_rect_changes_id = false;
    }
    ctx.set_global_style(style);
    resolved_theme
}

fn apply_native_viewport_theme(ctx: &egui::Context, theme: ThemeKind) {
    let system_theme = match theme {
        ThemeKind::System => egui::SystemTheme::SystemDefault,
        ThemeKind::Light => egui::SystemTheme::Light,
        ThemeKind::Dark => egui::SystemTheme::Dark,
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(system_theme));
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
    crate::theme::panel_frame()
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), add_contents);
        });
}

pub(super) fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(14.0)
            .color(crate::theme::palette().text)
            .strong(),
    );
}

pub(super) fn table_header(ui: &mut egui::Ui, title: &str) {
    crate::theme::table_header_label(ui, title);
}

pub(super) fn about_row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    let value = value.into();
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.add_sized(
            [84.0, 18.0],
            egui::Label::new(crate::theme::muted_text(label)),
        );
        ui.add_sized(
            [ui.available_width(), 18.0],
            egui::Label::new(crate::theme::body_text(value.clone())).selectable(true),
        )
        .on_hover_text(value);
    });
}

pub(super) fn form_label(ui: &mut egui::Ui, label: &str) {
    ui.label(crate::theme::muted_text(label).strong());
}

pub(super) fn token_text_edit(
    ui: &mut egui::Ui,
    value: &mut String,
    visible: &mut bool,
    hint: &str,
) {
    let row_width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(row_width, TOOLBAR_CONTROL_HEIGHT),
        egui::Sense::hover(),
    );
    let gap = ui.spacing().item_spacing.x;
    let button_width = TOOLBAR_CONTROL_HEIGHT;
    let button_rect = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - button_width, rect.min.y),
        egui::vec2(button_width, TOOLBAR_CONTROL_HEIGHT),
    );
    let text_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2((button_rect.min.x - gap).max(rect.min.x), rect.max.y),
    );

    ui.put(
        text_rect,
        egui::TextEdit::singleline(value)
            .password(!*visible)
            .hint_text(hint)
            .vertical_align(egui::Align::Center),
    );

    let (icon, hover_text) = if *visible {
        ("🔒", t("Hide token"))
    } else {
        ("👁", t("Show token"))
    };
    let response = ui
        .put(button_rect, egui::Button::new(""))
        .on_hover_text(hover_text);
    let icon_color = ui.style().interact(&response).fg_stroke.color;
    ui.painter().text(
        button_rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(TOOLBAR_CONTROL_HEIGHT),
        icon_color,
    );
    if response.clicked() {
        *visible = !*visible;
    }
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
    let response = crate::theme::table_body_label(ui, text.clone());
    if response.hovered() {
        response.on_hover_text(hover_text);
    }
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
            if ui.button(t("Copy")).clicked() {
                ui.ctx().copy_text(log_lines.join("\n"));
                ui.close();
            }
            if ui.button(t("Clear")).clicked() {
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
            egui::RichText::new(t("No clients online"))
                .size(16.0)
                .color(crate::theme::palette().text),
        );
        ui.label(
            egui::RichText::new(t("Start a client or refresh after it connects."))
                .size(13.0)
                .color(crate::theme::palette().muted),
        );
    });
    ui.add_space(48.0);
}

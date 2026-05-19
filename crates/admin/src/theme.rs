use eframe::egui;
use std::sync::atomic::{AtomicU8, Ordering};

static CURRENT_THEME: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ThemeKind {
    System,
    Light,
    Dark,
}

impl ThemeKind {
    pub(crate) const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub(crate) fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" => Self::Dark,
            "light" => Self::Light,
            "auto" | "system" => Self::System,
            _ => Self::System,
        }
    }

    pub(crate) fn as_config(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedTheme {
    Light,
    Dark,
}

impl ResolvedTheme {
    fn from_egui(theme: egui::Theme) -> Self {
        match theme {
            egui::Theme::Light => Self::Light,
            egui::Theme::Dark => Self::Dark,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub bg: egui::Color32,
    pub panel: egui::Color32,
    pub panel_subtle: egui::Color32,
    pub border: egui::Color32,
    pub text: egui::Color32,
    pub muted: egui::Color32,
    pub accent: egui::Color32,
    pub on_accent: egui::Color32,
    pub good: egui::Color32,
    pub bad: egui::Color32,
    pub warn: egui::Color32,
    pub widget_idle: egui::Color32,
    pub widget_hovered: egui::Color32,
    pub widget_active: egui::Color32,
    pub selection_bg: egui::Color32,
    pub success_bg: egui::Color32,
    pub danger_bg: egui::Color32,
    pub neutral_bg: egui::Color32,
    pub meter_bg: egui::Color32,
    pub metric_cpu: egui::Color32,
    pub metric_memory: egui::Color32,
    pub metric_disk: egui::Color32,
}

#[derive(Clone, Copy)]
pub(crate) struct MapPalette {
    pub border_highlight: egui::Color32,
    pub stat_chip_bg: egui::Color32,
    pub stat_chip_border: egui::Color32,
    pub ocean: egui::Color32,
    pub ocean_bands: [egui::Color32; 4],
    pub equator: egui::Color32,
    pub graticule_label: egui::Color32,
    pub graticule_major: egui::Color32,
    pub graticule_minor: egui::Color32,
    pub land_shadow: egui::Color32,
    pub land: egui::Color32,
    pub coast_glow: egui::Color32,
    pub coast: egui::Color32,
    pub summary_bg: egui::Color32,
    pub summary_border: egui::Color32,
    pub cluster_shadow: egui::Color32,
    pub cluster_label_selected_bg: egui::Color32,
    pub cluster_label_bg: egui::Color32,
    pub hover_shadow: egui::Color32,
    pub hover_bg: egui::Color32,
    pub hover_border: egui::Color32,
}

pub(crate) const LIGHT_PALETTE: Palette = Palette {
    bg: egui::Color32::from_rgb(247, 249, 252),
    panel: egui::Color32::from_rgb(255, 255, 255),
    panel_subtle: egui::Color32::from_rgb(250, 252, 255),
    border: egui::Color32::from_rgb(228, 233, 241),
    text: egui::Color32::from_rgb(24, 33, 47),
    muted: egui::Color32::from_rgb(98, 111, 130),
    accent: egui::Color32::from_rgb(35, 99, 188),
    on_accent: egui::Color32::WHITE,
    good: egui::Color32::from_rgb(24, 135, 84),
    bad: egui::Color32::from_rgb(190, 58, 58),
    warn: egui::Color32::from_rgb(179, 116, 28),
    widget_idle: egui::Color32::from_rgb(243, 246, 250),
    widget_hovered: egui::Color32::from_rgb(235, 241, 248),
    widget_active: egui::Color32::from_rgb(226, 235, 247),
    selection_bg: egui::Color32::from_rgb(235, 244, 255),
    success_bg: egui::Color32::from_rgb(224, 246, 235),
    danger_bg: egui::Color32::from_rgb(255, 238, 238),
    neutral_bg: egui::Color32::from_rgb(243, 246, 250),
    meter_bg: egui::Color32::from_rgb(232, 237, 244),
    metric_cpu: egui::Color32::from_rgb(35, 99, 188),
    metric_memory: egui::Color32::from_rgb(24, 135, 84),
    metric_disk: egui::Color32::from_rgb(179, 116, 28),
};

pub(crate) const DARK_PALETTE: Palette = Palette {
    bg: egui::Color32::from_rgb(20, 24, 31),
    panel: egui::Color32::from_rgb(28, 34, 43),
    panel_subtle: egui::Color32::from_rgb(34, 41, 52),
    border: egui::Color32::from_rgb(55, 65, 81),
    text: egui::Color32::from_rgb(232, 238, 247),
    muted: egui::Color32::from_rgb(158, 171, 190),
    accent: egui::Color32::from_rgb(93, 156, 236),
    on_accent: egui::Color32::WHITE,
    good: egui::Color32::from_rgb(54, 183, 119),
    bad: egui::Color32::from_rgb(232, 100, 100),
    warn: egui::Color32::from_rgb(219, 155, 60),
    widget_idle: egui::Color32::from_rgb(36, 44, 56),
    widget_hovered: egui::Color32::from_rgb(48, 58, 73),
    widget_active: egui::Color32::from_rgb(59, 72, 92),
    selection_bg: egui::Color32::from_rgb(38, 73, 112),
    success_bg: egui::Color32::from_rgb(26, 67, 48),
    danger_bg: egui::Color32::from_rgb(74, 39, 43),
    neutral_bg: egui::Color32::from_rgb(36, 44, 56),
    meter_bg: egui::Color32::from_rgb(44, 53, 67),
    metric_cpu: egui::Color32::from_rgb(93, 156, 236),
    metric_memory: egui::Color32::from_rgb(54, 183, 119),
    metric_disk: egui::Color32::from_rgb(219, 155, 60),
};

pub(crate) fn resolve_theme(ctx: &egui::Context, theme: ThemeKind) -> ResolvedTheme {
    match theme {
        ThemeKind::Light => ResolvedTheme::Light,
        ThemeKind::Dark => ResolvedTheme::Dark,
        ThemeKind::System => ctx
            .system_theme()
            .map(ResolvedTheme::from_egui)
            .unwrap_or_else(|| ResolvedTheme::from_egui(ctx.theme())),
    }
}

pub(crate) fn theme_preference(theme: ThemeKind) -> egui::ThemePreference {
    match theme {
        ThemeKind::System => egui::ThemePreference::System,
        ThemeKind::Light => egui::ThemePreference::Light,
        ThemeKind::Dark => egui::ThemePreference::Dark,
    }
}

pub(crate) fn set_resolved_theme(theme: ResolvedTheme) {
    CURRENT_THEME.store(theme as u8, Ordering::Relaxed);
}

pub(crate) fn set_theme_kind(theme: ThemeKind) {
    set_resolved_theme(match theme {
        ThemeKind::Dark => ResolvedTheme::Dark,
        ThemeKind::System | ThemeKind::Light => ResolvedTheme::Light,
    });
}

pub(crate) fn current_resolved_theme() -> ResolvedTheme {
    match CURRENT_THEME.load(Ordering::Relaxed) {
        1 => ResolvedTheme::Dark,
        _ => ResolvedTheme::Light,
    }
}

pub(crate) fn palette() -> Palette {
    match current_resolved_theme() {
        ResolvedTheme::Light => LIGHT_PALETTE,
        ResolvedTheme::Dark => DARK_PALETTE,
    }
}

pub(crate) fn map_palette() -> MapPalette {
    match current_resolved_theme() {
        ResolvedTheme::Light => light_map_palette(),
        ResolvedTheme::Dark => dark_map_palette(),
    }
}

fn light_map_palette() -> MapPalette {
    MapPalette {
        border_highlight: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        stat_chip_bg: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180),
        stat_chip_border: egui::Color32::from_rgba_unmultiplied(208, 218, 229, 180),
        ocean: egui::Color32::from_rgb(226, 239, 249),
        ocean_bands: [
            egui::Color32::from_rgba_unmultiplied(214, 231, 245, 120),
            egui::Color32::from_rgba_unmultiplied(236, 246, 251, 120),
            egui::Color32::from_rgba_unmultiplied(219, 235, 247, 120),
            egui::Color32::from_rgba_unmultiplied(241, 248, 252, 120),
        ],
        equator: egui::Color32::from_rgba_unmultiplied(95, 132, 154, 80),
        graticule_label: egui::Color32::from_rgba_unmultiplied(74, 92, 110, 120),
        graticule_major: egui::Color32::from_rgba_unmultiplied(112, 145, 168, 70),
        graticule_minor: egui::Color32::from_rgba_unmultiplied(112, 145, 168, 38),
        land_shadow: egui::Color32::from_rgba_unmultiplied(69, 88, 80, 32),
        land: egui::Color32::from_rgb(221, 231, 214),
        coast_glow: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 95),
        coast: egui::Color32::from_rgba_unmultiplied(126, 151, 126, 170),
        summary_bg: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 218),
        summary_border: egui::Color32::from_rgba_unmultiplied(188, 202, 214, 165),
        cluster_shadow: egui::Color32::from_rgba_unmultiplied(25, 36, 48, 45),
        cluster_label_selected_bg: egui::Color32::from_rgba_unmultiplied(229, 239, 253, 235),
        cluster_label_bg: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220),
        hover_shadow: egui::Color32::from_rgba_unmultiplied(19, 30, 42, 45),
        hover_bg: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 242),
        hover_border: egui::Color32::from_rgba_unmultiplied(172, 190, 208, 210),
    }
}

fn dark_map_palette() -> MapPalette {
    MapPalette {
        border_highlight: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 35),
        stat_chip_bg: egui::Color32::from_rgba_unmultiplied(34, 41, 52, 220),
        stat_chip_border: egui::Color32::from_rgba_unmultiplied(83, 96, 118, 180),
        ocean: egui::Color32::from_rgb(20, 39, 55),
        ocean_bands: [
            egui::Color32::from_rgba_unmultiplied(26, 48, 67, 120),
            egui::Color32::from_rgba_unmultiplied(18, 34, 50, 120),
            egui::Color32::from_rgba_unmultiplied(24, 44, 62, 120),
            egui::Color32::from_rgba_unmultiplied(16, 30, 45, 120),
        ],
        equator: egui::Color32::from_rgba_unmultiplied(126, 176, 207, 90),
        graticule_label: egui::Color32::from_rgba_unmultiplied(154, 176, 198, 150),
        graticule_major: egui::Color32::from_rgba_unmultiplied(116, 160, 190, 85),
        graticule_minor: egui::Color32::from_rgba_unmultiplied(116, 160, 190, 45),
        land_shadow: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 50),
        land: egui::Color32::from_rgb(54, 76, 61),
        coast_glow: egui::Color32::from_rgba_unmultiplied(154, 210, 172, 45),
        coast: egui::Color32::from_rgba_unmultiplied(126, 173, 139, 170),
        summary_bg: egui::Color32::from_rgba_unmultiplied(28, 34, 43, 230),
        summary_border: egui::Color32::from_rgba_unmultiplied(83, 96, 118, 190),
        cluster_shadow: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 80),
        cluster_label_selected_bg: egui::Color32::from_rgba_unmultiplied(38, 73, 112, 235),
        cluster_label_bg: egui::Color32::from_rgba_unmultiplied(28, 34, 43, 230),
        hover_shadow: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 110),
        hover_bg: egui::Color32::from_rgba_unmultiplied(28, 34, 43, 245),
        hover_border: egui::Color32::from_rgba_unmultiplied(95, 110, 135, 220),
    }
}

pub(crate) const COLOR_ACCENT: egui::Color32 = LIGHT_PALETTE.accent;
pub(crate) const COLOR_ON_ACCENT: egui::Color32 = LIGHT_PALETTE.on_accent;
pub(crate) const COLOR_GOOD: egui::Color32 = LIGHT_PALETTE.good;
pub(crate) const COLOR_BAD: egui::Color32 = LIGHT_PALETTE.bad;
pub(crate) const COLOR_WARN: egui::Color32 = LIGHT_PALETTE.warn;
pub(crate) const COLOR_BG: egui::Color32 = LIGHT_PALETTE.bg;
pub(crate) const COLOR_PANEL: egui::Color32 = LIGHT_PALETTE.panel;
pub(crate) const COLOR_BORDER: egui::Color32 = LIGHT_PALETTE.border;
pub(crate) const COLOR_TEXT: egui::Color32 = LIGHT_PALETTE.text;
pub(crate) const COLOR_MUTED: egui::Color32 = LIGHT_PALETTE.muted;
pub(crate) const COLOR_METER_BG: egui::Color32 = LIGHT_PALETTE.meter_bg;
pub(crate) const COLOR_METRIC_CPU: egui::Color32 = LIGHT_PALETTE.metric_cpu;
pub(crate) const COLOR_METRIC_MEMORY: egui::Color32 = LIGHT_PALETTE.metric_memory;
pub(crate) const COLOR_METRIC_DISK: egui::Color32 = LIGHT_PALETTE.metric_disk;

pub(crate) fn with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub(crate) fn map_label_color(alpha: u8) -> egui::Color32 {
    match current_resolved_theme() {
        ResolvedTheme::Light => egui::Color32::from_rgba_unmultiplied(76, 91, 77, alpha),
        ResolvedTheme::Dark => egui::Color32::from_rgba_unmultiplied(174, 201, 177, alpha),
    }
}

pub(crate) const CONTROL_HEIGHT: f32 = 28.0;
pub(crate) const COMPACT_CONTROL_HEIGHT: f32 = 24.0;
pub(crate) const PANEL_MARGIN: f32 = 8.0;
pub(crate) const SECTION_GAP: f32 = 6.0;
pub(crate) const TABLE_HEADER_HEIGHT: f32 = COMPACT_CONTROL_HEIGHT;
pub(crate) const TABLE_ROW_HEIGHT: f32 = COMPACT_CONTROL_HEIGHT;

pub(crate) fn panel_frame() -> egui::Frame {
    let palette = palette();
    egui::Frame::default()
        .fill(palette.panel)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(6.0)
}

pub(crate) fn panel_frame_with_margin(margin: f32) -> egui::Frame {
    panel_frame().inner_margin(margin)
}

pub(crate) fn page_frame() -> egui::Frame {
    egui::Frame::default().fill(palette().bg).inner_margin(12.0)
}

pub(crate) fn status_frame() -> egui::Frame {
    panel_frame().inner_margin(egui::Margin::symmetric(12, 8))
}

pub(crate) fn footer_frame() -> egui::Frame {
    let palette = palette();
    egui::Frame::default()
        .fill(palette.bg)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .inner_margin(egui::Margin::symmetric(8, 6))
}

pub(crate) fn clickable_table<'a>(
    ui: &'a mut egui::Ui,
    id_salt: impl std::hash::Hash,
    striped: bool,
) -> egui_extras::TableBuilder<'a> {
    egui_extras::TableBuilder::new(ui)
        .id_salt(id_salt)
        .striped(striped)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
}

pub(crate) fn muted_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(12.0).color(palette().muted)
}

pub(crate) fn body_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(12.0).color(palette().text)
}

pub(crate) fn strong_body_text(text: impl Into<String>) -> egui::RichText {
    body_text(text).strong()
}

pub(crate) fn danger_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(12.0).color(palette().bad)
}

pub(crate) fn render_status_line(
    ui: &mut egui::Ui,
    label: &str,
    color: egui::Color32,
    notice: &str,
    add_extra: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        ui.label(
            egui::RichText::new(label)
                .size(12.0)
                .color(palette().text)
                .strong(),
        );
        ui.label(muted_text(notice));
        add_extra(ui);
    });
}

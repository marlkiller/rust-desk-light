use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::theme::COLOR_TEXT;
pub(super) use crate::theme::{COLOR_BORDER, COLOR_MUTED, COLOR_PANEL};

pub(super) const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::CONTROL_HEIGHT;
const INLINE_LABEL_WIDTH: f32 = 86.0;
pub(super) const CODE_ROW_HEIGHT: f32 = 18.0;
const STATUS_BAR_HEIGHT: f32 = 42.0;

pub(super) fn render_status_panel(ui: &mut egui::Ui, result_status: &Arc<Mutex<String>>) {
    egui::Panel::bottom(egui::Id::new((
        "execute_status_panel",
        Arc::as_ptr(result_status),
    )))
    .exact_size(STATUS_BAR_HEIGHT)
    .show_separator_line(false)
    .frame(crate::theme::footer_frame())
    .show_inside(ui, |ui| render_status_bar(ui, result_status));
}

fn render_status_bar(ui: &mut egui::Ui, result_status: &Arc<Mutex<String>>) {
    let status = result_status
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let status = status_bar_text(&status);

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), TOOLBAR_CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            render_inline_label(ui, "Status");
            ui.label(egui::RichText::new(status).size(12.0).color(COLOR_TEXT));
        },
    );
}

fn status_bar_text(status: &str) -> String {
    if status.trim().is_empty() {
        "Ready".to_string()
    } else {
        status.to_string()
    }
}

pub(super) fn render_inline_label(ui: &mut egui::Ui, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(INLINE_LABEL_WIDTH, TOOLBAR_CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(label).size(12.0).color(COLOR_MUTED));
        },
    );
}

pub(super) fn render_inline_text_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &Arc<Mutex<String>>,
    hint: &str,
) {
    let mut text = value.lock().map(|value| value.clone()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        render_inline_label(ui, label);
        let response = ui.add_sized(
            [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut text)
                .hint_text(hint)
                .vertical_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(mut value) = value.lock() {
                *value = text;
            }
        }
    });
}

pub(super) fn render_text_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &Arc<Mutex<String>>,
    hint: &str,
) {
    let mut text = value.lock().map(|value| value.clone()).unwrap_or_default();
    ui.label(egui::RichText::new(label).size(12.0).color(COLOR_MUTED));
    let response = ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .vertical_align(egui::Align::Center),
    );
    if response.changed() {
        if let Ok(mut value) = value.lock() {
            *value = text;
        }
    }
}

pub(super) fn render_run_button(
    ui: &mut egui::Ui,
    can_run: bool,
    disabled_message: &str,
    send_requested: &Arc<AtomicBool>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(can_run, egui::Button::new("Run")).clicked() {
                send_requested.store(true, Ordering::Relaxed);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            if !can_run && !disabled_message.is_empty() {
                ui.label(
                    egui::RichText::new(disabled_message)
                        .size(12.0)
                        .color(COLOR_TEXT),
                );
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::status_bar_text;

    #[test]
    fn status_bar_defaults_to_ready() {
        assert_eq!(status_bar_text(""), "Ready");
        assert_eq!(status_bar_text("Running..."), "Running...");
    }
}

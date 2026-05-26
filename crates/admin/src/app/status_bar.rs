use super::*;

fn info_icon_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(STATUS_BAR_CONTENT_HEIGHT, STATUS_BAR_CONTENT_HEIGHT),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let icon_color = if response.hovered() || selected {
            crate::theme::palette().text
        } else {
            crate::theme::palette().muted
        };
        let palette = crate::theme::palette();
        let bg_color = if selected {
            palette.widget_active
        } else if response.hovered() {
            palette.widget_idle
        } else {
            egui::Color32::TRANSPARENT
        };
        if bg_color != egui::Color32::TRANSPARENT {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(6), bg_color);
        }

        let center = rect.center();
        ui.painter()
            .circle_stroke(center, 8.0, egui::Stroke::new(1.5, icon_color));
        ui.painter()
            .circle_filled(egui::pos2(center.x, center.y - 4.2), 1.25, icon_color);
        ui.painter().line_segment(
            [
                egui::pos2(center.x, center.y - 0.8),
                egui::pos2(center.x, center.y + 5.0),
            ],
            egui::Stroke::new(1.6, icon_color),
        );
    }
    response.on_hover_text(t("About"))
}

impl AdminApp {
    pub(super) fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        let (status_text, notice, color) = if self.connected {
            (t("Online"), t("Connected to service"), color_good())
        } else {
            (
                t("Reconnecting"),
                t("Waiting for service connection"),
                color_bad(),
            )
        };
        crate::theme::status_frame().show(ui, |ui| {
            ui.set_min_height(STATUS_BAR_CONTENT_HEIGHT);
            crate::theme::render_status_line(ui, status_text, color, notice, |ui| {
                ui.separator();
                ui.label(crate::theme::muted_text(format!(
                    "{} {}:{}",
                    t("Service"),
                    self.config.ip,
                    self.config.port
                )));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if info_icon_button(ui, self.about_open).clicked() {
                        self.about_open = true;
                    }
                });
            });
        });
    }
}

use super::*;

impl AdminApp {
    pub(super) fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        let (status_text, notice, color) = if self.connected {
            (t("Online"), t("Connected to service"), COLOR_GOOD)
        } else {
            (
                t("Reconnecting"),
                t("Waiting for service connection"),
                COLOR_BAD,
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

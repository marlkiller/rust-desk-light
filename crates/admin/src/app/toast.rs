use super::*;

impl AdminApp {
    pub(super) fn render_client_online_toasts(&mut self, ctx: &egui::Context) {
        self.client_online_toasts
            .retain(|toast| toast.created_at.elapsed() < CLIENT_ONLINE_TOAST_TTL);
        if self.client_online_toasts.is_empty() {
            return;
        }

        let mut dismiss_index = None;
        egui::Area::new(egui::Id::new("admin_client_online_toasts"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-18.0, 18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                for index in (0..self.client_online_toasts.len()).rev() {
                    let toast = &self.client_online_toasts[index];
                    let title = toast.title.clone();
                    let detail = toast.detail.clone();
                    crate::theme::panel_frame()
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(8.0, 8.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), 4.0, color_good());
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(title)
                                            .size(13.0)
                                            .color(crate::theme::palette().text)
                                            .strong(),
                                    );
                                    ui.label(crate::theme::muted_text(detail));
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Min),
                                    |ui| {
                                        if ui.small_button("x").on_hover_text("Dismiss").clicked() {
                                            dismiss_index = Some(index);
                                        }
                                    },
                                );
                            });
                        });
                }
            });

        if let Some(index) = dismiss_index {
            self.client_online_toasts.remove(index);
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

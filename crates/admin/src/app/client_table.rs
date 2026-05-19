use super::*;

impl AdminApp {
    pub(super) fn render_overview(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, t("Overview"));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                overview_metric(ui, t("Online"), self.online_client_count().to_string());
                overview_metric(ui, t("Known"), self.clients.len().to_string());
                overview_metric(ui, t("Selected"), self.selected_client_label());
                overview_metric(ui, t("Version"), rdl_version::display_version());
            });
        });
    }

    pub(super) fn render_clients(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            ui.horizontal(|ui| {
                section_title(ui, t("Clients"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(crate::theme::muted_text(t(
                        "Right click a row for commands",
                    )));
                });
            });
            ui.add_space(6.0);
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
                ui.add_sized(
                    [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
                    egui::TextEdit::singleline(&mut self.client_filter)
                        .hint_text(t("Search by id, fingerprint, host, user, OS, or location"))
                        .vertical_align(egui::Align::Center),
                );
            });
            ui.add_space(8.0);

            let clients = self.filtered_clients();
            if clients.is_empty() {
                empty_state(ui);
                return;
            }

            let ctx = ui.ctx().clone();
            crate::theme::clickable_table(ui, "admin_clients_table_resizable", false)
                .column(egui_extras::Column::initial(86.0).at_least(72.0).clip(true))
                .column(
                    egui_extras::Column::initial(190.0)
                        .at_least(140.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(150.0)
                        .at_least(120.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(180.0)
                        .at_least(120.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(150.0)
                        .at_least(120.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(100.0)
                        .at_least(80.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(220.0)
                        .at_least(130.0)
                        .clip(true),
                )
                .header(24.0, |mut header| {
                    header.col(|ui| table_header(ui, t("Status")));
                    header.col(|ui| table_header(ui, t("Client ID")));
                    header.col(|ui| table_header(ui, t("IP")));
                    header.col(|ui| table_header(ui, t("Location")));
                    header.col(|ui| table_header(ui, t("Host")));
                    header.col(|ui| table_header(ui, t("User")));
                    header.col(|ui| table_header(ui, t("OS Version")));
                })
                .body(|body| {
                    body.rows(30.0, clients.len(), |mut row| {
                        let row_data = &clients[row.index()];
                        let client = &row_data.info;
                        let selected =
                            self.selected_client_id.as_deref() == Some(client.id.as_str());
                        row.set_selected(selected);
                        row.col(|ui| {
                            centered_cell(ui, |ui| client_status_text(ui, row_data.status))
                        });
                        row.col(|ui| centered_cell(ui, |ui| cell_label(ui, &client.id)));
                        row.col(|ui| centered_cell(ui, |ui| cell_label(ui, &client.peer_addr)));
                        row.col(|ui| {
                            centered_cell(ui, |ui| cell_label(ui, client_location_label(client)))
                        });
                        row.col(|ui| centered_cell(ui, |ui| cell_label(ui, &client.hostname)));
                        row.col(|ui| centered_cell(ui, |ui| cell_label(ui, &client.username)));
                        row.col(|ui| {
                            centered_cell(ui, |ui| cell_label(ui, client_os_label(&client.os)))
                        });
                        let response = row.response();
                        if response.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            self.selected_client_id = Some(client.id.clone());
                        }
                        response.context_menu(|ui| {
                            if row_data.status.can_receive_commands() {
                                command_menu::render_context_menu(
                                    ui,
                                    &client.id,
                                    client.gui_available,
                                    &mut |client_id, command| {
                                        self.send_command(client_id, command);
                                    },
                                );
                            } else {
                                command_menu::render_unavailable_client_menu(
                                    ui,
                                    &client.id,
                                    client_status_display(row_data.status).0,
                                );
                            }
                        });
                    });
                });
        });
    }
}

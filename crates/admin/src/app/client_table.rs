use super::*;

const GROUP_TAG_MAX_WIDTH: f32 = 74.0;
const GROUP_TAG_HEIGHT: f32 = 18.0;
const GROUP_TAG_TEXT_SIZE: f32 = 11.0;

fn overview_metric(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    let value = value.into();
    let palette = crate::theme::palette();
    egui::Frame::default()
        .fill(palette.bg)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(match label {
                value if value == t("Selected") => 170.0,
                value if value == t("Version") => 112.0,
                _ => 82.0,
            });
            ui.horizontal(|ui| {
                ui.label(crate::theme::muted_text(label));
                ui.add(
                    egui::Label::new(crate::theme::strong_body_text(value.clone()).size(13.0))
                        .selectable(false),
                )
                .on_hover_text(value);
            });
        });
}

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
                    if ui
                        .add_enabled(self.connected, egui::Button::new(t("Refresh")))
                        .on_hover_text(t("Refresh table content"))
                        .clicked()
                    {
                        let _ = self.input_tx.send(AdminInput::ListClients);
                        self.push_log("refresh clients requested");
                    }
                    if ui
                        .add_enabled(
                            !self.clients.is_empty(),
                            egui::Button::new("📤").min_size(egui::vec2(
                                TOOLBAR_CONTROL_HEIGHT,
                                TOOLBAR_CONTROL_HEIGHT,
                            )),
                        )
                        .on_hover_text(t("Export client list"))
                        .clicked()
                    {
                        self.export_client_list();
                    }
                    ui.add_space(8.0);
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
                        .hint_text(t(
                            "Search by alias, id, fingerprint, group, host, user, OS, or location",
                        ))
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
            crate::theme::clickable_table(ui, "admin_clients_table_group_tag", false)
                .column(
                    egui_extras::Column::initial(156.0)
                        .at_least(118.0)
                        .clip(true),
                )
                .column(
                    egui_extras::Column::initial(170.0)
                        .at_least(120.0)
                        .clip(true),
                )
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
                    header.col(|ui| table_header(ui, t("Name")));
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
                            centered_cell(ui, |ui| {
                                grouped_status_cell(
                                    ui,
                                    self.client_group(&client.id),
                                    row_data.status,
                                )
                            })
                        });
                        row.col(|ui| {
                            centered_cell(ui, |ui| {
                                cell_label(ui, self.client_display_label(row_data))
                            })
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
                            let mut queued_command = None::<(String, CommandKind)>;
                            let mut edit_alias = false;
                            if row_data.status.can_receive_commands() {
                                command_menu::render_context_menu(
                                    ui,
                                    &client.id,
                                    client.gui_available,
                                    &mut |client_id, command| {
                                        queued_command = Some((client_id.to_string(), command));
                                    },
                                    &mut |_| {
                                        edit_alias = true;
                                    },
                                );
                            } else {
                                command_menu::render_unavailable_client_menu(
                                    ui,
                                    &client.id,
                                    client_status_display(row_data.status).0,
                                    &mut |client_id, command| {
                                        queued_command = Some((client_id.to_string(), command));
                                    },
                                    &mut |_| {
                                        edit_alias = true;
                                    },
                                );
                            }
                            if edit_alias {
                                self.open_alias_window(&client.id);
                            }
                            if let Some((client_id, command)) = queued_command {
                                self.send_command(&client_id, command);
                            }
                        });
                    });
                });
        });
    }

    fn export_client_list(&mut self) {
        let clients = self.filtered_clients();
        if clients.is_empty() {
            return;
        }

        let Some(path) = rfd::FileDialog::new()
            .set_title(t("Export client list"))
            .add_filter("CSV", &["csv"])
            .set_file_name(format!(
                "rust-desk-light-clients-{}.csv",
                rdl_protocol::now_epoch_ms()
            ))
            .save_file()
        else {
            return;
        };
        let path = ensure_csv_extension(path);
        let content = self.client_list_csv(&clients);
        match std::fs::write(&path, content) {
            Ok(()) => self.push_log(format!(
                "{} {}",
                t("Client list exported to"),
                path.display()
            )),
            Err(error) => self.push_log(format!("{}: {error}", t("Export client list failed"))),
        }
    }

    fn client_list_csv(&self, clients: &[ClientRow]) -> String {
        let headers = [
            t("Status"),
            t("Name"),
            t("Alias"),
            t("Group"),
            t("Client ID"),
            t("IP"),
            t("Location"),
            t("Host"),
            t("User"),
            t("OS Version"),
            t("Fingerprint"),
        ];
        let mut lines = vec![headers.map(csv_field).join(",")];
        for row in clients {
            let client = &row.info;
            let fields = [
                client_status_display(row.status).0.to_string(),
                self.client_display_label(row),
                self.client_display_label(row),
                self.client_group(&client.id).to_string(),
                client.id.clone(),
                client.peer_addr.clone(),
                client_location_label(client),
                client.hostname.clone(),
                client.username.clone(),
                client.os.clone(),
                client.fingerprint.clone(),
            ];
            lines.push(fields.map(csv_field).join(","));
        }
        lines.push(String::new());
        lines.join("\n")
    }
}

fn ensure_csv_extension(mut path: std::path::PathBuf) -> std::path::PathBuf {
    if path.extension().is_none() {
        path.set_extension("csv");
    }
    path
}

fn csv_field(value: impl AsRef<str>) -> String {
    let value = value.as_ref().replace(['\r', '\n'], " ");
    if value.contains(',') || value.contains('"') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn grouped_status_cell(ui: &mut egui::Ui, group: &str, status: ClientStatus) {
    let item_spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 6.0;
    client_status_text(ui, status);
    let group = group.trim();
    if !group.is_empty() {
        group_tag(ui, group);
    }
    ui.spacing_mut().item_spacing.x = item_spacing;
}

fn group_tag(ui: &mut egui::Ui, group: &str) {
    let palette = crate::theme::palette();
    let available = ui.available_width().max(0.0);
    let width = ((group.chars().count() as f32 * 7.0) + 18.0)
        .clamp(34.0, GROUP_TAG_MAX_WIDTH)
        .min(available);
    if width < 24.0 {
        return;
    }

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, GROUP_TAG_HEIGHT), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, palette.selection_bg);
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, palette.accent),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        compact_group_tag(group, width),
        egui::FontId::proportional(GROUP_TAG_TEXT_SIZE),
        palette.accent,
    );
    if response.hovered() {
        response.on_hover_text(group);
    }
}

fn compact_group_tag(group: &str, width: f32) -> String {
    let max_chars = ((width - 14.0) / 7.0).floor().max(1.0) as usize;
    if group.chars().count() <= max_chars {
        return group.to_string();
    }
    if max_chars <= 2 {
        return group.chars().take(max_chars).collect();
    }
    format!(
        "{}..",
        group
            .chars()
            .take(max_chars.saturating_sub(2))
            .collect::<String>()
    )
}

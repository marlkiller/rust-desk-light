use crate::i18n::t;
use eframe::egui;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct MoveGroupWindow {
    open: bool,
    client_id: String,
    client_label: String,
    group: String,
    error: String,
}

pub(super) enum MoveGroupAction {
    Save { client_id: String, group: String },
    Clear { client_id: String },
}

impl MoveGroupWindow {
    pub(super) fn open(&mut self, client_id: &str, client_label: String, current_group: &str) {
        self.open = true;
        self.client_id = client_id.to_string();
        self.client_label = client_label;
        self.group = current_group.to_string();
        self.error.clear();
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.error.clear();
    }

    pub(super) fn set_error(&mut self, error: impl Into<String>) {
        self.error = error.into();
        self.open = true;
    }
}

pub(super) fn render_move_group_window(
    ctx: &egui::Context,
    state: &mut MoveGroupWindow,
    groups: &HashMap<String, String>,
) -> Option<MoveGroupAction> {
    if !state.open {
        return None;
    }

    let mut action = None;
    let mut open = state.open;
    let mut close_requested = false;
    let existing_groups = existing_group_names(groups);
    egui::Window::new(t("Move To Group"))
        .id(egui::Id::new("admin_move_group_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.label(crate::theme::muted_text(t("Client")).strong());
            ui.label(crate::theme::body_text(&state.client_label));
            ui.add_space(crate::theme::SECTION_GAP);

            ui.label(crate::theme::muted_text(t("Group Name")).strong());
            render_group_picker(ui, state, &existing_groups);
            ui.add_sized(
                [ui.available_width(), crate::theme::CONTROL_HEIGHT],
                egui::TextEdit::singleline(&mut state.group)
                    .hint_text(t("Group"))
                    .vertical_align(egui::Align::Center),
            );

            if !state.error.is_empty() {
                ui.add_space(crate::theme::SECTION_GAP);
                ui.label(
                    egui::RichText::new(&state.error)
                        .size(12.0)
                        .color(crate::theme::color_bad()),
                );
            }

            ui.add_space(crate::theme::PANEL_MARGIN);
            ui.horizontal(|ui| {
                ui.spacing_mut().interact_size.y = crate::theme::CONTROL_HEIGHT;
                if ui.button(t("Clear Group")).clicked() {
                    action = Some(MoveGroupAction::Clear {
                        client_id: state.client_id.clone(),
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("Cancel")).clicked() {
                        close_requested = true;
                    }
                    if ui.button(t("Save Group")).clicked() {
                        action = Some(MoveGroupAction::Save {
                            client_id: state.client_id.clone(),
                            group: clean_group_name(&state.group),
                        });
                    }
                });
            });
        });
    state.open = open && !close_requested;
    if close_requested {
        state.error.clear();
    }

    action
}

fn render_group_picker(ui: &mut egui::Ui, state: &mut MoveGroupWindow, existing_groups: &[String]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = crate::theme::CONTROL_HEIGHT;
        egui::ComboBox::from_id_salt("admin_move_group_picker")
            .width(ui.available_width())
            .selected_text(group_picker_label(&state.group))
            .show_ui(ui, |ui| {
                if existing_groups.is_empty() {
                    ui.add_enabled(false, egui::Label::new(t("No existing groups")));
                    return;
                }
                for group in existing_groups {
                    if ui.selectable_label(state.group == *group, group).clicked() {
                        state.group = group.clone();
                        ui.close();
                    }
                }
            });
    });
    ui.add_space(crate::theme::SECTION_GAP);
}

fn group_picker_label(group: &str) -> String {
    let group = group.trim();
    if group.is_empty() {
        t("Select existing group").to_string()
    } else {
        group.to_string()
    }
}

fn existing_group_names(groups: &HashMap<String, String>) -> Vec<String> {
    groups
        .values()
        .map(|group| clean_group_name(group))
        .filter(|group| !group.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn load_client_groups() -> HashMap<String, String> {
    let Ok(text) = fs::read_to_string(groups_path()) else {
        return HashMap::new();
    };

    text.lines()
        .filter_map(|line| {
            let (client_id, group) = line.split_once('\t')?;
            let client_id = client_id.trim();
            let group = group.trim();
            (!client_id.is_empty() && !group.is_empty())
                .then(|| (client_id.to_string(), group.to_string()))
        })
        .collect()
}

pub(super) fn save_client_groups(groups: &HashMap<String, String>) -> io::Result<()> {
    let path = groups_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rows = groups
        .iter()
        .filter_map(|(client_id, group)| {
            let client_id = clean_field(client_id);
            let group = clean_group_name(group);
            (!client_id.is_empty() && !group.is_empty()).then(|| format!("{client_id}\t{group}"))
        })
        .collect::<Vec<_>>();
    rows.sort();
    fs::write(path, rows.join("\n"))
}

pub(super) fn clean_group_name(value: &str) -> String {
    clean_field(value)
}

fn clean_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

fn groups_path() -> PathBuf {
    rdl_config::default_config_dir().join("admin.client-groups.tsv")
}

use crate::i18n::t;
use eframe::egui;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct AliasWindow {
    open: bool,
    client_id: String,
    client_label: String,
    default_alias: String,
    alias: String,
    error: String,
}

pub(super) enum AliasAction {
    Save { client_id: String, alias: String },
    RestoreDefault { client_id: String },
}

impl AliasWindow {
    pub(super) fn open(
        &mut self,
        client_id: &str,
        client_label: String,
        default_alias: String,
        current_alias: &str,
    ) {
        self.open = true;
        self.client_id = client_id.to_string();
        self.client_label = client_label;
        self.default_alias = default_alias;
        self.alias = current_alias.to_string();
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

pub(super) fn render_alias_window(
    ctx: &egui::Context,
    state: &mut AliasWindow,
) -> Option<AliasAction> {
    if !state.open {
        return None;
    }

    let mut action = None;
    let mut open = state.open;
    let mut close_requested = false;
    egui::Window::new(t("Edit Alias"))
        .id(egui::Id::new("admin_client_alias_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.label(crate::theme::muted_text(t("Client")).strong());
            ui.label(crate::theme::body_text(&state.client_label));
            ui.add_space(crate::theme::SECTION_GAP);

            ui.label(crate::theme::muted_text(t("Alias")).strong());
            ui.add_sized(
                [ui.available_width(), crate::theme::CONTROL_HEIGHT],
                egui::TextEdit::singleline(&mut state.alias)
                    .hint_text(t("Alias"))
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
                if ui.button(t("Restore Default")).clicked() {
                    state.alias = state.default_alias.clone();
                    action = Some(AliasAction::RestoreDefault {
                        client_id: state.client_id.clone(),
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t("Cancel")).clicked() {
                        close_requested = true;
                    }
                    if ui.button(t("Save Alias")).clicked() {
                        let alias = clean_alias(&state.alias);
                        if alias.is_empty() {
                            state.error = t("Alias cannot be empty").to_string();
                        } else {
                            action = Some(AliasAction::Save {
                                client_id: state.client_id.clone(),
                                alias,
                            });
                        }
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

pub(super) fn load_client_aliases() -> HashMap<String, String> {
    let Ok(text) = fs::read_to_string(aliases_path()) else {
        return HashMap::new();
    };

    text.lines()
        .filter_map(|line| {
            let (client_id, alias) = line.split_once('\t')?;
            let client_id = client_id.trim();
            let alias = alias.trim();
            (!client_id.is_empty() && !alias.is_empty())
                .then(|| (client_id.to_string(), alias.to_string()))
        })
        .collect()
}

pub(super) fn save_client_aliases(aliases: &HashMap<String, String>) -> io::Result<()> {
    let path = aliases_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut rows = aliases
        .iter()
        .filter_map(|(client_id, alias)| {
            let client_id = clean_field(client_id);
            let alias = clean_alias(alias);
            (!client_id.is_empty() && !alias.is_empty()).then(|| format!("{client_id}\t{alias}"))
        })
        .collect::<Vec<_>>();
    rows.sort();
    fs::write(path, rows.join("\n"))
}

pub(super) fn clean_alias(value: &str) -> String {
    clean_field(value)
}

fn clean_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

fn aliases_path() -> PathBuf {
    rdl_config::default_config_dir().join("admin.client-aliases.tsv")
}

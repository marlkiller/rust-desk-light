use super::ui;
use crate::i18n::t;
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use rdl_protocol::{
    default_static_command_preset_id, static_command_preset_label, static_command_presets,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub(super) fn render(
    ui: &mut egui::Ui,
    static_preset: &Arc<Mutex<String>>,
    static_custom_mode: &Arc<AtomicBool>,
    static_custom_command: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    let mut custom_mode = static_custom_mode.load(Ordering::Relaxed);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = ui::TOOLBAR_CONTROL_HEIGHT;
        ui::render_inline_label(ui, t("Mode"));
        if ui.selectable_label(!custom_mode, t("Preset")).clicked() {
            custom_mode = false;
            static_custom_mode.store(false, Ordering::Relaxed);
        }
        if ui.selectable_label(custom_mode, t("Custom")).clicked() {
            custom_mode = true;
            static_custom_mode.store(true, Ordering::Relaxed);
        }
    });
    ui.add_space(8.0);

    let presets = static_command_presets();
    let mut selected = static_preset
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| default_static_command_preset_id().to_string());
    if custom_mode {
        ui::render_inline_text_field(ui, t("Command"), static_custom_command, "whoami");
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = ui::TOOLBAR_CONTROL_HEIGHT;
            ui::render_inline_label(ui, t("Preset"));
            egui::ComboBox::from_id_salt("execute_static_command")
                .width(180.0)
                .selected_text(static_command_preset_label(&selected))
                .show_ui(ui, |ui| {
                    for preset in presets {
                        if ui
                            .selectable_label(selected == preset.id, preset.label)
                            .clicked()
                        {
                            selected = preset.id.to_string();
                            if let Ok(mut value) = static_preset.lock() {
                                *value = selected.clone();
                            }
                        }
                    }
                });
        });
    }
    ui.add_space(12.0);
    let can_run = !custom_mode
        || static_custom_command
            .lock()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
    ui::render_run_button(ui, can_run, t("Command is required"), send_requested);
}

pub(super) fn payload_for(preset: &str, custom_mode: bool, custom_command: &str) -> String {
    if custom_mode {
        return format!(
            "action=run\nmode=custom\ncommand_b64={}",
            STANDARD.encode(custom_command)
        );
    }
    format!(
        "action=run\nmode=preset\npreset={}",
        sanitize_single_line(preset)
    )
}

fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

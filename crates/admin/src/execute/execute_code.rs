use super::ui;
use crate::i18n::t;
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodeLanguage {
    pub(super) id: String,
    pub(super) command: String,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    ui: &mut egui::Ui,
    code_language: &Arc<Mutex<String>>,
    code_text: &Arc<Mutex<String>>,
    code_languages: &Arc<Mutex<Vec<CodeLanguage>>>,
    language_status: &Arc<Mutex<String>>,
    language_probe_requested: &Arc<AtomicBool>,
    has_result: bool,
    send_requested: &Arc<AtomicBool>,
) {
    let languages = code_languages
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let mut selected = code_language
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = ui::TOOLBAR_CONTROL_HEIGHT;
        ui::render_inline_label(ui, t("Language"));
        egui::ComboBox::from_id_salt("execute_code_language")
            .width(140.0)
            .selected_text(if selected.is_empty() {
                t("Loading...")
            } else {
                selected.as_str()
            })
            .show_ui(ui, |ui| {
                for language in &languages {
                    if ui
                        .selectable_label(selected == language.id, &language.id)
                        .clicked()
                    {
                        selected = language.id.clone();
                        if let Ok(mut value) = code_language.lock() {
                            *value = selected.clone();
                        }
                        set_code_template(code_text, &selected);
                    }
                }
            });
        if ui.button(t("Refresh")).clicked() {
            language_probe_requested.store(true, Ordering::Relaxed);
        }
        let status = language_status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        if !status.trim().is_empty() {
            ui.label(
                egui::RichText::new(status)
                    .size(12.0)
                    .color(crate::theme::palette().muted),
            );
        }
    });
    ui.add_space(8.0);
    let mut code = code_text
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    ui.label(
        egui::RichText::new(t("Code"))
            .size(12.0)
            .color(crate::theme::palette().muted),
    );
    let editor_height = if has_result {
        (ui.available_height() * 0.46).clamp(160.0, 240.0)
    } else {
        (ui.available_height() - ui::TOOLBAR_CONTROL_HEIGHT - 28.0).clamp(180.0, 280.0)
    };
    let desired_rows = code.lines().count().clamp(12, 240);
    let editor_content_height =
        (desired_rows as f32 * ui::CODE_ROW_HEIGHT + 18.0).max(editor_height);
    let editor_scroll_id = ("execute_code_editor_scroll", Arc::as_ptr(code_text));
    let changed = egui::ScrollArea::vertical()
        .id_salt(editor_scroll_id)
        .auto_shrink([false, false])
        .max_height(editor_height)
        .show(ui, |ui| {
            ui.add_sized(
                [ui.available_width(), editor_content_height],
                egui::TextEdit::multiline(&mut code)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(desired_rows),
            )
            .changed()
        })
        .inner;
    if changed {
        if let Ok(mut value) = code_text.lock() {
            *value = code.clone();
        }
    }
    ui.add_space(10.0);
    let can_run = !selected.trim().is_empty() && !code.trim().is_empty();
    ui::render_run_button(
        ui,
        can_run,
        t("Language and code are required"),
        send_requested,
    );
}

pub(super) fn payload_for(language: &str, code: &str) -> String {
    [
        "action=run".to_string(),
        format!("language={}", sanitize_single_line(language)),
        format!("code_b64={}", STANDARD.encode(code)),
    ]
    .join("\n")
}

pub(super) fn parse_language_response(detail: &str) -> Vec<CodeLanguage> {
    detail
        .lines()
        .skip_while(|line| line.trim().is_empty() || line.trim_end().ends_with(':'))
        .skip(1)
        .filter_map(|line| {
            let cells = line.split('\t').map(str::trim).collect::<Vec<_>>();
            let id = cells.first().copied().unwrap_or_default();
            let command = cells.get(1).copied().unwrap_or_default();
            let status = cells.get(2).copied().unwrap_or_default();
            (!id.is_empty() && id != "none" && status.eq_ignore_ascii_case("available")).then(
                || CodeLanguage {
                    id: id.to_string(),
                    command: command.to_string(),
                },
            )
        })
        .collect()
}

pub(super) fn set_code_template_if_empty(code_text: &Arc<Mutex<String>>, language: &str) {
    if code_text
        .lock()
        .map(|value| value.trim().is_empty())
        .unwrap_or(false)
    {
        set_code_template(code_text, language);
    }
}

fn set_code_template(code_text: &Arc<Mutex<String>>, language: &str) {
    if let Ok(mut value) = code_text.lock() {
        *value = template_for_language(language);
    }
}

fn template_for_language(language: &str) -> String {
    match language {
        "python" | "python3" => {
            format!("print(\"hello from rust-desk-light ({language})\")\n")
        }
        "node" => {
            format!("console.log(\"hello from rust-desk-light ({language})\");\n")
        }
        "powershell" => {
            format!("Write-Output \"hello from rust-desk-light ({language})\"\n")
        }
        "bash" | "sh" => {
            format!("echo \"hello from rust-desk-light ({language})\"\n")
        }
        _ => String::new(),
    }
}

fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

use super::ui;
use crate::i18n::t;
use eframe::egui;
use std::sync::{atomic::AtomicBool, Arc, Mutex};

pub(super) fn render(
    ui: &mut egui::Ui,
    file_path: &Arc<Mutex<String>>,
    file_args: &Arc<Mutex<String>>,
    working_dir: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    ui::render_text_field(ui, t("File Path"), file_path, t("Path on the client"));
    ui.add_space(8.0);
    ui::render_text_field(ui, t("Arguments"), file_args, "--flag value");
    ui.add_space(8.0);
    ui::render_text_field(ui, t("Working Directory"), working_dir, t("Optional"));
    ui.add_space(12.0);
    let can_run = file_path
        .lock()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    ui::render_run_button(ui, can_run, t("File path is required"), send_requested);
}

pub(super) fn payload_for(path: &str, args: &str, working_dir: &str) -> String {
    let mut lines = vec![
        "action=run".to_string(),
        format!("path={}", sanitize_single_line(path)),
    ];
    if !args.trim().is_empty() {
        lines.push(format!("args={}", sanitize_single_line(args)));
    }
    if !working_dir.trim().is_empty() {
        lines.push(format!("working_dir={}", sanitize_single_line(working_dir)));
    }
    lines.join("\n")
}

fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::payload_for;

    #[test]
    fn execute_file_payload_includes_optional_fields() {
        let payload = payload_for("/bin/echo", "\"hello world\"", "/tmp");

        assert!(payload.contains("path=/bin/echo"));
        assert!(payload.contains("args=\"hello world\""));
        assert!(payload.contains("working_dir=/tmp"));
    }
}

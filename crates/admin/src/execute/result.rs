use super::ui;
use crate::i18n::t;
use eframe::egui;
use std::sync::{Arc, Mutex};

pub(super) fn status_text(accepted: bool, detail: &str) -> String {
    if !accepted {
        return t("Rejected").to_string();
    }
    detail
        .lines()
        .find_map(|line| line.strip_prefix("status="))
        .map(|status| match status.trim() {
            "success" => t("Completed").to_string(),
            "failed" => t("Failed").to_string(),
            other if !other.is_empty() => format!("{}: {other}", t("Status")),
            _ => t("Completed").to_string(),
        })
        .unwrap_or_else(|| t("Completed").to_string())
}

pub(super) fn output_text(detail: &str) -> String {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut section = None;

    for line in detail.lines() {
        match line.trim_end() {
            "stdout:" => {
                section = Some("stdout");
                continue;
            }
            "stderr:" => {
                section = Some("stderr");
                continue;
            }
            _ => {}
        }

        match section {
            Some("stdout") => stdout.push(line.to_string()),
            Some("stderr") => stderr.push(line.to_string()),
            _ => {}
        }
    }

    trim_empty_lines(&mut stdout);
    trim_empty_lines(&mut stderr);

    match (!stdout.is_empty(), !stderr.is_empty()) {
        (true, false) => stdout.join("\n"),
        (false, true) => stderr.join("\n"),
        (true, true) => format!(
            "stdout:\n{}\n\nstderr:\n{}",
            stdout.join("\n"),
            stderr.join("\n")
        ),
        (false, false) => payload_field(detail, "message").unwrap_or_default(),
    }
}

pub(super) fn render(ui: &mut egui::Ui, result_detail: &Arc<Mutex<String>>) {
    let detail = result_detail
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(t("Output"))
            .size(12.0)
            .color(crate::theme::palette().muted),
    );
    ui.add_space(4.0);
    let height = ui.available_height().clamp(96.0, 180.0);
    let mut output = if detail.trim().is_empty() {
        t("No output yet").to_string()
    } else {
        detail
    };
    let output_rows = output.lines().count().clamp(6, 120);
    let output_content_height = (output_rows as f32 * ui::CODE_ROW_HEIGHT + 18.0).max(height);
    egui::ScrollArea::vertical()
        .id_salt(("execute_output_scroll", Arc::as_ptr(result_detail)))
        .auto_shrink([false, false])
        .max_height(height)
        .show(ui, |ui| {
            ui.add_sized(
                [ui.available_width(), output_content_height],
                egui::TextEdit::multiline(&mut output)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(output_rows)
                    .interactive(false),
            );
        });
}

fn trim_empty_lines(lines: &mut Vec<String>) {
    while lines
        .first()
        .map(|line| line.trim().is_empty())
        .unwrap_or(false)
    {
        lines.remove(0);
    }
    while lines
        .last()
        .map(|line| line.trim().is_empty())
        .unwrap_or(false)
    {
        lines.pop();
    }
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

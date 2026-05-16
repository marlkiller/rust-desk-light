use crate::windowing;
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use rdl_protocol::{
    default_static_command_preset_id, static_command_preset_label, static_command_presets,
    CommandKind,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;
const INLINE_LABEL_WIDTH: f32 = 86.0;
const CODE_ROW_HEIGHT: f32 = 18.0;
const STATUS_BAR_HEIGHT: f32 = 42.0;

pub(crate) struct ExecuteWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    command: CommandKind,
    file_path: Arc<Mutex<String>>,
    file_args: Arc<Mutex<String>>,
    working_dir: Arc<Mutex<String>>,
    code_language: Arc<Mutex<String>>,
    code_text: Arc<Mutex<String>>,
    code_languages: Arc<Mutex<Vec<CodeLanguage>>>,
    language_status: Arc<Mutex<String>>,
    language_probe_requested: Arc<AtomicBool>,
    static_preset: Arc<Mutex<String>>,
    static_custom_mode: Arc<AtomicBool>,
    static_custom_command: Arc<Mutex<String>>,
    result_status: Arc<Mutex<String>>,
    result_detail: Arc<Mutex<String>>,
    open: bool,
    close_requested: Arc<AtomicBool>,
    send_requested: Arc<AtomicBool>,
}

pub(crate) struct OutboundExecuteCommand {
    pub(crate) client_id: String,
    pub(crate) command: CommandKind,
    pub(crate) payload: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodeLanguage {
    id: String,
    command: String,
}

pub(crate) fn open_window(
    windows: &mut Vec<ExecuteWindow>,
    client_id: &str,
    hostname: String,
    username: String,
    command: CommandKind,
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id && window.command == command)
    {
        window.open = true;
        window.hostname = hostname;
        window.username = username;
        if command == CommandKind::ExecuteCode
            && window
                .code_languages
                .lock()
                .map(|languages| languages.is_empty())
                .unwrap_or(true)
        {
            window
                .language_probe_requested
                .store(true, Ordering::Relaxed);
        }
        return;
    }

    windows.push(ExecuteWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        command: command.clone(),
        file_path: Arc::new(Mutex::new(String::new())),
        file_args: Arc::new(Mutex::new(String::new())),
        working_dir: Arc::new(Mutex::new(String::new())),
        code_language: Arc::new(Mutex::new(String::new())),
        code_text: Arc::new(Mutex::new(String::new())),
        code_languages: Arc::new(Mutex::new(Vec::new())),
        language_status: Arc::new(Mutex::new(if command == CommandKind::ExecuteCode {
            "Loading languages...".to_string()
        } else {
            String::new()
        })),
        language_probe_requested: Arc::new(AtomicBool::new(command == CommandKind::ExecuteCode)),
        static_preset: Arc::new(Mutex::new(default_static_command_preset_id().to_string())),
        static_custom_mode: Arc::new(AtomicBool::new(false)),
        static_custom_command: Arc::new(Mutex::new(String::new())),
        result_status: Arc::new(Mutex::new(String::new())),
        result_detail: Arc::new(Mutex::new(String::new())),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
        send_requested: Arc::new(AtomicBool::new(false)),
    });
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<ExecuteWindow>,
) -> Vec<OutboundExecuteCommand> {
    let mut outbound = Vec::new();
    for window in windows.iter_mut() {
        if window.close_requested.load(Ordering::Relaxed) {
            window.open = false;
        }
        if !window.open {
            continue;
        }

        let client_id = window.client_id.clone();
        let title = format!(
            "{} - {}",
            command_title(&window.command),
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id =
            egui::ViewportId::from_hash_of(("admin_execute", &client_id, window.command.as_str()));
        let builder = windowing::child_viewport_builder(title, [640.0, 520.0], [480.0, 360.0]);

        let command = window.command.clone();
        let file_path = window.file_path.clone();
        let file_args = window.file_args.clone();
        let working_dir = window.working_dir.clone();
        let code_language = window.code_language.clone();
        let code_text = window.code_text.clone();
        let code_languages = window.code_languages.clone();
        let language_status = window.language_status.clone();
        let language_probe_requested = window.language_probe_requested.clone();
        let static_preset = window.static_preset.clone();
        let static_custom_mode = window.static_custom_mode.clone();
        let static_custom_command = window.static_custom_command.clone();
        let result_status = window.result_status.clone();
        let result_detail = window.result_detail.clone();
        let close_requested = window.close_requested.clone();
        let send_requested = window.send_requested.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
                .show_inside(ui, |ui| {
                    windowing::render_child_window_controls(ui);
                    render_form(
                        ui,
                        &command,
                        &file_path,
                        &file_args,
                        &working_dir,
                        &code_language,
                        &code_text,
                        &code_languages,
                        &language_status,
                        &language_probe_requested,
                        &static_preset,
                        &static_custom_mode,
                        &static_custom_command,
                        &result_status,
                        &result_detail,
                        &send_requested,
                    );
                });
        });

        if window
            .language_probe_requested
            .swap(false, Ordering::Relaxed)
            && window.command == CommandKind::ExecuteCode
        {
            if let Ok(mut status) = window.language_status.lock() {
                *status = "Loading languages...".to_string();
            }
            outbound.push(OutboundExecuteCommand {
                client_id: client_id.clone(),
                command: CommandKind::ExecuteCode,
                payload: "action=languages".to_string(),
            });
        }

        if window.send_requested.swap(false, Ordering::Relaxed) {
            if let Ok(mut status) = window.result_status.lock() {
                *status = "Running...".to_string();
            }
            if let Ok(mut detail) = window.result_detail.lock() {
                detail.clear();
            }
            outbound.push(OutboundExecuteCommand {
                client_id: client_id.clone(),
                command: window.command.clone(),
                payload: payload_for_window(window),
            });
        }
    }

    windows.retain(|window| window.open);
    outbound
}

pub(crate) fn handle_ack(
    windows: &mut [ExecuteWindow],
    client_id: &str,
    command: &CommandKind,
    accepted: bool,
    detail: &str,
) -> bool {
    if !matches!(
        command,
        CommandKind::ExecuteFile | CommandKind::ExecuteCode | CommandKind::ExecuteStaticCommand
    ) {
        return false;
    }
    let Some(window) = windows.iter_mut().find(|window| {
        window.client_id == client_id
            && (window.command == *command
                || (detail.starts_with("execute_code_languages:")
                    && window.command == CommandKind::ExecuteCode))
    }) else {
        return false;
    };

    if detail.starts_with("execute_code_languages:") {
        handle_language_ack(window, detail);
        return true;
    }

    if let Ok(mut status) = window.result_status.lock() {
        *status = result_status_text(accepted, detail);
    }
    if let Ok(mut target) = window.result_detail.lock() {
        *target = result_output_text(detail);
    }
    true
}

fn handle_language_ack(window: &mut ExecuteWindow, detail: &str) {
    let languages = parse_language_response(detail);
    if let Ok(mut target) = window.code_languages.lock() {
        *target = languages.clone();
    }
    if languages.is_empty() {
        if let Ok(mut status) = window.language_status.lock() {
            *status = "No supported language found".to_string();
        }
        return;
    }

    if let Ok(mut selected) = window.code_language.lock() {
        if !languages.iter().any(|language| language.id == *selected) {
            *selected = languages[0].id.clone();
            set_code_template_if_empty(&window.code_text, &selected);
        }
    }
    if let Ok(mut status) = window.language_status.lock() {
        *status = format!("{} language(s) available", languages.len());
    }
}

fn result_status_text(accepted: bool, detail: &str) -> String {
    if !accepted {
        return "Rejected".to_string();
    }
    detail
        .lines()
        .find_map(|line| line.strip_prefix("status="))
        .map(|status| match status.trim() {
            "success" => "Completed".to_string(),
            "failed" => "Failed".to_string(),
            other if !other.is_empty() => format!("Status: {other}"),
            _ => "Completed".to_string(),
        })
        .unwrap_or_else(|| "Completed".to_string())
}

fn result_output_text(detail: &str) -> String {
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

fn render_form(
    ui: &mut egui::Ui,
    command: &CommandKind,
    file_path: &Arc<Mutex<String>>,
    file_args: &Arc<Mutex<String>>,
    working_dir: &Arc<Mutex<String>>,
    code_language: &Arc<Mutex<String>>,
    code_text: &Arc<Mutex<String>>,
    code_languages: &Arc<Mutex<Vec<CodeLanguage>>>,
    language_status: &Arc<Mutex<String>>,
    language_probe_requested: &Arc<AtomicBool>,
    static_preset: &Arc<Mutex<String>>,
    static_custom_mode: &Arc<AtomicBool>,
    static_custom_command: &Arc<Mutex<String>>,
    result_status: &Arc<Mutex<String>>,
    result_detail: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    let has_result = !result_status
        .lock()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
        || !result_detail
            .lock()
            .map(|value| value.trim().is_empty())
            .unwrap_or(true);
    render_status_panel(ui, result_status);

    egui::CentralPanel::no_frame().show_inside(ui, |ui| {
        egui::Frame::default()
            .fill(COLOR_PANEL)
            .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
            .corner_radius(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| match command {
                CommandKind::ExecuteFile => {
                    render_execute_file(ui, file_path, file_args, working_dir, send_requested)
                }
                CommandKind::ExecuteCode => render_execute_code(
                    ui,
                    code_language,
                    code_text,
                    code_languages,
                    language_status,
                    language_probe_requested,
                    has_result,
                    send_requested,
                ),
                CommandKind::ExecuteStaticCommand => render_static_command(
                    ui,
                    static_preset,
                    static_custom_mode,
                    static_custom_command,
                    send_requested,
                ),
                _ => {}
            });
        render_result(ui, result_detail);
    });
}

fn render_execute_file(
    ui: &mut egui::Ui,
    file_path: &Arc<Mutex<String>>,
    file_args: &Arc<Mutex<String>>,
    working_dir: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    render_text_field(ui, "File Path", file_path, "Path on the client");
    ui.add_space(8.0);
    render_text_field(ui, "Arguments", file_args, "--flag value");
    ui.add_space(8.0);
    render_text_field(ui, "Working Directory", working_dir, "Optional");
    ui.add_space(12.0);
    let can_run = file_path
        .lock()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    render_run_button(ui, can_run, "File path is required", send_requested);
}

fn render_execute_code(
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
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        render_inline_label(ui, "Language");
        egui::ComboBox::from_id_salt("execute_code_language")
            .width(140.0)
            .selected_text(if selected.is_empty() {
                "Loading..."
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
        if ui.button("Refresh").clicked() {
            language_probe_requested.store(true, Ordering::Relaxed);
        }
        let status = language_status
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        if !status.trim().is_empty() {
            ui.label(egui::RichText::new(status).size(12.0).color(COLOR_MUTED));
        }
    });
    ui.add_space(8.0);
    let mut code = code_text
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    ui.label(egui::RichText::new("Code").size(12.0).color(COLOR_MUTED));
    let editor_height = if has_result {
        (ui.available_height() * 0.46).clamp(160.0, 240.0)
    } else {
        (ui.available_height() - TOOLBAR_CONTROL_HEIGHT - 28.0).clamp(180.0, 280.0)
    };
    let desired_rows = code.lines().count().clamp(12, 240);
    let editor_content_height = (desired_rows as f32 * CODE_ROW_HEIGHT + 18.0).max(editor_height);
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
    render_run_button(
        ui,
        can_run,
        "Language and code are required",
        send_requested,
    );
}

fn render_static_command(
    ui: &mut egui::Ui,
    static_preset: &Arc<Mutex<String>>,
    static_custom_mode: &Arc<AtomicBool>,
    static_custom_command: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    let mut custom_mode = static_custom_mode.load(Ordering::Relaxed);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        render_inline_label(ui, "Mode");
        if ui.selectable_label(!custom_mode, "Preset").clicked() {
            custom_mode = false;
            static_custom_mode.store(false, Ordering::Relaxed);
        }
        if ui.selectable_label(custom_mode, "Custom").clicked() {
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
        render_inline_text_field(ui, "Command", static_custom_command, "whoami");
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
            render_inline_label(ui, "Preset");
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
    render_run_button(ui, can_run, "Command is required", send_requested);
}

fn render_result(ui: &mut egui::Ui, result_detail: &Arc<Mutex<String>>) {
    let detail = result_detail
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    if detail.trim().is_empty() {
        return;
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Output").size(12.0).color(COLOR_MUTED));
    ui.add_space(4.0);
    let height = ui.available_height().clamp(96.0, 180.0);
    let mut output = detail;
    let output_rows = output.lines().count().clamp(6, 120);
    let output_content_height = (output_rows as f32 * CODE_ROW_HEIGHT + 18.0).max(height);
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

fn render_status_panel(ui: &mut egui::Ui, result_status: &Arc<Mutex<String>>) {
    egui::Panel::bottom(egui::Id::new((
        "execute_status_panel",
        Arc::as_ptr(result_status),
    )))
    .exact_size(STATUS_BAR_HEIGHT)
    .show_separator_line(false)
    .frame(
        egui::Frame::default()
            .fill(COLOR_BG)
            .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
            .inner_margin(egui::Margin::symmetric(8, 6)),
    )
    .show_inside(ui, |ui| render_status_bar(ui, result_status));
}

fn render_status_bar(ui: &mut egui::Ui, result_status: &Arc<Mutex<String>>) {
    let status = result_status
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let status = status_bar_text(&status);

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), TOOLBAR_CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            render_inline_label(ui, "Status");
            ui.label(egui::RichText::new(status).size(12.0).color(COLOR_TEXT));
        },
    );
}

fn status_bar_text(status: &str) -> String {
    if status.trim().is_empty() {
        "Ready".to_string()
    } else {
        status.to_string()
    }
}

fn render_inline_label(ui: &mut egui::Ui, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(INLINE_LABEL_WIDTH, TOOLBAR_CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(label).size(12.0).color(COLOR_MUTED));
        },
    );
}

fn render_inline_text_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &Arc<Mutex<String>>,
    hint: &str,
) {
    let mut text = value.lock().map(|value| value.clone()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        render_inline_label(ui, label);
        let response = ui.add_sized(
            [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut text)
                .hint_text(hint)
                .vertical_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(mut value) = value.lock() {
                *value = text;
            }
        }
    });
}

fn render_text_field(ui: &mut egui::Ui, label: &str, value: &Arc<Mutex<String>>, hint: &str) {
    let mut text = value.lock().map(|value| value.clone()).unwrap_or_default();
    ui.label(egui::RichText::new(label).size(12.0).color(COLOR_MUTED));
    let response = ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .vertical_align(egui::Align::Center),
    );
    if response.changed() {
        if let Ok(mut value) = value.lock() {
            *value = text;
        }
    }
}

fn render_run_button(
    ui: &mut egui::Ui,
    can_run: bool,
    disabled_message: &str,
    send_requested: &Arc<AtomicBool>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(can_run, egui::Button::new("Run")).clicked() {
                send_requested.store(true, Ordering::Relaxed);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            if !can_run && !disabled_message.is_empty() {
                ui.label(
                    egui::RichText::new(disabled_message)
                        .size(12.0)
                        .color(COLOR_TEXT),
                );
            }
        });
    });
}

fn payload_for_window(window: &ExecuteWindow) -> String {
    match window.command {
        CommandKind::ExecuteFile => payload_for_execute_file(
            &lock_string(&window.file_path),
            &lock_string(&window.file_args),
            &lock_string(&window.working_dir),
        ),
        CommandKind::ExecuteCode => payload_for_execute_code(
            &lock_string(&window.code_language),
            &lock_string(&window.code_text),
        ),
        CommandKind::ExecuteStaticCommand => payload_for_static_command(
            &lock_string(&window.static_preset),
            window.static_custom_mode.load(Ordering::Relaxed),
            &lock_string(&window.static_custom_command),
        ),
        _ => String::new(),
    }
}

fn payload_for_execute_file(path: &str, args: &str, working_dir: &str) -> String {
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

fn payload_for_execute_code(language: &str, code: &str) -> String {
    [
        "action=run".to_string(),
        format!("language={}", sanitize_single_line(language)),
        format!("code_b64={}", STANDARD.encode(code)),
    ]
    .join("\n")
}

fn payload_for_static_command(preset: &str, custom_mode: bool, custom_command: &str) -> String {
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

fn parse_language_response(detail: &str) -> Vec<CodeLanguage> {
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

fn set_code_template_if_empty(code_text: &Arc<Mutex<String>>, language: &str) {
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
        *value = template_for_language(language).to_string();
    }
}

fn template_for_language(language: &str) -> &'static str {
    match language {
        "python" | "python3" => "print(\"hello from rust-desk-light\")\n",
        "node" => "console.log(\"hello from rust-desk-light\");\n",
        "powershell" => "Write-Output \"hello from rust-desk-light\"\n",
        "bash" | "sh" => "echo \"hello from rust-desk-light\"\n",
        _ => "",
    }
}

fn lock_string(value: &Arc<Mutex<String>>) -> String {
    value.lock().map(|value| value.clone()).unwrap_or_default()
}

fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

fn identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

fn command_title(command: &CommandKind) -> String {
    command
        .as_str()
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        handle_ack, open_window, parse_language_response, payload_for_execute_code,
        payload_for_execute_file, payload_for_static_command, result_output_text, status_bar_text,
        template_for_language,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    use rdl_protocol::CommandKind;

    #[test]
    fn execute_file_payload_includes_optional_fields() {
        let payload = payload_for_execute_file("/bin/echo", "\"hello world\"", "/tmp");

        assert!(payload.contains("path=/bin/echo"));
        assert!(payload.contains("args=\"hello world\""));
        assert!(payload.contains("working_dir=/tmp"));
    }

    #[test]
    fn execute_code_payload_encodes_code() {
        let payload = payload_for_execute_code("python3", "print('hi')");

        assert!(payload.contains("language=python3"));
        assert!(payload.contains(&format!("code_b64={}", STANDARD.encode("print('hi')"))));
    }

    #[test]
    fn static_command_payload_uses_preset() {
        assert_eq!(
            payload_for_static_command("hostname", false, ""),
            "action=run\nmode=preset\npreset=hostname"
        );
    }

    #[test]
    fn static_command_payload_encodes_custom_command() {
        let payload = payload_for_static_command("hostname", true, "echo hello && whoami");

        assert!(payload.contains("mode=custom"));
        assert!(payload.contains(&format!(
            "command_b64={}",
            STANDARD.encode("echo hello && whoami")
        )));
    }

    #[test]
    fn language_response_parses_available_rows() {
        let languages = parse_language_response(
            "execute_code_languages:\nLanguage\tCommand\tStatus\npython3\tpython3\tavailable\nnone\t-\tunavailable",
        );

        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].id, "python3");
        assert_eq!(languages[0].command, "python3");
    }

    #[test]
    fn language_templates_include_hello_world() {
        assert!(template_for_language("python3").contains("hello"));
        assert!(template_for_language("node").contains("hello"));
        assert!(template_for_language("bash").contains("hello"));
    }

    #[test]
    fn status_bar_defaults_to_ready() {
        assert_eq!(status_bar_text(""), "Ready");
        assert_eq!(status_bar_text("Running..."), "Running...");
    }

    #[test]
    fn run_ack_updates_execute_window_result() {
        let mut windows = Vec::new();
        open_window(
            &mut windows,
            "client-1",
            "host".to_string(),
            "user".to_string(),
            CommandKind::ExecuteStaticCommand,
        );

        assert!(handle_ack(
            &mut windows,
            "client-1",
            &CommandKind::ExecuteStaticCommand,
            true,
            "execute_static_command\nstatus=success\nstdout:\nhello",
        ));

        assert_eq!(
            windows[0].result_status.lock().unwrap().as_str(),
            "Completed"
        );
        assert_eq!(windows[0].result_detail.lock().unwrap().as_str(), "hello");
    }

    #[test]
    fn result_output_omits_execute_metadata() {
        assert_eq!(
            result_output_text(
                "execute_code\nlanguage=python3\ncommand=python3\nstatus=success\nstdout:\nhello from rust-desk-light",
            ),
            "hello from rust-desk-light"
        );
    }

    #[test]
    fn language_ack_does_not_replace_execute_result() {
        let mut windows = Vec::new();
        open_window(
            &mut windows,
            "client-1",
            "host".to_string(),
            "user".to_string(),
            CommandKind::ExecuteCode,
        );
        *windows[0].result_detail.lock().unwrap() = "previous output".to_string();

        assert!(handle_ack(
            &mut windows,
            "client-1",
            &CommandKind::ExecuteCode,
            true,
            "execute_code_languages:\nLanguage\tCommand\tStatus\npython3\tpython3\tavailable",
        ));

        assert_eq!(
            windows[0].result_detail.lock().unwrap().as_str(),
            "previous output"
        );
        assert_eq!(windows[0].code_language.lock().unwrap().as_str(), "python3");
    }
}

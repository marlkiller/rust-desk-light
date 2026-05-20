use super::{
    create_task,
    execute_code::{self, CodeLanguage},
    execute_file, execute_static_command, result, ui,
};
use crate::{
    i18n::{self, t, tf},
    windowing,
};
use eframe::egui;
use rdl_protocol::{default_static_command_preset_id, CommandKind};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

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
    task_manager: create_task::TaskManagerState,
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
        if command == CommandKind::CreateTask {
            window.task_manager.queue_refresh(&window.send_requested);
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
            t("Loading languages...").to_string()
        } else {
            String::new()
        })),
        language_probe_requested: Arc::new(AtomicBool::new(command == CommandKind::ExecuteCode)),
        static_preset: Arc::new(Mutex::new(default_static_command_preset_id().to_string())),
        static_custom_mode: Arc::new(AtomicBool::new(false)),
        static_custom_command: Arc::new(Mutex::new(String::new())),
        task_manager: create_task::TaskManagerState::default(),
        result_status: Arc::new(Mutex::new(String::new())),
        result_detail: Arc::new(Mutex::new(String::new())),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
        send_requested: Arc::new(AtomicBool::new(command == CommandKind::CreateTask)),
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
        let (default_size, min_size) = if window.command == CommandKind::CreateTask {
            ([820.0, 620.0], [620.0, 460.0])
        } else {
            ([640.0, 520.0], [480.0, 360.0])
        };
        let builder = windowing::child_viewport_builder(title, default_size, min_size);

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
        let task_manager = window.task_manager.clone();
        let result_status = window.result_status.clone();
        let result_detail = window.result_detail.clone();
        let close_requested = window.close_requested.clone();
        let send_requested = window.send_requested.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(crate::theme::page_frame())
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
                        &task_manager,
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
                *status = t("Loading languages...").to_string();
            }
            outbound.push(OutboundExecuteCommand {
                client_id: client_id.clone(),
                command: CommandKind::ExecuteCode,
                payload: "action=languages".to_string(),
            });
        }

        if window.send_requested.swap(false, Ordering::Relaxed) {
            if let Ok(mut status) = window.result_status.lock() {
                *status = t("Running...").to_string();
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
        CommandKind::ExecuteFile
            | CommandKind::ExecuteCode
            | CommandKind::ExecuteStaticCommand
            | CommandKind::CreateTask
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
        *status = result::status_text(accepted, detail);
    }
    if let Ok(mut target) = window.result_detail.lock() {
        *target = result::output_text(detail);
    }
    true
}

fn handle_language_ack(window: &mut ExecuteWindow, detail: &str) {
    let languages = execute_code::parse_language_response(detail);
    if let Ok(mut target) = window.code_languages.lock() {
        *target = languages.clone();
    }
    if languages.is_empty() {
        if let Ok(mut status) = window.language_status.lock() {
            *status = t("No supported language found").to_string();
        }
        return;
    }

    if let Ok(mut selected) = window.code_language.lock() {
        if !languages.iter().any(|language| language.id == *selected) {
            *selected = languages[0].id.clone();
            execute_code::set_code_template_if_empty(&window.code_text, &selected);
        }
    }
    if let Ok(mut status) = window.language_status.lock() {
        *status = tf(
            "{count} language(s) available",
            &[("count", &languages.len().to_string())],
        );
    }
}

#[allow(clippy::too_many_arguments)]
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
    task_manager: &create_task::TaskManagerState,
    result_status: &Arc<Mutex<String>>,
    result_detail: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    ui::render_status_panel(ui, result_status);

    egui::CentralPanel::no_frame().show_inside(ui, |ui| {
        egui::Frame::default()
            .fill(crate::theme::palette().panel)
            .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
            .corner_radius(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| match command {
                CommandKind::ExecuteFile => {
                    execute_file::render(ui, file_path, file_args, working_dir, send_requested)
                }
                CommandKind::ExecuteCode => execute_code::render(
                    ui,
                    code_language,
                    code_text,
                    code_languages,
                    language_status,
                    language_probe_requested,
                    true,
                    send_requested,
                ),
                CommandKind::ExecuteStaticCommand => execute_static_command::render(
                    ui,
                    static_preset,
                    static_custom_mode,
                    static_custom_command,
                    send_requested,
                ),
                CommandKind::CreateTask => {
                    create_task::render(ui, task_manager, result_detail, send_requested)
                }
                _ => {}
            });
        if command != &CommandKind::CreateTask {
            result::render(ui, result_detail);
        }
    });
}

fn payload_for_window(window: &ExecuteWindow) -> String {
    match window.command {
        CommandKind::ExecuteFile => execute_file::payload_for(
            &lock_string(&window.file_path),
            &lock_string(&window.file_args),
            &lock_string(&window.working_dir),
        ),
        CommandKind::ExecuteCode => execute_code::payload_for(
            &lock_string(&window.code_language),
            &lock_string(&window.code_text),
        ),
        CommandKind::ExecuteStaticCommand => execute_static_command::payload_for(
            &lock_string(&window.static_preset),
            window.static_custom_mode.load(Ordering::Relaxed),
            &lock_string(&window.static_custom_command),
        ),
        CommandKind::CreateTask => window.task_manager.payload(),
        _ => String::new(),
    }
}

fn lock_string(value: &Arc<Mutex<String>>) -> String {
    value.lock().map(|value| value.clone()).unwrap_or_default()
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
    i18n::command_title(command).to_string()
}

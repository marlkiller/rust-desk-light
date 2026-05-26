use crate::{
    i18n::t,
    theme::{color_bad, color_good, color_warn},
    windowing,
};
use eframe::egui;
use rdl_protocol::{
    default_static_command_preset_id, static_command_preset_label, static_command_presets,
    static_command_script_for_os, CommandOutputStream, REMOTE_TERMINAL_CANCEL,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::CONTROL_HEIGHT;

pub(crate) struct TerminalWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    os: String,
    lines: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<TerminalStatus>>,
    current_dir: Arc<Mutex<String>>,
    draft: Arc<Mutex<String>>,
    history: Arc<Mutex<Vec<String>>>,
    history_cursor: Arc<Mutex<Option<usize>>>,
    preset_command: Arc<Mutex<String>>,
    outbound: Arc<Mutex<Vec<TerminalOutbound>>>,
    copy_requested: Arc<AtomicBool>,
    clear_requested: Arc<AtomicBool>,
    open: bool,
    close_requested: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
enum TerminalStatus {
    Ready,
    Running,
    Done,
    Failed,
}

pub(crate) struct OutboundCommand {
    pub(crate) client_id: String,
    pub(crate) command: String,
}

struct TerminalOutbound {
    command: String,
    visible: bool,
}

pub(crate) fn open_window(
    windows: &mut Vec<TerminalWindow>,
    client_id: &str,
    hostname: String,
    username: String,
    os: String,
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    {
        window.open = true;
        window.hostname = hostname;
        window.username = username;
        window.os = os;
        window.close_requested.store(false, Ordering::Relaxed);
        if window
            .current_dir
            .lock()
            .map(|value| value.trim().is_empty())
            .unwrap_or(false)
        {
            window.queue_hidden("cd");
        }
        return;
    }

    let window = TerminalWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        os,
        lines: Arc::new(Mutex::new(Vec::new())),
        status: Arc::new(Mutex::new(TerminalStatus::Ready)),
        current_dir: Arc::new(Mutex::new(String::new())),
        draft: Arc::new(Mutex::new(String::new())),
        history: Arc::new(Mutex::new(Vec::new())),
        history_cursor: Arc::new(Mutex::new(None)),
        preset_command: Arc::new(Mutex::new(default_static_command_preset_id().to_string())),
        outbound: Arc::new(Mutex::new(Vec::new())),
        copy_requested: Arc::new(AtomicBool::new(false)),
        clear_requested: Arc::new(AtomicBool::new(false)),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
    };
    window.queue_hidden("cd");
    windows.push(window);
}

pub(crate) fn handle_ack(
    windows: &mut [TerminalWindow],
    client_id: &str,
    hostname: String,
    username: String,
    os: String,
    accepted: bool,
    detail: String,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if !window.open {
        return;
    }
    window.hostname = hostname;
    window.username = username;
    window.os = os;

    let (current_dir, output) = parse_terminal_detail(&detail);
    if let Some(current_dir) = current_dir {
        if let Ok(mut value) = window.current_dir.lock() {
            *value = current_dir;
        }
    }
    if let Ok(mut status) = window.status.lock() {
        *status = if accepted && !terminal_output_failed(&output) {
            TerminalStatus::Done
        } else {
            TerminalStatus::Failed
        };
    }
    if let Ok(mut lines) = window.lines.lock() {
        let output = output.trim();
        if !accepted {
            lines.push(format!("error: {output}"));
        } else if !output.is_empty() && output != "ok" {
            lines.push(output.to_string());
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_output(
    windows: &mut [TerminalWindow],
    client_id: &str,
    hostname: String,
    username: String,
    os: String,
    _stream_id: u64,
    _sequence: u64,
    stream: CommandOutputStream,
    chunk: String,
    current_dir: String,
    finished: bool,
    success: bool,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if !window.open {
        return;
    }
    window.hostname = hostname;
    window.username = username;
    window.os = os;

    if !current_dir.trim().is_empty() {
        if let Ok(mut value) = window.current_dir.lock() {
            *value = current_dir;
        }
    }
    if let Ok(mut status) = window.status.lock() {
        *status = if finished {
            if success {
                TerminalStatus::Done
            } else {
                TerminalStatus::Failed
            }
        } else {
            TerminalStatus::Running
        };
    }
    if let Ok(mut lines) = window.lines.lock() {
        append_terminal_output(&mut lines, stream, &chunk, finished);
    }
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<TerminalWindow>,
) -> Vec<OutboundCommand> {
    let mut outbound = Vec::new();
    for window in windows.iter_mut() {
        if window.close_requested.swap(false, Ordering::Relaxed) {
            if terminal_is_running(&window.status) {
                outbound.push(OutboundCommand {
                    client_id: window.client_id.clone(),
                    command: REMOTE_TERMINAL_CANCEL.to_string(),
                });
            }
            window.open = false;
        }
        if !window.open {
            continue;
        }

        let client_id = window.client_id.clone();
        let title = format!(
            "{} - {}",
            t("Remote Terminal"),
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("admin_remote_terminal", &client_id));
        let builder = windowing::child_viewport_builder(title, [760.0, 520.0], [420.0, 320.0]);

        let lines = window.lines.clone();
        let status = window.status.clone();
        let current_dir = window.current_dir.clone();
        let draft = window.draft.clone();
        let history = window.history.clone();
        let history_cursor = window.history_cursor.clone();
        let preset_command = window.preset_command.clone();
        let outbound_queue = window.outbound.clone();
        let copy_requested = window.copy_requested.clone();
        let clear_requested = window.clear_requested.clone();
        let close_requested = window.close_requested.clone();
        let history_id = client_id.clone();
        let target_os = window.os.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(crate::theme::page_frame())
                .show_inside(ui, |ui| {
                    windowing::render_child_window_controls(ui);
                    render_toolbar(
                        ui,
                        &lines,
                        &status,
                        &target_os,
                        &preset_command,
                        &outbound_queue,
                        &copy_requested,
                        &clear_requested,
                    );
                    ui.add_space(8.0);
                    let input_height = 42.0;
                    let status_height = 44.0;
                    let toolbar_height = 38.0;
                    let history_height = (ui.available_height()
                        - toolbar_height
                        - input_height
                        - status_height
                        - 16.0)
                        .max(120.0);
                    crate::theme::panel_frame_with_margin(10.0).show(ui, |ui| {
                        ui.set_min_height(history_height);
                        ui.set_max_height(history_height);
                        egui::ScrollArea::vertical()
                            .id_salt(("admin_remote_terminal_history", &history_id))
                            .stick_to_bottom(true)
                            .auto_shrink([false, false])
                            .show(ui, |ui| render_history(ui, &lines));
                    });
                    ui.add_space(8.0);
                    render_input(
                        ui,
                        &draft,
                        &history,
                        &history_cursor,
                        &outbound_queue,
                        &status,
                        &current_dir,
                    );
                    ui.add_space(8.0);
                    render_status_bar(ui, &status, &current_dir);
                });
        });

        if window.clear_requested.swap(false, Ordering::Relaxed) {
            if let Ok(mut lines) = window.lines.lock() {
                lines.clear();
            }
        }
        let command = window
            .outbound
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop());
        if let Some(outbound_item) = command {
            if outbound_item.visible {
                let prompt = window
                    .current_dir
                    .lock()
                    .map(|value| prompt_label(&value))
                    .unwrap_or_else(|_| "$".to_string());
                if let Ok(mut lines) = window.lines.lock() {
                    lines.push(format!("{prompt} {}", outbound_item.command));
                }
                if let Ok(mut history) = window.history.lock() {
                    if history.last() != Some(&outbound_item.command) {
                        history.push(outbound_item.command.clone());
                    }
                }
            }
            if let Ok(mut status) = window.status.lock() {
                *status = TerminalStatus::Running;
            }
            outbound.push(OutboundCommand {
                client_id: client_id.clone(),
                command: outbound_item.command,
            });
        }
    }

    windows.retain(|window| window.open);
    outbound
}

impl TerminalWindow {
    fn queue_hidden(&self, command: &str) {
        if let Ok(mut queue) = self.outbound.lock() {
            queue.insert(
                0,
                TerminalOutbound {
                    command: command.to_string(),
                    visible: false,
                },
            );
        }
    }
}

fn render_toolbar(
    ui: &mut egui::Ui,
    lines: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<TerminalStatus>>,
    target_os: &str,
    preset_command: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<TerminalOutbound>>>,
    copy_requested: &Arc<AtomicBool>,
    clear_requested: &Arc<AtomicBool>,
) {
    toolbar_row(ui, |ui| {
        let running = status
            .lock()
            .map(|status| matches!(*status, TerminalStatus::Running))
            .unwrap_or(false);
        ui.label(
            egui::RichText::new(t("Terminal"))
                .size(13.0)
                .color(crate::theme::palette().text)
                .strong(),
        );
        ui.separator();
        if ui
            .add_enabled(running, egui::Button::new(t("Stop")))
            .clicked()
        {
            if let Ok(mut queue) = outbound.lock() {
                queue.insert(
                    0,
                    TerminalOutbound {
                        command: REMOTE_TERMINAL_CANCEL.to_string(),
                        visible: false,
                    },
                );
            }
        }
        ui.separator();
        render_preset_shortcut(ui, running, target_os, preset_command, outbound);
        ui.separator();
        if ui.button(t("Copy All")).clicked() {
            if let Ok(lines) = lines.lock() {
                ui.ctx().copy_text(lines.join("\n"));
            }
            copy_requested.store(true, Ordering::Relaxed);
        }
        if ui.button(t("Clear")).clicked() {
            clear_requested.store(true, Ordering::Relaxed);
        }
    });
}

fn toolbar_row(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        ui.horizontal(add_contents);
    });
}

fn render_preset_shortcut(
    ui: &mut egui::Ui,
    running: bool,
    target_os: &str,
    preset_command: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<TerminalOutbound>>>,
) {
    let mut selected = preset_command
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| default_static_command_preset_id().to_string());
    egui::ComboBox::from_id_salt("remote_terminal_preset_command")
        .width(150.0)
        .selected_text(static_command_preset_label(&selected))
        .show_ui(ui, |ui| {
            for preset in static_command_presets() {
                if ui
                    .selectable_label(selected == preset.id, preset.label)
                    .clicked()
                {
                    selected = preset.id.to_string();
                    if let Ok(mut value) = preset_command.lock() {
                        *value = selected.clone();
                    }
                }
            }
        });
    if ui
        .add_enabled(!running, egui::Button::new(t("Run Preset")))
        .clicked()
    {
        if let Some(command) = static_command_script_for_os(&selected, target_os) {
            if let Ok(mut queue) = outbound.lock() {
                queue.insert(
                    0,
                    TerminalOutbound {
                        command: command.to_string(),
                        visible: true,
                    },
                );
            }
            ui.ctx().request_repaint();
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

fn terminal_is_running(status: &Arc<Mutex<TerminalStatus>>) -> bool {
    status
        .lock()
        .map(|status| matches!(*status, TerminalStatus::Running))
        .unwrap_or(false)
}

fn render_history(ui: &mut egui::Ui, lines: &Arc<Mutex<Vec<String>>>) {
    if let Ok(lines) = lines.lock() {
        let mut transcript = lines.join("\n");
        ui.add(
            egui::TextEdit::multiline(&mut transcript)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(18),
        );
    }
}

fn render_input(
    ui: &mut egui::Ui,
    draft: &Arc<Mutex<String>>,
    history: &Arc<Mutex<Vec<String>>>,
    history_cursor: &Arc<Mutex<Option<usize>>>,
    outbound: &Arc<Mutex<Vec<TerminalOutbound>>>,
    status: &Arc<Mutex<TerminalStatus>>,
    current_dir: &Arc<Mutex<String>>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        let mut text = draft.lock().map(|value| value.clone()).unwrap_or_default();
        let running = status
            .lock()
            .map(|status| matches!(*status, TerminalStatus::Running))
            .unwrap_or(false);
        let prompt = current_dir
            .lock()
            .map(|value| prompt_label(&value))
            .unwrap_or_else(|_| "$".to_string());
        let button_width = 72.0;
        let spacing = ui.spacing().item_spacing.x;
        let available_width = ui.available_width();
        let prompt_width = (available_width * 0.22)
            .clamp(56.0, 180.0)
            .min((available_width - button_width - spacing * 2.0 - 100.0).max(56.0));
        ui.add_sized(
            [prompt_width, TOOLBAR_CONTROL_HEIGHT],
            egui::Label::new(
                egui::RichText::new(prompt)
                    .font(egui::FontId::monospace(13.0))
                    .color(crate::theme::palette().muted),
            )
            .selectable(false)
            .truncate()
            .halign(egui::Align::Max),
        );
        let input_width =
            (available_width - prompt_width - button_width - spacing * 2.0).max(100.0);
        let response = ui.add_sized(
            [input_width, TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut text)
                .hint_text(t("Command"))
                .vertical_align(egui::Align::Center),
        );
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            apply_history_delta(history, history_cursor, draft, -1);
            ui.ctx().request_repaint();
            return;
        }
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            apply_history_delta(history, history_cursor, draft, 1);
            ui.ctx().request_repaint();
            return;
        }
        if response.changed() {
            if let Ok(mut draft) = draft.lock() {
                *draft = text.clone();
            }
        }
        let run_clicked = ui
            .add_enabled_ui(!running, |ui| {
                ui.add_sized(
                    [button_width, TOOLBAR_CONTROL_HEIGHT],
                    egui::Button::new(t("Run")),
                )
                .clicked()
            })
            .inner
            || (!running
                && response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if !running && run_clicked && !text.trim().is_empty() {
            if let Ok(mut queue) = outbound.lock() {
                queue.insert(
                    0,
                    TerminalOutbound {
                        command: text.trim().to_string(),
                        visible: true,
                    },
                );
            }
            if let Ok(mut draft) = draft.lock() {
                draft.clear();
            }
            if let Ok(mut cursor) = history_cursor.lock() {
                *cursor = None;
            }
            ui.ctx().request_repaint();
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    });
}

fn render_status_bar(
    ui: &mut egui::Ui,
    status: &Arc<Mutex<TerminalStatus>>,
    current_dir: &Arc<Mutex<String>>,
) {
    let status = status
        .lock()
        .map(|status| *status)
        .unwrap_or(TerminalStatus::Ready);
    let current_dir = current_dir
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let (label, color) = match status {
        TerminalStatus::Ready => (t("Ready"), crate::theme::palette().muted),
        TerminalStatus::Running => (t("Running"), color_warn()),
        TerminalStatus::Done => (t("Done"), color_good()),
        TerminalStatus::Failed => (t("Failed"), color_bad()),
    };
    let progress_text = if current_dir.trim().is_empty() {
        t("cwd: resolving...").to_string()
    } else {
        format!("cwd: {current_dir}")
    };
    crate::theme::status_frame().show(ui, |ui| {
        ui.set_min_height(26.0);
        crate::theme::render_status_line(ui, label, color, &progress_text, |_| {});
    });
}

fn apply_history_delta(
    history: &Arc<Mutex<Vec<String>>>,
    history_cursor: &Arc<Mutex<Option<usize>>>,
    draft: &Arc<Mutex<String>>,
    delta: isize,
) {
    let Ok(history) = history.lock() else {
        return;
    };
    if history.is_empty() {
        return;
    }
    let Ok(mut cursor) = history_cursor.lock() else {
        return;
    };
    let current = cursor.unwrap_or(history.len());
    let next = if delta < 0 {
        current.saturating_sub(1)
    } else {
        (current + 1).min(history.len())
    };
    *cursor = if next >= history.len() {
        None
    } else {
        Some(next)
    };
    if let Ok(mut draft) = draft.lock() {
        *draft = cursor
            .and_then(|index| history.get(index).cloned())
            .unwrap_or_default();
    }
}

fn prompt_label(current_dir: &str) -> String {
    let current_dir = current_dir.trim();
    if current_dir.is_empty() {
        "$".to_string()
    } else {
        format!("{} $", compact_path(current_dir))
    }
}

fn compact_path(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

fn parse_terminal_detail(detail: &str) -> (Option<String>, String) {
    let Some(rest) = detail.strip_prefix("__rdl_terminal_cwd\t") else {
        return (None, detail.to_string());
    };
    let (current_dir, output) = rest
        .split_once('\n')
        .map(|(current_dir, output)| (current_dir.to_string(), output.to_string()))
        .unwrap_or_else(|| (rest.to_string(), String::new()));
    (Some(current_dir), output)
}

fn terminal_output_failed(output: &str) -> bool {
    let output = output.trim().to_ascii_lowercase();
    output.starts_with("cd failed:") || output.contains(" exited with error")
}

fn append_terminal_output(
    lines: &mut Vec<String>,
    stream: CommandOutputStream,
    chunk: &str,
    finished: bool,
) {
    let chunk = chunk.trim_end_matches('\0');
    if chunk.trim().is_empty() {
        return;
    }
    let text = if stream == CommandOutputStream::Status && !finished {
        format!("[status] {}", chunk.trim_end())
    } else if stream == CommandOutputStream::Status {
        chunk.trim_end().to_string()
    } else {
        chunk.replace('\r', "\n")
    };
    for line in text.lines() {
        lines.push(line.to_string());
    }
}

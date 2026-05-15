use crate::windowing;
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);
const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;

pub(crate) struct TerminalWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    lines: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<TerminalStatus>>,
    current_dir: Arc<Mutex<String>>,
    draft: Arc<Mutex<String>>,
    history: Arc<Mutex<Vec<String>>>,
    history_cursor: Arc<Mutex<Option<usize>>>,
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
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    {
        window.open = true;
        window.hostname = hostname;
        window.username = username;
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
        lines: Arc::new(Mutex::new(Vec::new())),
        status: Arc::new(Mutex::new(TerminalStatus::Ready)),
        current_dir: Arc::new(Mutex::new(String::new())),
        draft: Arc::new(Mutex::new(String::new())),
        history: Arc::new(Mutex::new(Vec::new())),
        history_cursor: Arc::new(Mutex::new(None)),
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
    windows: &mut Vec<TerminalWindow>,
    client_id: &str,
    hostname: String,
    username: String,
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

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<TerminalWindow>,
) -> Vec<OutboundCommand> {
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
            "Remote Terminal - {}",
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
        let outbound_queue = window.outbound.clone();
        let copy_requested = window.copy_requested.clone();
        let clear_requested = window.clear_requested.clone();
        let close_requested = window.close_requested.clone();
        let history_id = client_id.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
                .show_inside(ui, |ui| {
                    windowing::render_child_window_controls(ui);
                    render_toolbar(ui, &lines, &copy_requested, &clear_requested);
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
                    egui::Frame::default()
                        .fill(COLOR_PANEL)
                        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
                        .inner_margin(10.0)
                        .show(ui, |ui| {
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
    copy_requested: &Arc<AtomicBool>,
    clear_requested: &Arc<AtomicBool>,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Terminal")
                .size(13.0)
                .color(COLOR_TEXT)
                .strong(),
        );
        ui.separator();
        if ui.button("Copy All").clicked() {
            if let Ok(lines) = lines.lock() {
                ui.ctx().copy_text(lines.join("\n"));
            }
            copy_requested.store(true, Ordering::Relaxed);
        }
        if ui.button("Clear").clicked() {
            clear_requested.store(true, Ordering::Relaxed);
        }
    });
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
        ui.label(
            egui::RichText::new(prompt)
                .font(egui::FontId::monospace(13.0))
                .color(COLOR_MUTED),
        );
        let button_width = 72.0;
        let input_width =
            (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(100.0);
        let response = ui.add_sized(
            [input_width, TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut text)
                .hint_text("Command")
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
                    egui::Button::new("Run"),
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
        TerminalStatus::Ready => ("Ready", COLOR_MUTED),
        TerminalStatus::Running => ("Running", COLOR_WARN),
        TerminalStatus::Done => ("Done", COLOR_GOOD),
        TerminalStatus::Failed => ("Failed", COLOR_BAD),
    };
    let progress_text = if current_dir.trim().is_empty() {
        "cwd: resolving...".to_string()
    } else {
        format!("cwd: {current_dir}")
    };
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_min_height(26.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(
                    egui::RichText::new(label)
                        .size(12.0)
                        .color(COLOR_TEXT)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(progress_text)
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
            });
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

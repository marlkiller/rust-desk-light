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

pub(crate) struct TerminalWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    lines: Arc<Mutex<Vec<String>>>,
    status: Arc<Mutex<TerminalStatus>>,
    current_dir: Arc<Mutex<String>>,
    draft: Arc<Mutex<String>>,
    outbound: Arc<Mutex<Vec<String>>>,
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
        return;
    }

    windows.push(TerminalWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        lines: Arc::new(Mutex::new(Vec::new())),
        status: Arc::new(Mutex::new(TerminalStatus::Ready)),
        current_dir: Arc::new(Mutex::new(String::new())),
        draft: Arc::new(Mutex::new(String::new())),
        outbound: Arc::new(Mutex::new(Vec::new())),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
    });
}

pub(crate) fn handle_ack(
    windows: &mut Vec<TerminalWindow>,
    client_id: &str,
    hostname: String,
    username: String,
    accepted: bool,
    detail: String,
) {
    open_window(windows, client_id, hostname, username);
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };

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
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([760.0, 520.0])
            .with_min_inner_size([420.0, 320.0])
            .with_resizable(true);

        let lines = window.lines.clone();
        let status = window.status.clone();
        let current_dir = window.current_dir.clone();
        let draft = window.draft.clone();
        let outbound_queue = window.outbound.clone();
        let close_requested = window.close_requested.clone();
        let history_id = client_id.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
                .show_inside(ui, |ui| {
                    let input_height = 42.0;
                    let status_height = 44.0;
                    let history_height =
                        (ui.available_height() - input_height - status_height - 16.0).max(120.0);
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
                    render_input(ui, &draft, &outbound_queue, &status);
                    ui.add_space(8.0);
                    render_status_bar(ui, &status, &current_dir);
                });
        });

        let command = window
            .outbound
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop());
        if let Some(command) = command {
            if let Ok(mut lines) = window.lines.lock() {
                lines.push(format!("> {command}"));
            }
            if let Ok(mut status) = window.status.lock() {
                *status = TerminalStatus::Running;
            }
            outbound.push(OutboundCommand {
                client_id: client_id.clone(),
                command,
            });
        }
    }

    windows.retain(|window| window.open);
    outbound
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
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<TerminalStatus>>,
) {
    ui.horizontal(|ui| {
        let mut text = draft.lock().map(|value| value.clone()).unwrap_or_default();
        let running = status
            .lock()
            .map(|status| matches!(*status, TerminalStatus::Running))
            .unwrap_or(false);
        let button_width = 72.0;
        let input_width =
            (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(100.0);
        let response = ui.add_sized(
            [input_width, 28.0],
            egui::TextEdit::singleline(&mut text).hint_text("Command"),
        );
        if response.changed() {
            if let Ok(mut draft) = draft.lock() {
                *draft = text.clone();
            }
        }
        let run_clicked = ui
            .add_enabled_ui(!running, |ui| {
                ui.add_sized([button_width, 28.0], egui::Button::new("Run"))
                    .clicked()
            })
            .inner
            || (!running
                && response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if !running && run_clicked && !text.trim().is_empty() {
            if let Ok(mut queue) = outbound.lock() {
                queue.insert(0, text.trim().to_string());
            }
            if let Ok(mut draft) = draft.lock() {
                draft.clear();
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
        TerminalStatus::Running => ("Pending", COLOR_WARN),
        TerminalStatus::Done => ("Done", COLOR_GOOD),
        TerminalStatus::Failed => ("Failed", COLOR_BAD),
    };
    let progress_text = if current_dir.trim().is_empty() {
        "cwd: unknown".to_string()
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

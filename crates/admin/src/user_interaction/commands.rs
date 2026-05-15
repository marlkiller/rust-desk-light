use crate::windowing;
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use rdl_protocol::CommandKind;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;

pub(crate) struct InteractionCommandWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    command: CommandKind,
    title: Arc<Mutex<String>>,
    body: Arc<Mutex<String>>,
    open: bool,
    close_requested: Arc<AtomicBool>,
    send_requested: Arc<AtomicBool>,
}

pub(crate) struct OutboundInteractionCommand {
    pub(crate) client_id: String,
    pub(crate) command: CommandKind,
    pub(crate) payload: String,
}

pub(crate) fn open_window(
    windows: &mut Vec<InteractionCommandWindow>,
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
        return;
    }

    let (title, body) = default_fields(&command);
    windows.push(InteractionCommandWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        command,
        title: Arc::new(Mutex::new(title)),
        body: Arc::new(Mutex::new(body)),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
        send_requested: Arc::new(AtomicBool::new(false)),
    });
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<InteractionCommandWindow>,
) -> Vec<OutboundInteractionCommand> {
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
        let viewport_id = egui::ViewportId::from_hash_of((
            "admin_user_interaction_command",
            &client_id,
            window.command.as_str(),
        ));
        let builder = windowing::child_viewport_builder(title, [520.0, 430.0], [420.0, 320.0]);

        let command = window.command.clone();
        let field_title = window.title.clone();
        let body = window.body.clone();
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
                    render_form(ui, &command, &field_title, &body, &send_requested);
                });
        });

        if window.send_requested.swap(false, Ordering::Relaxed) {
            let title = window
                .title
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            let body = window
                .body
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            outbound.push(OutboundInteractionCommand {
                client_id: client_id.clone(),
                command: window.command.clone(),
                payload: payload_for(&window.command, &title, &body),
            });
            window.open = false;
        }
    }

    windows.retain(|window| window.open);
    outbound
}

fn render_form(
    ui: &mut egui::Ui,
    command: &CommandKind,
    title: &Arc<Mutex<String>>,
    body: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(8.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                render_title_field(ui, command, title);
                ui.add_space(8.0);
                render_body_field(ui, command, body);
                ui.add_space(10.0);
                render_actions(ui, body, send_requested);
            });
        });
}

fn render_title_field(ui: &mut egui::Ui, command: &CommandKind, title: &Arc<Mutex<String>>) {
    let mut value = title.lock().map(|value| value.clone()).unwrap_or_default();
    ui.label(
        egui::RichText::new(title_label(command))
            .size(12.0)
            .color(COLOR_MUTED),
    );
    let response = ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut value)
            .hint_text(title_hint(command))
            .vertical_align(egui::Align::Center),
    );
    if response.changed() {
        if let Ok(mut title) = title.lock() {
            *title = value;
        }
    }
}

fn render_body_field(ui: &mut egui::Ui, command: &CommandKind, body: &Arc<Mutex<String>>) {
    let mut value = body.lock().map(|value| value.clone()).unwrap_or_default();
    ui.label(
        egui::RichText::new(body_label(command))
            .size(12.0)
            .color(COLOR_MUTED),
    );
    let body_height = (ui.available_height() - TOOLBAR_CONTROL_HEIGHT - 36.0).max(150.0);
    let response = ui.add_sized(
        [ui.available_width(), body_height],
        egui::TextEdit::multiline(&mut value)
            .desired_width(f32::INFINITY)
            .desired_rows(12),
    );
    if response.changed() {
        if let Ok(mut body) = body.lock() {
            *body = value;
        }
    }
}

fn render_actions(ui: &mut egui::Ui, body: &Arc<Mutex<String>>, send_requested: &Arc<AtomicBool>) {
    let can_send = body
        .lock()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(can_send, egui::Button::new("Send"))
                .clicked()
            {
                send_requested.store(true, Ordering::Relaxed);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            if !can_send {
                ui.label(
                    egui::RichText::new("Body is empty")
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
            }
        });
    });
}

fn payload_for(command: &CommandKind, title: &str, body: &str) -> String {
    let body_key = match command {
        CommandKind::OpenTextInNotepad => "text_b64",
        _ => "message_b64",
    };
    let title_key = match command {
        CommandKind::OpenTextInNotepad => "file_name",
        _ => "title",
    };
    let mut lines = vec![
        format!("{title_key}={}", sanitize_single_line(title)),
        format!("{body_key}={}", STANDARD.encode(body)),
    ];
    if matches!(command, CommandKind::MessageBox) {
        lines.push("kind=info".to_string());
    }
    lines.join("\n")
}

fn default_fields(command: &CommandKind) -> (String, String) {
    match command {
        CommandKind::OpenTextInNotepad => ("rdl-note.txt".to_string(), String::new()),
        _ => ("Rust Desk Light".to_string(), String::new()),
    }
}

fn title_label(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::OpenTextInNotepad => "File Name",
        _ => "Title",
    }
}

fn title_hint(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::OpenTextInNotepad => "rdl-note.txt",
        _ => "Rust Desk Light",
    }
}

fn body_label(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::BalloonTip => "Notification",
        CommandKind::OpenTextInNotepad => "Text",
        _ => "Message",
    }
}

fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
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
    use super::payload_for;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use rdl_protocol::CommandKind;

    #[test]
    fn encodes_message_box_payload() {
        let payload = payload_for(&CommandKind::MessageBox, "Title", "hello\nworld");

        assert!(payload.contains("title=Title"));
        assert!(payload.contains("kind=info"));
        assert!(payload.contains(&format!("message_b64={}", STANDARD.encode("hello\nworld"))));
    }

    #[test]
    fn encodes_open_text_payload_with_file_name() {
        let payload = payload_for(&CommandKind::OpenTextInNotepad, "note.txt", "body");

        assert!(payload.contains("file_name=note.txt"));
        assert!(payload.contains(&format!("text_b64={}", STANDARD.encode("body"))));
    }
}

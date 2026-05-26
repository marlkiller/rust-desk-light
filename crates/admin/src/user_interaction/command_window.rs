use super::{balloon_tip, message_box, open_text_in_notepad};
use crate::{
    i18n::{self, t},
    theme::{color_bad, color_good, color_warn},
    windowing,
};
use eframe::egui;
use rdl_protocol::CommandKind;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::CONTROL_HEIGHT;
const STATUS_BAR_HEIGHT: f32 = 44.0;

pub(crate) struct InteractionCommandWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    command: CommandKind,
    title: Arc<Mutex<String>>,
    body: Arc<Mutex<String>>,
    status: Arc<Mutex<InteractionStatus>>,
    notice: Arc<Mutex<String>>,
    open: bool,
    close_requested: Arc<AtomicBool>,
    send_requested: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InteractionStatus {
    Ready,
    Sending,
    Done,
    Failed,
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
        set_status(
            &window.status,
            &window.notice,
            InteractionStatus::Ready,
            t("Ready"),
        );
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
        status: Arc::new(Mutex::new(InteractionStatus::Ready)),
        notice: Arc::new(Mutex::new(t("Ready").to_string())),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
        send_requested: Arc::new(AtomicBool::new(false)),
    });
}

pub(crate) fn handle_ack(
    windows: &mut [InteractionCommandWindow],
    client_id: &str,
    command: &CommandKind,
    accepted: bool,
    detail: &str,
) -> bool {
    if !interaction_command_is_supported(command) {
        return false;
    }

    let Some(window) = windows
        .iter_mut()
        .rev()
        .find(|window| window.client_id == client_id && &window.command == command)
    else {
        return false;
    };

    let detail_failed = interaction_detail_failed(detail);
    let status = if accepted && !detail_failed {
        InteractionStatus::Done
    } else {
        InteractionStatus::Failed
    };
    let notice = interaction_notice(
        detail,
        if accepted {
            t("Command sent")
        } else {
            t("Command failed")
        },
    );
    set_status(&window.status, &window.notice, status, &notice);
    true
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
        let status = window.status.clone();
        let notice = window.notice.clone();
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
                        &field_title,
                        &body,
                        &status,
                        &notice,
                        &send_requested,
                    );
                });
        });

        if window.send_requested.swap(false, Ordering::Relaxed) {
            set_status(
                &window.status,
                &window.notice,
                InteractionStatus::Sending,
                t("Sending command..."),
            );
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
    status: &Arc<Mutex<InteractionStatus>>,
    notice: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    egui::Panel::bottom(egui::Id::new((
        "interaction_command_status_panel",
        Arc::as_ptr(status),
    )))
    .exact_size(STATUS_BAR_HEIGHT)
    .show_separator_line(false)
    .frame(
        egui::Frame::default()
            .fill(crate::theme::palette().bg)
            .inner_margin(0.0),
    )
    .show_inside(ui, |ui| {
        ui.add_space(8.0);
        render_status_bar(ui, status, notice);
    });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::default()
                .fill(crate::theme::palette().bg)
                .inner_margin(0.0),
        )
        .show_inside(ui, |ui| {
            crate::theme::panel_frame()
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
        });
}

fn render_status_bar(
    ui: &mut egui::Ui,
    status: &Arc<Mutex<InteractionStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let status = status
        .lock()
        .map(|status| *status)
        .unwrap_or(InteractionStatus::Ready);
    let notice = notice.lock().map(|value| value.clone()).unwrap_or_default();
    let (label, default_notice, color) = match status {
        InteractionStatus::Ready => (t("Ready"), t("Ready"), crate::theme::palette().muted),
        InteractionStatus::Sending => (t("Sending"), t("Waiting for client result"), color_warn()),
        InteractionStatus::Done => (t("Done"), t("Command sent"), color_good()),
        InteractionStatus::Failed => (t("Failed"), t("Command failed"), color_bad()),
    };
    let notice = if notice.trim().is_empty() {
        default_notice
    } else {
        notice.trim()
    };

    crate::theme::status_frame().show(ui, |ui| {
        ui.set_min_height(26.0);
        crate::theme::render_status_line(ui, label, color, notice, |_| {});
    });
}

fn render_title_field(ui: &mut egui::Ui, command: &CommandKind, title: &Arc<Mutex<String>>) {
    let mut value = title.lock().map(|value| value.clone()).unwrap_or_default();
    ui.label(
        egui::RichText::new(title_label(command))
            .size(12.0)
            .color(crate::theme::palette().muted),
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
            .color(crate::theme::palette().muted),
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
                .add_enabled(can_send, egui::Button::new(t("Send")))
                .clicked()
            {
                send_requested.store(true, Ordering::Relaxed);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            if !can_send {
                ui.label(
                    egui::RichText::new(t("Body is empty"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
            }
        });
    });
}

fn payload_for(command: &CommandKind, title: &str, body: &str) -> String {
    match command {
        CommandKind::MessageBox => message_box::payload_for(title, body),
        CommandKind::BalloonTip => balloon_tip::payload_for(title, body),
        CommandKind::OpenTextInNotepad => open_text_in_notepad::payload_for(title, body),
        _ => String::new(),
    }
}

fn default_fields(command: &CommandKind) -> (String, String) {
    match command {
        CommandKind::MessageBox => message_box::default_fields(),
        CommandKind::BalloonTip => balloon_tip::default_fields(),
        CommandKind::OpenTextInNotepad => open_text_in_notepad::default_fields(),
        _ => ("Rust Desk Light".to_string(), String::new()),
    }
}

fn title_label(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::OpenTextInNotepad => open_text_in_notepad::title_label(),
        _ => message_box::title_label(),
    }
}

fn title_hint(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::OpenTextInNotepad => open_text_in_notepad::title_hint(),
        _ => message_box::title_hint(),
    }
}

fn body_label(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::BalloonTip => balloon_tip::body_label(),
        CommandKind::OpenTextInNotepad => open_text_in_notepad::body_label(),
        _ => message_box::body_label(),
    }
}

fn interaction_command_is_supported(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::OpenTextInNotepad
    )
}

fn interaction_notice(detail: &str, fallback: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() || detail.eq_ignore_ascii_case("ok") {
        fallback.to_string()
    } else if detail.eq_ignore_ascii_case("forwarded") {
        t("Sent to client").to_string()
    } else if interaction_detail_failed(detail) {
        detail_field(detail, "message").unwrap_or_else(|| detail_header(detail).to_string())
    } else if let Some(status) = detail_field(detail, "status") {
        status
    } else if let Some(message) = detail_field(detail, "message") {
        message
    } else {
        let header = detail_header(detail);
        if matches!(
            header,
            "message_box" | "balloon_tip" | "open_text_in_notepad"
        ) {
            fallback.to_string()
        } else {
            header.to_string()
        }
    }
}

fn interaction_detail_failed(detail: &str) -> bool {
    let header = detail_header(detail);
    header.ends_with("_error") || header.ends_with("_disabled")
}

fn detail_header(detail: &str) -> &str {
    detail.lines().next().unwrap_or_default().trim()
}

fn detail_field(detail: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    detail.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn set_status(
    status: &Arc<Mutex<InteractionStatus>>,
    notice: &Arc<Mutex<String>>,
    next_status: InteractionStatus,
    next_notice: &str,
) {
    if let Ok(mut status) = status.lock() {
        *status = next_status;
    }
    if let Ok(mut notice) = notice.lock() {
        *notice = next_notice.to_string();
    }
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

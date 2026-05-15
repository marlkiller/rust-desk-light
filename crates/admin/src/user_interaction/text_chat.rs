use crate::windowing;
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;
const CHAT_FRAME_VERTICAL_MARGIN: f32 = 20.0;
const BOTTOM_SAFE_SPACE: f32 = 6.0;

pub(crate) struct ChatWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    messages: Arc<Mutex<Vec<ChatLine>>>,
    draft: Arc<Mutex<String>>,
    outbound: Arc<Mutex<Vec<String>>>,
    open: bool,
    close_requested: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ChatLine {
    sender: String,
    text: String,
}

pub(crate) struct OutboundMessage {
    pub(crate) client_id: String,
    pub(crate) text: String,
}

pub(crate) fn open_window(
    windows: &mut Vec<ChatWindow>,
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
        return;
    }

    windows.push(ChatWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        messages: Arc::new(Mutex::new(Vec::new())),
        draft: Arc::new(Mutex::new(String::new())),
        outbound: Arc::new(Mutex::new(Vec::new())),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
    });
}

pub(crate) fn handle_ack(
    windows: &mut Vec<ChatWindow>,
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

    if detail == "chat_delivered" {
        return;
    }

    let (sender, text) = if let Some(text) = detail.strip_prefix("chat_message:") {
        ("Client", text.trim_start())
    } else if accepted {
        ("Client", detail.as_str())
    } else {
        ("System", detail.as_str())
    };
    push_line(window, sender, text);
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<ChatWindow>,
) -> Vec<OutboundMessage> {
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
            "Text Chat - {}",
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("admin_text_chat", &client_id));
        let builder = windowing::child_viewport_builder(title, [480.0, 420.0], [360.0, 300.0]);

        let messages = window.messages.clone();
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
                    windowing::render_child_window_controls(ui);
                    let history_height = (ui.available_height()
                        - TOOLBAR_CONTROL_HEIGHT
                        - CHAT_FRAME_VERTICAL_MARGIN
                        - 8.0
                        - BOTTOM_SAFE_SPACE)
                        .max(80.0);
                    egui::Frame::default()
                        .fill(COLOR_PANEL)
                        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.set_min_height(history_height);
                            ui.set_max_height(history_height);
                            egui::ScrollArea::vertical()
                                .id_salt(("admin_text_chat_history", &history_id))
                                .stick_to_bottom(true)
                                .auto_shrink([false, false])
                                .show(ui, |ui| render_messages(ui, &messages));
                        });
                    ui.add_space(8.0);
                    render_input(ui, &draft, &outbound_queue);
                    ui.add_space(BOTTOM_SAFE_SPACE);
                });
        });

        let text = window
            .outbound
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop());
        if let Some(text) = text {
            push_line(window, "Admin", &text);
            outbound.push(OutboundMessage {
                client_id: client_id.clone(),
                text,
            });
        }
    }

    windows.retain(|window| window.open);
    outbound
}

fn render_messages(ui: &mut egui::Ui, messages: &Arc<Mutex<Vec<ChatLine>>>) {
    if let Ok(messages) = messages.lock() {
        let mut transcript = if messages.is_empty() {
            "No messages yet.".to_string()
        } else {
            messages
                .iter()
                .map(|message| format!("{}: {}", message.sender, message.text))
                .collect::<Vec<_>>()
                .join("\n")
        };
        ui.add(
            egui::TextEdit::multiline(&mut transcript)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(12),
        );
    }
}

fn render_input(ui: &mut egui::Ui, draft: &Arc<Mutex<String>>, outbound: &Arc<Mutex<Vec<String>>>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        let mut text = draft.lock().map(|value| value.clone()).unwrap_or_default();
        let button_width = 72.0;
        let input_width =
            (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(80.0);
        let response = ui.add_sized(
            [input_width, TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut text)
                .hint_text("Message")
                .vertical_align(egui::Align::Center),
        );
        response.context_menu(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(text.clone());
                ui.close();
            }
            if ui.button("Paste").clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                ui.close();
            }
        });
        if response.changed() {
            if let Ok(mut draft) = draft.lock() {
                *draft = text.clone();
            }
        }
        let send_clicked = ui
            .add_sized(
                [button_width, TOOLBAR_CONTROL_HEIGHT],
                egui::Button::new("Send"),
            )
            .clicked()
            || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if send_clicked && !text.trim().is_empty() {
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

fn push_line(window: &mut ChatWindow, sender: &str, text: &str) {
    if let Ok(mut messages) = window.messages.lock() {
        messages.push(ChatLine {
            sender: sender.to_string(),
            text: text.to_string(),
        });
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

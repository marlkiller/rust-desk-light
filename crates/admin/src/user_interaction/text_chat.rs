use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);

pub(crate) struct ChatWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    messages: Arc<Mutex<Vec<ChatLine>>>,
    draft: Arc<Mutex<String>>,
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
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([480.0, 420.0])
            .with_min_inner_size([360.0, 300.0])
            .with_resizable(true);

        let messages = window.messages.clone();
        let draft = window.draft.clone();
        let close_requested = window.close_requested.clone();
        let send_requested = Arc::new(Mutex::new(None::<String>));
        let send_requested_ui = send_requested.clone();
        let history_id = client_id.clone();

        ctx.show_viewport_deferred(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
                .show_inside(ui, |ui| {
                    let input_height = 42.0;
                    let history_height = (ui.available_height() - input_height - 8.0).max(80.0);
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
                    render_input(ui, &draft, &send_requested_ui);
                });
        });

        let text = send_requested
            .lock()
            .ok()
            .and_then(|mut request| request.take());
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
        if messages.is_empty() {
            ui.label(egui::RichText::new("No messages yet.").color(COLOR_TEXT));
            return;
        }
        for message in messages.iter() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}:", message.sender))
                        .strong()
                        .color(COLOR_TEXT),
                );
                ui.label(egui::RichText::new(&message.text).color(COLOR_TEXT));
            });
        }
    }
}

fn render_input(
    ui: &mut egui::Ui,
    draft: &Arc<Mutex<String>>,
    send_requested: &Arc<Mutex<Option<String>>>,
) {
    ui.horizontal(|ui| {
        let mut text = draft.lock().map(|value| value.clone()).unwrap_or_default();
        let response = ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text("Message")
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            if let Ok(mut draft) = draft.lock() {
                *draft = text.clone();
            }
        }
        let send_clicked = ui.button("Send").clicked()
            || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if send_clicked && !text.trim().is_empty() {
            if let Ok(mut request) = send_requested.lock() {
                *request = Some(text.trim().to_string());
            }
            if let Ok(mut draft) = draft.lock() {
                draft.clear();
            }
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

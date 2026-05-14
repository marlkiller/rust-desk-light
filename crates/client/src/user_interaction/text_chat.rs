use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);

pub(crate) struct ChatWindow {
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

pub(crate) fn receive_admin_message(window: &mut Option<ChatWindow>, text: String) {
    let window = window.get_or_insert_with(|| ChatWindow {
        messages: Arc::new(Mutex::new(Vec::new())),
        draft: Arc::new(Mutex::new(String::new())),
        outbound: Arc::new(Mutex::new(Vec::new())),
        open: true,
        close_requested: Arc::new(AtomicBool::new(false)),
    });
    window.open = true;
    window.close_requested.store(false, Ordering::Relaxed);
    push_line(window, "Admin", &text);
}

pub(crate) fn render_window(ctx: &egui::Context, window: &mut Option<ChatWindow>) -> Vec<String> {
    let Some(window) = window else {
        return Vec::new();
    };
    if window.close_requested.load(Ordering::Relaxed) {
        window.open = false;
    }
    if !window.open {
        return Vec::new();
    }

    let mut outbound = Vec::new();
    let viewport_id = egui::ViewportId::from_hash_of("client_text_chat");
    let builder = egui::ViewportBuilder::default()
        .with_title("Text Chat")
        .with_inner_size([480.0, 420.0])
        .with_min_inner_size([360.0, 300.0])
        .with_resizable(true);

    let messages = window.messages.clone();
    let draft = window.draft.clone();
    let outbound_queue = window.outbound.clone();
    let close_requested = window.close_requested.clone();

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
                            .id_salt("client_text_chat_history")
                            .stick_to_bottom(true)
                            .auto_shrink([false, false])
                            .show(ui, |ui| render_messages(ui, &messages));
                    });
                ui.add_space(8.0);
                render_input(ui, &draft, &outbound_queue);
            });
    });

    let text = window
        .outbound
        .lock()
        .ok()
        .and_then(|mut queue| queue.pop());
    if let Some(text) = text {
        push_line(window, "Me", &text);
        outbound.push(text);
    }
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
        let mut text = draft.lock().map(|value| value.clone()).unwrap_or_default();
        let button_width = 72.0;
        let input_width =
            (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(80.0);
        let response = ui.add_sized(
            [input_width, 28.0],
            egui::TextEdit::singleline(&mut text).hint_text("Reply"),
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
            .add_sized([button_width, 28.0], egui::Button::new("Send"))
            .clicked()
            || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if send_clicked && !text.trim().is_empty() {
            if let Ok(mut queue) = outbound.lock() {
                queue.insert(0, text.trim().to_string());
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

use crate::windowing;
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);
const VOICE_CHAT_REPAINT_MS: u64 = 20;
const CALL_BUTTON_WIDTH: f32 = 112.0;
const CALL_BUTTON_HEIGHT: f32 = 42.0;
const CALL_BUTTON_GAP: f32 = 20.0;

pub(crate) struct VoiceChatWindow {
    status: VoiceChatStatus,
    notice: String,
    started_at: Option<Instant>,
    mic_muted: Arc<AtomicBool>,
    speaker_muted: Arc<AtomicBool>,
    mic_changed: Arc<AtomicBool>,
    speaker_changed: Arc<AtomicBool>,
    accept_requested: Arc<AtomicBool>,
    decline_requested: Arc<AtomicBool>,
    end_requested: Arc<AtomicBool>,
    close_requested: Arc<AtomicBool>,
    open: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum VoiceChatStatus {
    Incoming,
    Connecting,
    Live,
    Ended,
    Failed,
}

pub(crate) enum VoiceChatAction {
    Accept,
    Decline,
    End,
    MicMuted(bool),
    SpeakerMuted(bool),
}

pub(crate) fn handle(_payload: &str, gui_mode: bool) -> String {
    if !gui_mode {
        return super::disabled_detail(&rdl_protocol::CommandKind::VoiceChat);
    }

    "voice_chat_ready\nmessage=voice chat handled by client GUI".to_string()
}

pub(crate) fn receive_invite(window: &mut Option<VoiceChatWindow>) {
    match window {
        Some(window) => {
            window.status = VoiceChatStatus::Incoming;
            window.notice = "Incoming voice chat".to_string();
            window.started_at = None;
            window.mic_muted.store(false, Ordering::Relaxed);
            window.speaker_muted.store(false, Ordering::Relaxed);
            window.mic_changed.store(false, Ordering::Relaxed);
            window.speaker_changed.store(false, Ordering::Relaxed);
            window.accept_requested.store(false, Ordering::Relaxed);
            window.decline_requested.store(false, Ordering::Relaxed);
            window.end_requested.store(false, Ordering::Relaxed);
            window.open = true;
            window.close_requested.store(false, Ordering::Relaxed);
        }
        None => {
            *window = Some(VoiceChatWindow {
                status: VoiceChatStatus::Incoming,
                notice: "Incoming voice chat".to_string(),
                started_at: None,
                mic_muted: Arc::new(AtomicBool::new(false)),
                speaker_muted: Arc::new(AtomicBool::new(false)),
                mic_changed: Arc::new(AtomicBool::new(false)),
                speaker_changed: Arc::new(AtomicBool::new(false)),
                accept_requested: Arc::new(AtomicBool::new(false)),
                decline_requested: Arc::new(AtomicBool::new(false)),
                end_requested: Arc::new(AtomicBool::new(false)),
                close_requested: Arc::new(AtomicBool::new(false)),
                open: true,
            });
        }
    }
}

pub(crate) fn mark_connecting(window: &mut Option<VoiceChatWindow>) {
    if let Some(window) = window {
        window.status = VoiceChatStatus::Connecting;
        window.notice = "Connecting voice chat".to_string();
    }
}

pub(crate) fn mark_live(window: &mut Option<VoiceChatWindow>) {
    if let Some(window) = window {
        window.status = VoiceChatStatus::Live;
        window.notice = "Voice chat connected".to_string();
        window.started_at = Some(Instant::now());
        window.open = true;
    }
}

pub(crate) fn mark_ended(window: &mut Option<VoiceChatWindow>, notice: impl Into<String>) {
    if let Some(window) = window {
        window.status = VoiceChatStatus::Ended;
        window.notice = notice.into();
        window.started_at = None;
        window.open = true;
    }
}

pub(crate) fn mark_failed(window: &mut Option<VoiceChatWindow>, notice: impl Into<String>) {
    if let Some(window) = window {
        window.status = VoiceChatStatus::Failed;
        window.notice = notice.into();
        window.started_at = None;
        window.open = true;
    }
}

pub(crate) fn render_window(
    ctx: &egui::Context,
    window: &mut Option<VoiceChatWindow>,
) -> Vec<VoiceChatAction> {
    let mut actions = Vec::new();
    let Some(call) = window else {
        return actions;
    };
    if !call.open {
        return actions;
    }

    if call.close_requested.load(Ordering::Relaxed) {
        if matches!(
            call.status,
            VoiceChatStatus::Incoming | VoiceChatStatus::Connecting | VoiceChatStatus::Live
        ) {
            call.end_requested.store(true, Ordering::Relaxed);
        } else {
            call.open = false;
        }
    }

    let viewport_id = egui::ViewportId::from_hash_of("client_voice_chat");
    let builder = windowing::child_viewport_builder("Voice Chat", [360.0, 500.0], [320.0, 420.0]);
    let status = call.status;
    let notice = call.notice.clone();
    let started_at = call.started_at;
    let mic_muted = call.mic_muted.clone();
    let speaker_muted = call.speaker_muted.clone();
    let mic_changed = call.mic_changed.clone();
    let speaker_changed = call.speaker_changed.clone();
    let accept_requested = call.accept_requested.clone();
    let decline_requested = call.decline_requested.clone();
    let end_requested = call.end_requested.clone();
    let close_requested = call.close_requested.clone();

    ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
        if ui.ctx().input(|input| input.viewport().close_requested()) {
            close_requested.store(true, Ordering::Relaxed);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(16.0))
            .show_inside(ui, |ui| {
                windowing::render_child_window_controls(ui);
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    render_avatar(ui, status);
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new(status_title(status))
                            .size(22.0)
                            .strong()
                            .color(COLOR_TEXT),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(notice.as_str())
                            .size(13.0)
                            .color(COLOR_MUTED),
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(duration_label(status, started_at))
                            .size(18.0)
                            .color(COLOR_TEXT),
                    );
                    ui.add_space(28.0);
                    render_controls(
                        ui,
                        status,
                        &mic_muted,
                        &speaker_muted,
                        &mic_changed,
                        &speaker_changed,
                        &accept_requested,
                        &decline_requested,
                        &end_requested,
                    );
                });
            });
        if matches!(status, VoiceChatStatus::Live | VoiceChatStatus::Connecting) {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(VOICE_CHAT_REPAINT_MS));
        }
    });

    if call.accept_requested.swap(false, Ordering::Relaxed) {
        call.status = VoiceChatStatus::Connecting;
        call.notice = "Connecting voice chat".to_string();
        actions.push(VoiceChatAction::Accept);
    }
    if call.decline_requested.swap(false, Ordering::Relaxed) {
        call.status = VoiceChatStatus::Ended;
        call.notice = "Declined".to_string();
        actions.push(VoiceChatAction::Decline);
    }
    if call.end_requested.swap(false, Ordering::Relaxed) {
        call.status = VoiceChatStatus::Ended;
        call.notice = "Call ended".to_string();
        call.started_at = None;
        actions.push(VoiceChatAction::End);
    }
    if call.mic_changed.swap(false, Ordering::Relaxed) {
        actions.push(VoiceChatAction::MicMuted(
            call.mic_muted.load(Ordering::Relaxed),
        ));
    }
    if call.speaker_changed.swap(false, Ordering::Relaxed) {
        actions.push(VoiceChatAction::SpeakerMuted(
            call.speaker_muted.load(Ordering::Relaxed),
        ));
    }

    if matches!(
        call.status,
        VoiceChatStatus::Ended | VoiceChatStatus::Failed
    ) && call.close_requested.load(Ordering::Relaxed)
    {
        call.open = false;
    }
    actions
}

fn render_avatar(ui: &mut egui::Ui, status: VoiceChatStatus) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(112.0, 112.0), egui::Sense::hover());
    let color = match status {
        VoiceChatStatus::Live => COLOR_GOOD,
        VoiceChatStatus::Failed => COLOR_BAD,
        VoiceChatStatus::Ended => COLOR_MUTED,
        _ => COLOR_WARN,
    };
    ui.painter().circle_filled(rect.center(), 54.0, COLOR_PANEL);
    ui.painter()
        .circle_stroke(rect.center(), 54.0, egui::Stroke::new(1.0, COLOR_BORDER));
    ui.painter()
        .circle_filled(rect.center(), 34.0, color.gamma_multiply(0.16));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Voice",
        egui::FontId::proportional(16.0),
        color,
    );
}

fn render_controls(
    ui: &mut egui::Ui,
    status: VoiceChatStatus,
    mic_muted: &Arc<AtomicBool>,
    speaker_muted: &Arc<AtomicBool>,
    mic_changed: &Arc<AtomicBool>,
    speaker_changed: &Arc<AtomicBool>,
    accept_requested: &Arc<AtomicBool>,
    decline_requested: &Arc<AtomicBool>,
    end_requested: &Arc<AtomicBool>,
) {
    match status {
        VoiceChatStatus::Incoming => {
            let button_group_width = CALL_BUTTON_WIDTH * 2.0 + CALL_BUTTON_GAP;
            ui.allocate_ui_with_layout(
                egui::vec2(button_group_width, CALL_BUTTON_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    if call_button(ui, "Decline", COLOR_BAD).clicked() {
                        decline_requested.store(true, Ordering::Relaxed);
                    }
                    ui.add_space(CALL_BUTTON_GAP);
                    if call_button(ui, "Accept", COLOR_GOOD).clicked() {
                        accept_requested.store(true, Ordering::Relaxed);
                    }
                },
            );
        }
        VoiceChatStatus::Connecting | VoiceChatStatus::Live => {
            ui.horizontal(|ui| {
                let mut mic = mic_muted.load(Ordering::Relaxed);
                let mic_label = mic_label(mic);
                if ui.checkbox(&mut mic, mic_label).changed() {
                    mic_muted.store(mic, Ordering::Relaxed);
                    mic_changed.store(true, Ordering::Relaxed);
                }
                let mut speaker = speaker_muted.load(Ordering::Relaxed);
                let speaker_label = speaker_label(speaker);
                if ui.checkbox(&mut speaker, speaker_label).changed() {
                    speaker_muted.store(speaker, Ordering::Relaxed);
                    speaker_changed.store(true, Ordering::Relaxed);
                }
            });
            ui.add_space(22.0);
            if call_button(ui, "Hang Up", COLOR_BAD).clicked() {
                end_requested.store(true, Ordering::Relaxed);
            }
        }
        VoiceChatStatus::Ended | VoiceChatStatus::Failed => {
            ui.label(egui::RichText::new("Closed").color(COLOR_MUTED));
        }
    }
}

fn mic_label(muted: bool) -> &'static str {
    if muted {
        "Mic off"
    } else {
        "Mic on"
    }
}

fn speaker_label(muted: bool) -> &'static str {
    if muted {
        "Speaker off"
    } else {
        "Speaker on"
    }
}

fn call_button(ui: &mut egui::Ui, label: &str, color: egui::Color32) -> egui::Response {
    let fill = color.gamma_multiply(0.92);
    let text = egui::RichText::new(label)
        .color(egui::Color32::WHITE)
        .strong();
    ui.add_sized(
        [CALL_BUTTON_WIDTH, CALL_BUTTON_HEIGHT],
        egui::Button::new(text)
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.65))),
    )
}

fn status_title(status: VoiceChatStatus) -> &'static str {
    match status {
        VoiceChatStatus::Incoming => "Incoming Call",
        VoiceChatStatus::Connecting => "Connecting",
        VoiceChatStatus::Live => "Voice Chat",
        VoiceChatStatus::Ended => "Call Ended",
        VoiceChatStatus::Failed => "Call Failed",
    }
}

fn duration_label(status: VoiceChatStatus, started_at: Option<Instant>) -> String {
    if status != VoiceChatStatus::Live {
        return "--:--".to_string();
    }
    let elapsed = started_at
        .map(|started_at| started_at.elapsed())
        .unwrap_or_default()
        .as_secs();
    format!("{:02}:{:02}", elapsed / 60, elapsed % 60)
}

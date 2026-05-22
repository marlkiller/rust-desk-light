use super::{COLOR_BAD, COLOR_GOOD, COLOR_WARN};
use crate::i18n::t;
use eframe::egui;
use rdl_protocol::ClientInfo;
use std::time::Instant;

#[derive(Clone)]
pub(crate) struct ClientRow {
    pub(crate) info: ClientInfo,
    pub(crate) status: ClientStatus,
}

pub(super) struct ClientOnlineToast {
    pub(super) title: String,
    pub(super) detail: String,
    pub(super) created_at: Instant,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ClientStatus {
    Online,
    Stale,
    Offline,
}

impl ClientStatus {
    pub(super) fn can_receive_commands(self) -> bool {
        matches!(self, ClientStatus::Online)
    }
}

pub(super) fn client_status_text(ui: &mut egui::Ui, status: ClientStatus) {
    let (text, color) = client_status_display(status);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 3.5, color);
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(12.0).color(color).strong())
                .selectable(false)
                .sense(egui::Sense::hover()),
        );
    });
}

pub(crate) fn client_status_display(status: ClientStatus) -> (&'static str, egui::Color32) {
    match status {
        ClientStatus::Online => (t("Online"), COLOR_GOOD),
        ClientStatus::Stale => (t("Stale"), COLOR_WARN),
        ClientStatus::Offline => (t("Offline"), COLOR_BAD),
    }
}

pub(super) fn client_commands_disabled_text(status: ClientStatus) -> &'static str {
    match status {
        ClientStatus::Online => "",
        ClientStatus::Stale => t("Client stale - commands disabled"),
        ClientStatus::Offline => t("Client offline - commands disabled"),
    }
}

pub(super) fn client_mode_label(gui_mode: bool) -> &'static str {
    if gui_mode {
        t("GUI")
    } else {
        t("Terminal")
    }
}

pub(super) fn client_mode_search_tokens(gui_mode: bool) -> &'static str {
    if gui_mode {
        "gui graphical ui"
    } else {
        "terminal cli no ui"
    }
}

pub(super) fn client_location_label(client: &ClientInfo) -> String {
    client
        .location
        .as_ref()
        .map(|location| {
            let label = location.label.trim();
            if label.is_empty() {
                format!("{:.2}, {:.2}", location.latitude(), location.longitude())
            } else {
                label.to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn client_identity_label(client: &ClientInfo) -> String {
    match (client.hostname.trim(), client.username.trim()) {
        ("", "") => client.id.clone(),
        (hostname, "") => hostname.to_string(),
        ("", username) => username.to_string(),
        (hostname, username) => format!("{hostname} / {username}"),
    }
}

pub(super) fn client_online_notice(client: &ClientInfo) -> (String, String) {
    let title = format!("{} is online", client_identity_label(client));
    let detail = if client.peer_addr.trim().is_empty() {
        client.id.clone()
    } else {
        format!("{} - {}", client.id, client.peer_addr)
    };
    (title, detail)
}

pub(super) fn client_os_label(os: &str) -> String {
    let os = os.trim();
    if os.is_empty() {
        "💻 Unknown".to_string()
    } else {
        format!("{} {os}", client_os_emoji(os))
    }
}

fn client_os_emoji(os: &str) -> &'static str {
    let os = os.to_ascii_lowercase();
    if os.contains("android") {
        "🤖"
    } else if os.contains("iphone") || os.contains("ipad") || os.contains("ios") {
        "📱"
    } else if os.contains("macos") || os.contains("darwin") || os.contains("os x") {
        "🍎"
    } else if os.contains("windows") || os.starts_with("win") {
        "💻"
    } else if os.contains("linux")
        || os.contains("ubuntu")
        || os.contains("debian")
        || os.contains("fedora")
        || os.contains("centos")
        || os.contains("red hat")
        || os.contains("arch")
        || os.contains("alpine")
        || os.contains("nixos")
        || os.contains("mint")
    {
        "🐧"
    } else {
        "💻"
    }
}

#[cfg(feature = "desktop-interaction")]
pub(crate) mod balloon_tip;
#[cfg(feature = "desktop-interaction")]
pub(crate) mod message_box;
#[cfg(feature = "desktop-interaction")]
pub(crate) mod open_text_in_notepad;
#[cfg(feature = "desktop-interaction")]
mod payload;
#[cfg(feature = "desktop-interaction")]
mod platform;
#[cfg(feature = "gui")]
pub(crate) mod text_chat;
#[cfg(feature = "gui")]
pub(crate) mod voice_chat;

use rdl_protocol::CommandKind;

pub(crate) fn handle(command: &CommandKind, payload: &str, gui_mode: bool) -> String {
    match command {
        CommandKind::TextChat => handle_text_chat(gui_mode),
        CommandKind::MessageBox => handle_message_box(payload),
        CommandKind::BalloonTip => handle_balloon_tip(payload),
        CommandKind::VoiceChat => handle_voice_chat(payload, gui_mode),
        CommandKind::OpenTextInNotepad => handle_open_text_in_notepad(payload),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

#[cfg(feature = "gui")]
fn handle_text_chat(gui_mode: bool) -> String {
    text_chat::handle(gui_mode)
}

#[cfg(not(feature = "gui"))]
fn handle_text_chat(_gui_mode: bool) -> String {
    disabled_detail(&CommandKind::TextChat)
}

#[cfg(feature = "gui")]
fn handle_voice_chat(payload: &str, gui_mode: bool) -> String {
    voice_chat::handle(payload, gui_mode)
}

#[cfg(not(feature = "gui"))]
fn handle_voice_chat(_payload: &str, _gui_mode: bool) -> String {
    disabled_detail(&CommandKind::VoiceChat)
}

#[cfg(feature = "desktop-interaction")]
fn handle_message_box(payload: &str) -> String {
    message_box::handle(payload)
}

#[cfg(not(feature = "desktop-interaction"))]
fn handle_message_box(_payload: &str) -> String {
    disabled_detail(&CommandKind::MessageBox)
}

#[cfg(feature = "desktop-interaction")]
fn handle_balloon_tip(payload: &str) -> String {
    balloon_tip::handle(payload)
}

#[cfg(not(feature = "desktop-interaction"))]
fn handle_balloon_tip(_payload: &str) -> String {
    disabled_detail(&CommandKind::BalloonTip)
}

#[cfg(feature = "desktop-interaction")]
fn handle_open_text_in_notepad(payload: &str) -> String {
    open_text_in_notepad::handle(payload)
}

#[cfg(not(feature = "desktop-interaction"))]
fn handle_open_text_in_notepad(_payload: &str) -> String {
    disabled_detail(&CommandKind::OpenTextInNotepad)
}

pub(crate) fn command_available(command: &CommandKind, client_ui_available: bool) -> bool {
    match command {
        CommandKind::TextChat | CommandKind::VoiceChat => {
            client_ui_available && cfg!(feature = "gui")
        }
        CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::OpenTextInNotepad => {
            cfg!(feature = "desktop-interaction")
        }
        _ => true,
    }
}

pub(crate) fn disabled_detail(command: &CommandKind) -> String {
    let message = match command {
        CommandKind::TextChat | CommandKind::VoiceChat => "client UI is not available",
        CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::OpenTextInNotepad => {
            "client desktop interaction is not available"
        }
        _ => "client GUI is not available",
    };
    format!("{}_disabled\nmessage={message}", command.as_str())
}

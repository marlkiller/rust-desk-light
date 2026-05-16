use rdl_protocol::CommandKind;

pub(crate) fn handle(payload: &str, gui_mode: bool) -> String {
    if !gui_mode {
        return super::disabled_detail(&CommandKind::VoiceChat);
    }

    format!(
        "TODO: {} accepted as planned stub; payload='{}'",
        CommandKind::VoiceChat.as_str(),
        payload
    )
}

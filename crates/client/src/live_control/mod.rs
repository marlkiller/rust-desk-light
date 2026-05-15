use rdl_protocol::CommandKind;

mod remote_desktop;

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::RemoteDesktop => remote_desktop::handle(payload),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

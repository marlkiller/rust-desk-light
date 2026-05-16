use rdl_protocol::CommandKind;

pub struct CommandReply {
    pub accepted: bool,
    pub detail: String,
}

impl CommandReply {
    fn accepted(detail: String) -> Self {
        Self {
            accepted: true,
            detail,
        }
    }

    fn rejected(detail: String) -> Self {
        Self {
            accepted: false,
            detail,
        }
    }
}

pub fn handle_command(command: &CommandKind, payload: &str, gui_mode: bool) -> CommandReply {
    if command.requires_client_gui() && !gui_mode {
        return CommandReply::rejected(gui_disabled_detail(command));
    }

    CommandReply::accepted(match command {
        CommandKind::UpdateClient
        | CommandKind::UninstallClient
        | CommandKind::KillClientProcess
        | CommandKind::Shutdown
        | CommandKind::Reboot
        | CommandKind::DeleteClient => crate::session::handle(command, payload),
        CommandKind::ComputerInfo | CommandKind::Clipboard | CommandKind::Proxy => {
            crate::system_info::handle(command, payload)
        }
        CommandKind::FileManager
        | CommandKind::RemoteTerminal
        | CommandKind::ProcessManager
        | CommandKind::WindowManager
        | CommandKind::StartupManager
        | CommandKind::RegistryManager
        | CommandKind::DriverManager
        | CommandKind::EventLog
        | CommandKind::ActiveConnections
        | CommandKind::PerformanceMonitor
        | CommandKind::KillTargetProcess => crate::remote_management::handle(command, payload),
        CommandKind::RemoteDesktop | CommandKind::Camera | CommandKind::AudioListen => {
            crate::live_control::handle(command, payload)
        }
        CommandKind::MessageBox
        | CommandKind::BalloonTip
        | CommandKind::TextChat
        | CommandKind::VoiceChat
        | CommandKind::OpenTextInNotepad => {
            crate::user_interaction::handle(command, payload, gui_mode)
        }
        CommandKind::ExecuteFile
        | CommandKind::ExecuteCode
        | CommandKind::ExecuteStaticCommand
        | CommandKind::CreateTask
        | CommandKind::CommandPreset => crate::execute::handle(command, payload),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    })
}

pub(crate) fn gui_disabled_detail(command: &CommandKind) -> String {
    match command {
        CommandKind::RemoteDesktop | CommandKind::Camera | CommandKind::AudioListen => {
            crate::live_control::disabled_detail(command)
        }
        CommandKind::MessageBox
        | CommandKind::BalloonTip
        | CommandKind::TextChat
        | CommandKind::VoiceChat
        | CommandKind::OpenTextInNotepad => crate::user_interaction::disabled_detail(command),
        _ => format!(
            "{}_disabled\nmessage=client GUI is not available",
            command.as_str()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_gui_rejects_user_interaction_commands() {
        let reply = handle_command(&CommandKind::MessageBox, "title=x\nmessage=y", false);

        assert!(!reply.accepted);
        assert_eq!(
            reply.detail,
            "message_box_disabled\nmessage=client GUI is not available"
        );
    }

    #[test]
    fn no_gui_rejects_live_control_commands_with_parseable_detail() {
        let reply = handle_command(&CommandKind::RemoteDesktop, "action=screens", false);

        assert!(!reply.accepted);
        assert_eq!(
            reply.detail,
            "remote_desktop_error\nmessage=client GUI is not available"
        );
    }

    #[test]
    fn gui_mode_still_allows_user_interaction_commands() {
        let reply = handle_command(&CommandKind::TextChat, "", true);

        assert!(reply.accepted);
        assert_eq!(reply.detail, "chat_delivered");
    }
}

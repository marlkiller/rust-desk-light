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
    if let Some(detail) = unavailable_detail(command, gui_mode) {
        return CommandReply::rejected(detail);
    }

    CommandReply::accepted(match command {
        CommandKind::UpdateClient
        | CommandKind::UninstallClient
        | CommandKind::KillClientProcess
        | CommandKind::Shutdown
        | CommandKind::Reboot
        | CommandKind::DeleteClient => crate::session::handle(command, payload),
        CommandKind::ComputerInfo | CommandKind::Clipboard => {
            crate::system_info::handle(command, payload)
        }
        CommandKind::FileManager
        | CommandKind::RemoteTerminal
        | CommandKind::ProcessManager
        | CommandKind::WindowManager
        | CommandKind::StartupManager
        | CommandKind::ServiceManager
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
        | CommandKind::CreateTask => crate::execute::handle(command, payload),
        CommandKind::CommandPreset => {
            return CommandReply::rejected(
                "command_preset\nstatus=unsupported\nmessage=command presets were removed"
                    .to_string(),
            )
        }
        // Interactive stream commands, including reverse proxy, use dedicated Message variants.
        _ => return CommandReply::rejected(unsupported_command_detail(command)),
    })
}

fn unavailable_detail(command: &CommandKind, client_ui_available: bool) -> Option<String> {
    if matches!(command, CommandKind::RemoteDesktop | CommandKind::Camera)
        && !crate::live_control::command_available(command)
    {
        return Some(crate::live_control::disabled_detail(command));
    }
    if matches!(command, CommandKind::AudioListen)
        && !crate::live_control::command_available(command)
    {
        return Some(crate::live_control::disabled_detail(command));
    }
    if matches!(
        command,
        CommandKind::MessageBox
            | CommandKind::BalloonTip
            | CommandKind::TextChat
            | CommandKind::VoiceChat
            | CommandKind::OpenTextInNotepad
    ) && !crate::user_interaction::command_available(command, client_ui_available)
    {
        return Some(crate::user_interaction::disabled_detail(command));
    }
    None
}

fn unsupported_command_detail(command: &CommandKind) -> String {
    format!(
        "{}_error\nstatus=unsupported\nmessage=command is not supported by this client",
        command.as_str()
    )
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

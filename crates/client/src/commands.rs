use rdl_protocol::CommandKind;

pub fn handle_command(command: &CommandKind, payload: &str, gui_mode: bool) -> String {
    match command {
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
        CommandKind::RemoteDesktop | CommandKind::Camera => {
            crate::live_control::handle(command, payload)
        }
        CommandKind::MessageBox
        | CommandKind::BalloonTip
        | CommandKind::TextChat
        | CommandKind::VoiceChat
        | CommandKind::OpenTextInNotepad => {
            crate::user_interaction::commands::handle(command, payload, gui_mode)
        }
        CommandKind::ExecuteFile | CommandKind::ExecuteCode | CommandKind::ExecuteStaticCommand => {
            crate::execute::handle(command, payload)
        }
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

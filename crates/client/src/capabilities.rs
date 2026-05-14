mod remote_management;
mod support;
mod system_info;
mod user_interaction;

use rdl_protocol::CommandKind;

pub fn handle_command(command: &CommandKind, payload: &str, gui_mode: bool) -> String {
    match command {
        CommandKind::ComputerInfo | CommandKind::Clipboard | CommandKind::Proxy => {
            system_info::handle(command, payload)
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
        | CommandKind::KillTargetProcess => remote_management::handle(command, payload),
        CommandKind::MessageBox
        | CommandKind::BalloonTip
        | CommandKind::TextChat
        | CommandKind::VoiceChat
        | CommandKind::OpenTextInNotepad => user_interaction::handle(command, payload, gui_mode),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

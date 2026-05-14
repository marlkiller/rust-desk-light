use eframe::egui;
use rdl_protocol::CommandKind;

pub fn render_context_menu(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    render_session(ui, client_id, send_command);
    render_remote_management(ui, client_id, send_command);
    render_live_control(ui, client_id, send_command);
    render_user_interaction(ui, client_id, send_command);
    render_system_info(ui, client_id, send_command);
    render_execute(ui, client_id, send_command);
    render_plugins(ui, client_id, send_command);
}

fn render_session(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button("Session", |ui| {
        menu_command(
            ui,
            client_id,
            "Update Client",
            CommandKind::UpdateClient,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Uninstall Client",
            CommandKind::UninstallClient,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Kill Client Process",
            CommandKind::KillClientProcess,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Shutdown",
            CommandKind::Shutdown,
            send_command,
        );
        menu_command(ui, client_id, "Reboot", CommandKind::Reboot, send_command);
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Move To Group",
            CommandKind::MoveToGroup,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Clone Client Settings",
            CommandKind::CloneClientSettings,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Delete Client",
            CommandKind::DeleteClient,
            send_command,
        );
    });
}

fn render_remote_management(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button("Remote Management", |ui| {
        menu_command(
            ui,
            client_id,
            "File Manager",
            CommandKind::FileManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Remote Terminal",
            CommandKind::RemoteTerminal,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Process Manager",
            CommandKind::ProcessManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Window Manager",
            CommandKind::WindowManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Startup Manager",
            CommandKind::StartupManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Registry Manager",
            CommandKind::RegistryManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Driver Manager",
            CommandKind::DriverManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Event Log",
            CommandKind::EventLog,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Active Connections",
            CommandKind::ActiveConnections,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Performance Monitor",
            CommandKind::PerformanceMonitor,
            send_command,
        );
    });
}

fn render_live_control(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button("Live Control", |ui| {
        menu_command(
            ui,
            client_id,
            "Remote Desktop",
            CommandKind::RemoteDesktop,
            send_command,
        );
        ui.separator();
        menu_command(ui, client_id, "Camera", CommandKind::Camera, send_command);
        menu_command(
            ui,
            client_id,
            "Audio Listen",
            CommandKind::AudioListen,
            send_command,
        );
    });
}

fn render_user_interaction(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button("User Interaction", |ui| {
        menu_command(
            ui,
            client_id,
            "Message Box",
            CommandKind::MessageBox,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Balloon Tip",
            CommandKind::BalloonTip,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Text Chat",
            CommandKind::TextChat,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Voice Chat",
            CommandKind::VoiceChat,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Open Text In Notepad",
            CommandKind::OpenTextInNotepad,
            send_command,
        );
    });
}

fn render_system_info(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button("System Info", |ui| {
        menu_command(
            ui,
            client_id,
            "Computer Info",
            CommandKind::ComputerInfo,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Clipboard",
            CommandKind::Clipboard,
            send_command,
        );
        ui.separator();
        menu_command(ui, client_id, "Proxy", CommandKind::Proxy, send_command);
    });
}

fn render_execute(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button("Execute", |ui| {
        menu_command(
            ui,
            client_id,
            "Execute File",
            CommandKind::ExecuteFile,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Execute Code",
            CommandKind::ExecuteCode,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Execute Static Command",
            CommandKind::ExecuteStaticCommand,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Create Task",
            CommandKind::CreateTask,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Command Preset",
            CommandKind::CommandPreset,
            send_command,
        );
    });
}

fn render_plugins(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button("Plugins", |ui| {
        menu_command(
            ui,
            client_id,
            "Plugin Manager",
            CommandKind::PluginManager,
            send_command,
        );
    });
}

fn menu_command(
    ui: &mut egui::Ui,
    client_id: &str,
    label: &str,
    command: CommandKind,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    let label = if command_is_implemented(&command) {
        label.to_string()
    } else {
        format!("{label} (TODO)")
    };
    if ui.button(label).clicked() {
        send_command(client_id, command);
        ui.close();
    }
}

fn command_is_implemented(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::ComputerInfo
            | CommandKind::Clipboard
            | CommandKind::ProcessManager
            | CommandKind::EventLog
            | CommandKind::ActiveConnections
            | CommandKind::PerformanceMonitor
            | CommandKind::TextChat
    )
}

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
            "Client / Update Client",
            CommandKind::UpdateClient,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Client / Uninstall Client",
            CommandKind::UninstallClient,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Client / Kill Client Process",
            CommandKind::KillClientProcess,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Power / Shutdown",
            CommandKind::Shutdown,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Power / Reboot",
            CommandKind::Reboot,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Management / Move To Group",
            CommandKind::MoveToGroup,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Management / Clone Client Settings",
            CommandKind::CloneClientSettings,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Management / Delete Client",
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
            "Files / File Manager",
            CommandKind::FileManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Files / Remote Terminal",
            CommandKind::RemoteTerminal,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Tools / Process Manager",
            CommandKind::ProcessManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Tools / Window Manager",
            CommandKind::WindowManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Tools / Startup Manager",
            CommandKind::StartupManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Tools / Registry Manager",
            CommandKind::RegistryManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Tools / Driver Manager",
            CommandKind::DriverManager,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Tools / Event Log",
            CommandKind::EventLog,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Monitoring / Active Connections",
            CommandKind::ActiveConnections,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Monitoring / Performance Monitor",
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
            "Desktop / Remote Desktop",
            CommandKind::RemoteDesktop,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Media / Camera",
            CommandKind::Camera,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Media / Audio Listen",
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
            "Prompts / Message Box",
            CommandKind::MessageBox,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Prompts / Balloon Tip",
            CommandKind::BalloonTip,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Communication / Text Chat",
            CommandKind::TextChat,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Communication / Voice Chat",
            CommandKind::VoiceChat,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Text / Open Text In Notepad",
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
            "Basics / Computer Info",
            CommandKind::ComputerInfo,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Basics / Clipboard",
            CommandKind::Clipboard,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Network / Proxy",
            CommandKind::Proxy,
            send_command,
        );
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
            "Files / Execute File",
            CommandKind::ExecuteFile,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Code / Execute Code",
            CommandKind::ExecuteCode,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Tasks / Execute Static Command",
            CommandKind::ExecuteStaticCommand,
            send_command,
        );
        menu_command(
            ui,
            client_id,
            "Tasks / Create Task",
            CommandKind::CreateTask,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Automation / Command Preset",
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
            "Extensions / Plugin Manager",
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

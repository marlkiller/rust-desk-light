use crate::i18n::t;
use eframe::egui;
use rdl_protocol::CommandKind;

const CONTEXT_MENU_MIN_WIDTH: f32 = 220.0;
const SUBMENU_MIN_WIDTH: f32 = 240.0;

pub fn render_context_menu(
    ui: &mut egui::Ui,
    client_id: &str,
    gui_available: bool,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    prepare_menu_ui(ui, CONTEXT_MENU_MIN_WIDTH);
    render_session(ui, client_id, send_command);
    render_remote_management(ui, client_id, send_command);
    render_live_control(ui, client_id, gui_available, send_command);
    render_user_interaction(ui, client_id, gui_available, send_command);
    render_system_info(ui, client_id, send_command);
    render_execute(ui, client_id, send_command);
    render_plugins(ui, client_id, send_command);
}

pub fn render_toolbar_actions(
    ui: &mut egui::Ui,
    client_id: &str,
    gui_available: bool,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.label(crate::theme::muted_text(t("Quick")).strong());
    toolbar_command(
        ui,
        client_id,
        "Remote Desktop",
        CommandKind::RemoteDesktop,
        gui_available,
        "Disabled: selected client has no GUI session",
        send_command,
    );
    toolbar_command(
        ui,
        client_id,
        "Files",
        CommandKind::FileManager,
        true,
        "",
        send_command,
    );
    toolbar_command(
        ui,
        client_id,
        "Terminal",
        CommandKind::RemoteTerminal,
        true,
        "",
        send_command,
    );
    toolbar_command(
        ui,
        client_id,
        "Execute Code",
        CommandKind::ExecuteCode,
        true,
        "",
        send_command,
    );
}

pub fn render_unavailable_client_menu(ui: &mut egui::Ui, client_id: &str, status: &str) {
    prepare_menu_ui(ui, CONTEXT_MENU_MIN_WIDTH);
    ui.add(
        egui::Label::new(egui::RichText::new(format!("{} {status}", t("Client"))).strong())
            .wrap_mode(egui::TextWrapMode::Extend),
    );
    ui.add(
        egui::Label::new(t(
            "Remote commands are disabled until this client reconnects.",
        ))
        .wrap_mode(egui::TextWrapMode::Extend),
    );
    ui.separator();
    if ui.add(menu_button(t("Copy Client ID"))).clicked() {
        ui.ctx().copy_text(client_id.to_string());
        ui.close();
    }
}

fn render_session(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button(menu_title("🔐", "Session"), |ui| {
        prepare_menu_ui(ui, SUBMENU_MIN_WIDTH);
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
            "Client Config",
            CommandKind::ClientConfig,
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
    ui.menu_button(menu_title("🛠", "Remote Management"), |ui| {
        prepare_menu_ui(ui, SUBMENU_MIN_WIDTH);
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
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Reverse Proxy",
            CommandKind::Proxy,
            send_command,
        );
    });
}

fn render_live_control(
    ui: &mut egui::Ui,
    client_id: &str,
    gui_available: bool,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    let response = ui
        .add_enabled_ui(gui_available, |ui| {
            ui.menu_button(menu_title("📡", "Live Control"), |ui| {
                prepare_menu_ui(ui, SUBMENU_MIN_WIDTH);
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
        })
        .response;
    if !gui_available {
        response.on_hover_text(t("Disabled: selected client has no GUI session"));
    }
}

fn render_user_interaction(
    ui: &mut egui::Ui,
    client_id: &str,
    gui_available: bool,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    let response = ui
        .add_enabled_ui(gui_available, |ui| {
            ui.menu_button(menu_title("💬", "User Interaction"), |ui| {
                prepare_menu_ui(ui, SUBMENU_MIN_WIDTH);
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
        })
        .response;
    if !gui_available {
        response.on_hover_text(t("Disabled: selected client has no GUI session"));
    }
}

fn render_system_info(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button(menu_title("ℹ", "System Info"), |ui| {
        prepare_menu_ui(ui, SUBMENU_MIN_WIDTH);
        menu_command(
            ui,
            client_id,
            "Computer Info",
            CommandKind::ComputerInfo,
            send_command,
        );
        ui.separator();
        menu_command(
            ui,
            client_id,
            "Clipboard",
            CommandKind::Clipboard,
            send_command,
        );
    });
}

fn render_execute(
    ui: &mut egui::Ui,
    client_id: &str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    ui.menu_button(menu_title("▶", "Execute"), |ui| {
        prepare_menu_ui(ui, SUBMENU_MIN_WIDTH);
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
    ui.menu_button(menu_title("🔌", "Plugins"), |ui| {
        prepare_menu_ui(ui, SUBMENU_MIN_WIDTH);
        menu_command(
            ui,
            client_id,
            "Plugin Manager",
            CommandKind::PluginManager,
            send_command,
        );
    });
}

fn toolbar_command(
    ui: &mut egui::Ui,
    client_id: &str,
    label: &'static str,
    command: CommandKind,
    enabled: bool,
    disabled_hover: &'static str,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(format!("{} {}", command_icon(&command), t(label))),
    );
    if response.clicked() {
        send_command(client_id, command);
    }
    if !enabled && !disabled_hover.is_empty() {
        response.on_hover_text(t(disabled_hover));
    }
}

fn menu_command(
    ui: &mut egui::Ui,
    client_id: &str,
    label: &'static str,
    command: CommandKind,
    send_command: &mut impl FnMut(&str, CommandKind),
) {
    let icon = command_icon(&command);
    let label = t(label);
    let label = if command_is_implemented(&command) {
        label.to_string()
    } else {
        format!("{label} (TODO)")
    };
    if ui.add(menu_button(format!("{icon} {label}"))).clicked() {
        send_command(client_id, command);
        ui.close();
    }
}

fn prepare_menu_ui(ui: &mut egui::Ui, min_width: f32) {
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    ui.set_min_width(min_width);
}

fn menu_button(label: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(label)
        .wrap_mode(egui::TextWrapMode::Extend)
        .min_size(egui::vec2(SUBMENU_MIN_WIDTH, 0.0))
}

fn menu_title(icon: &str, label: &'static str) -> String {
    format!("{icon} {}", t(label))
}

fn command_icon(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::UpdateClient => "⬆",
        CommandKind::UninstallClient => "🗑",
        CommandKind::KillClientProcess | CommandKind::KillTargetProcess => "✖",
        CommandKind::Shutdown => "🔴",
        CommandKind::Reboot => "↻",
        CommandKind::MoveToGroup => "📦",
        CommandKind::CloneClientSettings => "📋",
        CommandKind::DeleteClient => "🗑",
        CommandKind::ClientConfig => "⚙",
        CommandKind::FileManager => "📁",
        CommandKind::RemoteTerminal => "⌨",
        CommandKind::ProcessManager => "⚙",
        CommandKind::WindowManager => "▣",
        CommandKind::StartupManager => "🚀",
        CommandKind::RegistryManager => "📚",
        CommandKind::DriverManager => "🔌",
        CommandKind::EventLog => "📄",
        CommandKind::ActiveConnections => "🔗",
        CommandKind::PerformanceMonitor => "📈",
        CommandKind::RemoteDesktop => "💻",
        CommandKind::Camera => "📷",
        CommandKind::AudioListen => "🎧",
        CommandKind::MessageBox => "💬",
        CommandKind::BalloonTip => "🔔",
        CommandKind::TextChat => "💬",
        CommandKind::VoiceChat => "🎤",
        CommandKind::OpenTextInNotepad => "📝",
        CommandKind::ComputerInfo => "💻",
        CommandKind::Clipboard => "📋",
        CommandKind::Proxy => "🌐",
        CommandKind::ExecuteFile => "📄",
        CommandKind::ExecuteCode => "💻",
        CommandKind::ExecuteStaticCommand => "▶",
        CommandKind::CreateTask => "⏱",
        CommandKind::CommandPreset => "★",
        CommandKind::PluginManager => "🔌",
    }
}

fn command_is_implemented(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::UpdateClient
            | CommandKind::UninstallClient
            | CommandKind::KillClientProcess
            | CommandKind::Shutdown
            | CommandKind::Reboot
            | CommandKind::ClientConfig
            | CommandKind::DeleteClient
            | CommandKind::ComputerInfo
            | CommandKind::Clipboard
            | CommandKind::FileManager
            | CommandKind::ProcessManager
            | CommandKind::WindowManager
            | CommandKind::StartupManager
            | CommandKind::RegistryManager
            | CommandKind::DriverManager
            | CommandKind::RemoteDesktop
            | CommandKind::Camera
            | CommandKind::AudioListen
            | CommandKind::MessageBox
            | CommandKind::BalloonTip
            | CommandKind::RemoteTerminal
            | CommandKind::EventLog
            | CommandKind::ActiveConnections
            | CommandKind::PerformanceMonitor
            | CommandKind::Proxy
            | CommandKind::TextChat
            | CommandKind::VoiceChat
            | CommandKind::OpenTextInNotepad
            | CommandKind::ExecuteFile
            | CommandKind::ExecuteCode
            | CommandKind::ExecuteStaticCommand
    )
}

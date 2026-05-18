use crate::runtime::Config;
use eframe::egui;

use super::{form_label, COLOR_BAD, COLOR_GOOD, COLOR_MUTED, COLOR_TEXT, TOOLBAR_CONTROL_HEIGHT};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsTheme {
    Light,
    Dark,
}

impl SettingsTheme {
    const ALL: [Self; 2] = [Self::Light, Self::Dark];

    fn as_config(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsLanguage {
    English,
    ChineseSimplified,
}

impl SettingsLanguage {
    const ALL: [Self; 2] = [Self::English, Self::ChineseSimplified];

    fn as_config(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::ChineseSimplified => "zh-CN",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::ChineseSimplified => "Chinese (Simplified)",
        }
    }
}

pub(super) struct SettingsState {
    pub(super) server_ip: String,
    pub(super) server_port: String,
    pub(super) auth_token: String,
    theme: SettingsTheme,
    language: SettingsLanguage,
    open: bool,
    reconnect_pending: bool,
    notice: String,
    error: String,
}

impl SettingsState {
    pub(super) fn new(config: &Config) -> Self {
        Self {
            server_ip: config.ip.clone(),
            server_port: config.port.to_string(),
            auth_token: config.auth_token.clone(),
            theme: SettingsTheme::Light,
            language: SettingsLanguage::English,
            open: false,
            reconnect_pending: false,
            notice: String::new(),
            error: String::new(),
        }
    }

    pub(super) fn open(&mut self) {
        self.open = true;
    }

    pub(super) fn open_with_connection_error(
        &mut self,
        ip: String,
        port: u16,
        token: String,
        error: impl Into<String>,
    ) {
        self.server_ip = ip;
        self.server_port = port.to_string();
        self.auth_token = token;
        self.open = true;
        self.finish_reconnect_error_or_set(error);
    }

    pub(super) fn sync_connection(&mut self, config: &Config) {
        self.server_ip = config.ip.clone();
        self.server_port = config.port.to_string();
        self.auth_token = config.auth_token.clone();
    }

    pub(super) fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = notice.into();
        self.error.clear();
    }

    pub(super) fn set_error(&mut self, error: impl Into<String>) {
        self.error = error.into();
        self.notice.clear();
    }

    pub(super) fn set_reconnect_pending(&mut self) {
        self.reconnect_pending = true;
        self.set_notice("Connection saved. Reconnecting...");
    }

    pub(super) fn finish_reconnect_success(&mut self) {
        if self.reconnect_pending {
            self.reconnect_pending = false;
            self.set_notice("Connection saved. Connected.");
        }
    }

    fn finish_reconnect_error_or_set(&mut self, detail: impl Into<String>) {
        let detail = detail.into();
        if self.reconnect_pending {
            self.reconnect_pending = false;
            self.set_error(format!("Reconnect failed: {detail}"));
        } else {
            self.set_error(detail);
        }
    }
}

pub(super) enum SettingsAction {
    SaveConnection {
        ip: String,
        port: String,
        token: String,
    },
    SavePreferences {
        theme: String,
        language: String,
    },
}

pub(super) fn render_settings_window(
    ctx: &egui::Context,
    state: &mut SettingsState,
) -> Option<SettingsAction> {
    if !state.open {
        return None;
    }

    let mut action = None;
    let mut open = state.open;
    egui::Window::new("Setting")
        .id(egui::Id::new("admin_settings_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.set_min_width(380.0);
            render_connection_settings(ui, state, &mut action);
            ui.separator();
            render_preference_settings(ui, state, &mut action);
            render_status(ui, state);
        });
    state.open = open;

    action
}

fn render_connection_settings(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    action: &mut Option<SettingsAction>,
) {
    ui.label(
        egui::RichText::new("Connection")
            .size(13.0)
            .color(COLOR_TEXT)
            .strong(),
    );
    ui.add_space(4.0);

    form_label(ui, "Server IP");
    ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut state.server_ip)
            .hint_text("127.0.0.1")
            .vertical_align(egui::Align::Center),
    );
    ui.add_space(6.0);

    form_label(ui, "Server Port");
    ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut state.server_port)
            .hint_text("5169")
            .vertical_align(egui::Align::Center),
    );
    ui.add_space(6.0);

    form_label(ui, "Token");
    ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut state.auth_token)
            .password(true)
            .hint_text("Auth token")
            .vertical_align(egui::Align::Center),
    );

    ui.add_space(8.0);
    if ui
        .add_sized(
            [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
            egui::Button::new("Save connection and reconnect"),
        )
        .clicked()
    {
        *action = Some(SettingsAction::SaveConnection {
            ip: state.server_ip.clone(),
            port: state.server_port.clone(),
            token: state.auth_token.clone(),
        });
    }
}

fn render_preference_settings(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    action: &mut Option<SettingsAction>,
) {
    ui.label(
        egui::RichText::new("Appearance")
            .size(13.0)
            .color(COLOR_TEXT)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "TODO: Saved to config only; runtime theme/i18n wiring is on the roadmap.",
        )
        .size(12.0)
        .color(COLOR_MUTED),
    );
    ui.add_space(6.0);

    form_label(ui, "Theme");
    egui::ComboBox::from_id_salt("admin_settings_theme_todo")
        .selected_text(state.theme.label())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for theme in SettingsTheme::ALL {
                ui.selectable_value(&mut state.theme, theme, theme.label());
            }
        });
    ui.add_space(6.0);

    form_label(ui, "Language");
    egui::ComboBox::from_id_salt("admin_settings_language_todo")
        .selected_text(state.language.label())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for language in SettingsLanguage::ALL {
                ui.selectable_value(&mut state.language, language, language.label());
            }
        });

    ui.add_space(8.0);
    if ui
        .add_sized(
            [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
            egui::Button::new("Save theme and language"),
        )
        .clicked()
    {
        *action = Some(SettingsAction::SavePreferences {
            theme: state.theme.as_config().to_string(),
            language: state.language.as_config().to_string(),
        });
    }
}

fn render_status(ui: &mut egui::Ui, state: &SettingsState) {
    if !state.error.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(&state.error)
                .size(12.0)
                .color(COLOR_BAD),
        );
    } else if !state.notice.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(&state.notice)
                .size(12.0)
                .color(COLOR_GOOD),
        );
    } else {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Connection settings are saved to the admin config.")
                .size(12.0)
                .color(COLOR_MUTED),
        );
    }
}

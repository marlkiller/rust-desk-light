use crate::{
    i18n::{t, tf, theme_label, Language},
    runtime::Config,
    theme::ThemeKind,
};
use eframe::egui;

use super::{
    ui::{form_label, token_text_edit},
    color_bad, color_good, TOOLBAR_CONTROL_HEIGHT,
};

pub(super) struct SettingsState {
    pub(super) server_ip: String,
    pub(super) server_port: String,
    pub(super) auth_token: String,
    auth_token_visible: bool,
    theme: ThemeKind,
    language: Language,
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
            auth_token_visible: false,
            theme: ThemeKind::from_config(&config.theme),
            language: Language::from_config(&config.language),
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

    pub(super) fn sync_preferences(&mut self, config: &Config) {
        self.theme = ThemeKind::from_config(&config.theme);
        self.language = Language::from_config(&config.language);
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
        self.set_notice(t("Connection saved. Reconnecting..."));
    }

    pub(super) fn finish_reconnect_success(&mut self) {
        if self.reconnect_pending {
            self.reconnect_pending = false;
            self.set_notice(t("Connection saved. Connected."));
        }
    }

    fn finish_reconnect_error_or_set(&mut self, detail: impl Into<String>) {
        let detail = detail.into();
        if self.reconnect_pending {
            self.reconnect_pending = false;
            self.set_error(tf("Reconnect failed: {detail}", &[("detail", &detail)]));
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

pub(super) fn parse_connection_settings(
    ip: &str,
    port_text: &str,
    token: &str,
) -> Result<(String, u16, String), String> {
    let ip = ip.trim().to_string();
    let port_text = port_text.trim();
    let token = token.trim().to_string();

    if ip.is_empty() {
        return Err(t("Server IP cannot be empty.").to_string());
    }
    let port = match port_text.parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => return Err(t("Server port must be 1-65535.").to_string()),
    };
    if token.is_empty() {
        return Err(t("Token cannot be empty.").to_string());
    }

    Ok((ip, port, token))
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
    egui::Window::new(t("Setting"))
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
        egui::RichText::new(t("Connection"))
            .size(13.0)
            .color(crate::theme::palette().text)
            .strong(),
    );
    ui.add_space(4.0);

    form_label(ui, t("Server IP"));
    ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut state.server_ip)
            .hint_text("127.0.0.1")
            .vertical_align(egui::Align::Center),
    );
    ui.add_space(6.0);

    form_label(ui, t("Server Port"));
    ui.add_sized(
        [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
        egui::TextEdit::singleline(&mut state.server_port)
            .hint_text("5169")
            .vertical_align(egui::Align::Center),
    );
    ui.add_space(6.0);

    form_label(ui, t("Token"));
    token_text_edit(
        ui,
        &mut state.auth_token,
        &mut state.auth_token_visible,
        t("Auth token"),
    );

    ui.add_space(8.0);
    if ui
        .add_sized(
            [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
            egui::Button::new(t("Save connection and reconnect")),
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
        egui::RichText::new(t("Appearance"))
            .size(13.0)
            .color(crate::theme::palette().text)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(t(
            "Theme changes apply immediately. System follows the OS appearance.",
        ))
        .size(12.0)
        .color(crate::theme::palette().muted),
    );
    ui.add_space(6.0);

    form_label(ui, t("Theme"));
    egui::ComboBox::from_id_salt("admin_settings_theme")
        .selected_text(theme_label(state.theme))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for theme in ThemeKind::ALL {
                ui.selectable_value(&mut state.theme, theme, theme_label(theme));
            }
        });
    ui.add_space(6.0);

    form_label(ui, t("Language"));
    egui::ComboBox::from_id_salt("admin_settings_language")
        .selected_text(state.language.native_label())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for language in Language::ALL {
                ui.selectable_value(&mut state.language, language, language.native_label());
            }
        });

    ui.add_space(8.0);
    if ui
        .add_sized(
            [ui.available_width(), TOOLBAR_CONTROL_HEIGHT],
            egui::Button::new(t("Save")),
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
                .color(color_bad()),
        );
    } else if !state.notice.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(&state.notice)
                .size(12.0)
                .color(color_good()),
        );
    } else {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(t("Connection settings are saved to the admin config."))
                .size(12.0)
                .color(crate::theme::palette().muted),
        );
    }
}

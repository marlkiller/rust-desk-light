use super::ui::{
    token_text_edit, color_bad, color_good, color_muted, color_text, TOOLBAR_CONTROL_HEIGHT,
};
use crate::{
    client_binary::detect_binary_format,
    i18n::{self, t, Language},
    runtime::Config,
};
use chrono::{Local, TimeZone};
use eframe::egui;
use rdl_config::{ConfigKind, EndpointConfig};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const FORM_LABEL_WIDTH: f32 = 92.0;
const DETAILS_HEIGHT: f32 = 150.0;

pub(super) struct ClientBuilderState {
    template_path: String,
    output_path: String,
    server_ip: String,
    server_port: String,
    auth_token: String,
    auth_token_visible: bool,
    template_detail: String,
    build_status: BuildStatus,
    template_status: TemplateStatus,
    last_template_path: String,
    report_language: Language,
}

enum TemplateStatus {
    Unknown(String),
    Valid(String),
    Invalid(String),
}

enum BuildStatus {
    Idle,
    Success(String),
    Error(String),
}

impl TemplateStatus {
    fn is_valid(&self) -> bool {
        matches!(self, Self::Valid(_))
    }

    fn notice(&self) -> &str {
        match self {
            Self::Unknown(notice) | Self::Valid(notice) | Self::Invalid(notice) => notice,
        }
    }
}

impl ClientBuilderState {
    pub(super) fn new(config: &Config) -> Self {
        let template_path = default_template_path();
        let output_path = default_output_path(&template_path);
        let template_path = path_to_string(template_path);
        let mut state = Self {
            template_path,
            output_path: path_to_string(output_path),
            server_ip: config.ip.clone(),
            server_port: config.port.to_string(),
            auth_token: config.auth_token.clone(),
            auth_token_visible: false,
            template_detail: String::new(),
            build_status: BuildStatus::Idle,
            template_status: TemplateStatus::Unknown(t("Template not loaded").to_string()),
            last_template_path: String::new(),
            report_language: i18n::current_language(),
        };
        state.refresh_template_report();
        state
    }

    pub(super) fn render(
        &mut self,
        ctx: &egui::Context,
        open: &mut bool,
        admin_config: &Config,
    ) -> Option<String> {
        if !*open {
            return None;
        }

        let mut log_line = None;
        egui::Window::new(t("Client Builder"))
            .id(egui::Id::new("admin_client_builder_window"))
            .open(open)
            .default_width(620.0)
            .resizable(true)
            .show(ctx, |ui| {
                path_row(ui, t("Template"), &mut self.template_path, true);
                self.refresh_template_report_if_needed();
                ui.add_space(6.0);
                render_template_detail(ui, &self.template_detail, &self.template_status);
                ui.add_space(6.0);
                path_row(ui, t("Output"), &mut self.output_path, false);
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    form_label(ui, t("Server"));
                    ui.add_sized(
                        [240.0, TOOLBAR_CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut self.server_ip)
                            .hint_text(t("IP or host"))
                            .vertical_align(egui::Align::Center),
                    );
                    ui.add_sized(
                        [92.0, TOOLBAR_CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut self.server_port)
                            .hint_text(t("Port"))
                            .vertical_align(egui::Align::Center),
                    );
                    if ui.button(t("Use current")).clicked() {
                        self.server_ip = admin_config.ip.clone();
                        self.server_port = admin_config.port.to_string();
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    form_label(ui, t("Token"));
                    token_text_edit(
                        ui,
                        &mut self.auth_token,
                        &mut self.auth_token_visible,
                        t("Optional client auth token"),
                    );
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let generate_response = ui.add_enabled(
                        self.template_status.is_valid(),
                        egui::Button::new(t("Generate")),
                    );
                    let generate_clicked = generate_response.clicked();
                    if !self.template_status.is_valid() {
                        generate_response.on_hover_text(self.template_status.notice());
                    }
                    if ui.button(t("Reload Template")).clicked() {
                        self.build_status = BuildStatus::Idle;
                        self.refresh_template_report();
                    }
                    if generate_clicked {
                        match self.generate() {
                            Ok(message) => {
                                self.build_status = BuildStatus::Success(message.clone());
                                log_line = Some(message);
                            }
                            Err(message) => {
                                self.build_status = BuildStatus::Error(message);
                            }
                        }
                    }
                });

                ui.add_space(8.0);
                render_build_status_bar(ui, &self.build_status);
            });

        log_line
    }

    fn generate(&self) -> Result<String, String> {
        let template_text = self.template_path.trim().to_string();
        let output_text = self.output_path.trim().to_string();
        let ip = self.server_ip.trim().to_string();
        let port_text = self.server_port.trim().to_string();

        let template_path = PathBuf::from(&template_text);
        let output_path = PathBuf::from(&output_text);
        if template_path.as_os_str().is_empty() {
            return Err(t("Select a client template binary.").to_string());
        }
        if output_path.as_os_str().is_empty() {
            return Err(t("Select an output path.").to_string());
        }
        if ip.is_empty() {
            return Err(t("Server IP cannot be empty.").to_string());
        }
        let port = match port_text.parse::<u16>() {
            Ok(port) => port,
            Err(_) => return Err(t("Server port must be 1-65535.").to_string()),
        };

        let config_toml = rdl_config::client_embedded_config_toml(
            &EndpointConfig::new(&ip, port),
            optional_token(&self.auth_token),
        );

        let written = match rdl_config::write_embedded_endpoint_config(
            &template_path,
            &output_path,
            &config_toml,
        ) {
            Ok(written) => written,
            Err(error) => {
                return Err(error.to_string());
            }
        };

        let sign_detail = postprocess_generated_client(&output_path)?;

        Ok(format!(
            "payload={} bytes slot_offset={}{}",
            written.payload_bytes,
            written.slot_offset,
            sign_detail
                .as_deref()
                .map(|detail| format!(" {detail}"))
                .unwrap_or_default()
        ))
    }

    fn refresh_template_report_if_needed(&mut self) {
        let template_changed = self.template_path != self.last_template_path;
        let language_changed = self.report_language != i18n::current_language();
        if template_changed || language_changed {
            if template_changed {
                self.build_status = BuildStatus::Idle;
            }
            self.refresh_template_report();
        }
    }

    fn refresh_template_report(&mut self) {
        self.last_template_path = self.template_path.clone();
        self.report_language = i18n::current_language();
        let report = inspect_template(&self.template_path);
        self.template_detail = report.detail;
        self.template_status = report.status;
    }
}

fn path_row(ui: &mut egui::Ui, label: &str, value: &mut String, open_file: bool) {
    ui.horizontal(|ui| {
        form_label(ui, label);
        let text_width = (ui.available_width() - 88.0).max(160.0);
        ui.add_sized(
            [text_width, TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(value).vertical_align(egui::Align::Center),
        );
        if ui.button(t("Browse")).clicked() {
            let selected = if open_file {
                rfd::FileDialog::new()
                    .set_title(t("Select client template"))
                    .pick_file()
            } else {
                rfd::FileDialog::new()
                    .set_title(t("Save configured client"))
                    .save_file()
            };
            if let Some(path) = selected {
                *value = path_to_string(path);
            }
        }
    });
}

fn form_label(ui: &mut egui::Ui, label: &str) {
    ui.add_sized(
        [FORM_LABEL_WIDTH, TOOLBAR_CONTROL_HEIGHT],
        egui::Label::new(egui::RichText::new(label).color(color_muted())),
    );
}

fn render_template_detail(ui: &mut egui::Ui, detail: &str, status: &TemplateStatus) {
    let label = match status {
        TemplateStatus::Unknown(_) => format!("! {}", t("Details")),
        TemplateStatus::Valid(_) => format!("+ {}", t("Details")),
        TemplateStatus::Invalid(_) => format!("x {}", t("Details")),
    };
    ui.horizontal_top(|ui| {
        form_label(ui, &label);
        let mut detail = detail.to_string();
        ui.add_sized(
            [ui.available_width(), DETAILS_HEIGHT],
            egui::TextEdit::multiline(&mut detail)
                .font(egui::TextStyle::Monospace)
                .desired_rows(8)
                .desired_width(f32::INFINITY)
                .interactive(false)
                .code_editor(),
        );
    });
}

fn render_build_status_bar(ui: &mut egui::Ui, status: &BuildStatus) {
    let (label, notice, color) = match status {
        BuildStatus::Idle => (
            t("Ready"),
            t("No client has been generated in this window yet"),
            color_muted(),
        ),
        BuildStatus::Success(message) => (t("Generated"), message.as_str(), color_good()),
        BuildStatus::Error(message) => (t("Failed"), message.as_str(), color_bad()),
    };
    egui::Frame::default()
        .fill(color.gamma_multiply(0.08))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.35)))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_min_height(26.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(
                    egui::RichText::new(label)
                        .size(12.0)
                        .color(color_text())
                        .strong(),
                );
                ui.label(egui::RichText::new(notice).size(12.0).color(color_muted()))
                    .on_hover_text(notice);
            });
        });
}

fn optional_token(token: &str) -> Option<&str> {
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn default_template_path() -> PathBuf {
    binary_sibling_path(client_binary_name()).unwrap_or_else(|| PathBuf::from(client_binary_name()))
}

fn default_output_path(template_path: &Path) -> PathBuf {
    let parent = template_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = template_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("rdl-client-gui");
    let extension = template_path.extension().and_then(|value| value.to_str());
    let file_name = match extension {
        Some(extension) if !extension.is_empty() => format!("{stem}-configured.{extension}"),
        _ => format!("{stem}-configured"),
    };
    parent.join(file_name)
}

fn binary_sibling_path(file_name: &str) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(file_name)))
}

fn client_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "rdl-client-gui.exe"
    } else {
        "rdl-client-gui"
    }
}

fn path_to_string(path: impl Into<PathBuf>) -> String {
    path.into().display().to_string()
}

fn inspect_template(path_text: &str) -> TemplateReport {
    let path_text = path_text.trim();
    if path_text.is_empty() {
        return TemplateReport {
            detail: format!(
                "{}: {}\n{}: {}",
                t("Template"),
                t("not selected"),
                t("Validation"),
                t("not loaded")
            ),
            status: TemplateStatus::Unknown(t("Select a client template binary").to_string()),
        };
    }

    let path = Path::new(path_text);

    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            let notice = format!("{}: {error}", t("Template cannot be read"));
            return TemplateReport {
                detail: format!(
                    "{}\n{}: {} - {notice}",
                    t("Template"),
                    t("Validation"),
                    t("invalid")
                ),
                status: TemplateStatus::Invalid(notice),
            };
        }
    };

    let size = human_size(metadata.len());
    let modified = metadata
        .modified()
        .map(format_system_time)
        .unwrap_or_else(|error| format!("{} ({error})", t("modified unavailable")));

    if !metadata.is_file() {
        let notice = t("Template path is not a file").to_string();
        return TemplateReport {
            detail: format!(
                "{}\n{}: {size} ({} {})\n{}: {modified}\n{}: {} - {notice}",
                t("Template"),
                t("Size"),
                metadata.len(),
                t("bytes"),
                t("Modified"),
                t("Validation"),
                t("invalid")
            ),
            status: TemplateStatus::Invalid(notice),
        };
    }

    let mut detail_lines = vec![
        t("Template").to_string(),
        format!("{}: {size} ({} {})", t("Size"), metadata.len(), t("bytes")),
        format!("{}: {modified}", t("Modified")),
        t("Embedded mode: generated clients do not load, create, or save client.toml").to_string(),
    ];

    let binary = match fs::read(path) {
        Ok(bytes) => {
            let binary = detect_binary_format(&bytes);
            detail_lines.push(format!("{}: {}", t("Platform"), binary.platform));
            detail_lines.push(format!("{}: {}", t("Format"), binary.format));
            detail_lines.push(format!("{}: {}", t("Arch"), binary.arch));
            binary
        }
        Err(error) => {
            let notice = format!("{}: {error}", t("Template bytes cannot be read"));
            return TemplateReport {
                detail: {
                    detail_lines.push(format!("{}: {} - {notice}", t("Validation"), t("invalid")));
                    detail_lines.join("\n")
                },
                status: TemplateStatus::Invalid(notice),
            };
        }
    };

    let mut slot_present = false;
    match rdl_config::inspect_embedded_endpoint_config(path, ConfigKind::Client) {
        Ok(inspection) => {
            if let Some(offset) = inspection.slot_offset {
                slot_present = true;
                detail_lines.push(format!(
                    "{}: {} {}={} {}={} {}={}",
                    t("Embedded slot"),
                    t("present"),
                    t("offset"),
                    offset,
                    t("capacity"),
                    inspection.payload_capacity,
                    t("used"),
                    inspection.payload_bytes
                ));
                match inspection.config {
                    Some(config) => {
                        let ip = config.ip.unwrap_or_else(|| format!("<{}>", t("missing")));
                        let port = config
                            .port
                            .map(|port| port.to_string())
                            .unwrap_or_else(|| format!("<{}>", t("missing")));
                        let token = if config
                            .auth_token
                            .as_deref()
                            .map(str::trim)
                            .unwrap_or_default()
                            .is_empty()
                        {
                            t("no")
                        } else {
                            t("yes")
                        };
                        detail_lines.push(format!(
                            "{}: {}={ip}:{port} {}={token}",
                            t("Embedded config"),
                            t("server"),
                            t("token")
                        ));
                        detail_lines.push(
                            t("Reuse: existing embedded config will be replaced when generated")
                                .to_string(),
                        );
                    }
                    None => {
                        detail_lines.push(format!("{}: {}", t("Embedded config"), t("empty")));
                    }
                }
            } else {
                detail_lines.push(format!(
                    "{}: {}",
                    t("Embedded slot"),
                    t("missing (not a supported client template)")
                ));
            }
        }
        Err(error) => {
            detail_lines.push(format!("{}: {} ({error})", t("Embedded slot"), t("error")));
        }
    }

    let status = if binary.platform == "Unknown" {
        TemplateStatus::Invalid(t("Unsupported or unknown binary format").to_string())
    } else if !slot_present {
        TemplateStatus::Invalid(t("Embedded config slot is missing").to_string())
    } else {
        TemplateStatus::Valid(format!(
            "{} {} {} {}",
            binary.platform,
            binary.format,
            binary.arch,
            t("template is ready")
        ))
    };
    let validation = match &status {
        TemplateStatus::Valid(notice) => format!("{}: {} - {notice}", t("Validation"), t("valid")),
        TemplateStatus::Invalid(notice) => {
            format!("{}: {} - {notice}", t("Validation"), t("invalid"))
        }
        TemplateStatus::Unknown(notice) => {
            format!("{}: {} - {notice}", t("Validation"), t("not loaded"))
        }
    };
    detail_lines.push(validation);

    TemplateReport {
        detail: detail_lines.join("\n"),
        status,
    }
}

struct TemplateReport {
    detail: String,
    status: TemplateStatus,
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_system_time(time: SystemTime) -> String {
    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return "before 1970-01-01".to_string();
    };
    Local
        .timestamp_opt(duration.as_secs() as i64, 0)
        .single()
        .map(|datetime| datetime.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| duration.as_secs().to_string())
}

#[cfg(target_os = "macos")]
fn postprocess_generated_client(path: &Path) -> Result<Option<String>, String> {
    let output = Command::new("codesign")
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(path)
        .output()
        .map_err(|error| format!("generated, but macOS ad-hoc sign failed: {error}"))?;
    if output.status.success() {
        return Ok(Some("signed=adhoc".to_string()));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!(
            "generated, but macOS ad-hoc sign failed with status {}",
            output.status
        ))
    } else {
        Err(format!(
            "generated, but macOS ad-hoc sign failed: {}",
            stderr
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn postprocess_generated_client(_path: &Path) -> Result<Option<String>, String> {
    Ok(None)
}

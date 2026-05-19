use super::{
    event::AdminInput,
    payload::payload_field,
    ui::{COLOR_BAD, COLOR_GOOD, COLOR_WARN, TOOLBAR_CONTROL_HEIGHT},
};
use crate::i18n::{self, t};
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use egui_extras::Column;
use rdl_protocol::CommandKind;
use std::hash::{Hash, Hasher};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::SyncSender,
    Arc, Mutex,
};
use std::time::{Duration, Instant};

mod registry;

const PERFORMANCE_AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const TABLE_BODY_TEXT_SIZE: f32 = 11.5;
const TABLE_HEADER_TEXT_SIZE: f32 = 11.5;
const TABLE_BODY_CELL_HEIGHT: f32 = 16.0;
const TABLE_HEADER_CELL_HEIGHT: f32 = 17.0;
const TABLE_SORT_MARKER_WIDTH: f32 = 12.0;
const TABLE_WIDTH_SAMPLE_ROWS: usize = 200;

pub(super) struct CommandResultWindow {
    pub(super) id: u64,
    pub(super) client_id: String,
    pub(super) hostname: String,
    pub(super) username: String,
    pub(super) command: CommandKind,
    pub(super) status: CommandResultStatus,
    pub(super) detail: String,
    pub(super) open: bool,
    pub(super) close_requested: Arc<AtomicBool>,
    pub(super) refresh_requested: Arc<AtomicBool>,
    pub(super) auto_refresh_enabled: Arc<AtomicBool>,
    pub(super) last_auto_refresh_at: Option<Instant>,
    pub(super) process_kill_requested: Arc<Mutex<Option<String>>>,
    pub(super) startup_action_requested: Arc<Mutex<Option<String>>>,
    pub(super) registry_key_requested: Arc<Mutex<Option<String>>>,
    pub(super) startup_add_form: Arc<Mutex<StartupAddForm>>,
    pub(super) table_filter: Arc<Mutex<String>>,
    pub(super) table_sort: Arc<Mutex<Option<TableSort>>>,
    pub(super) table_selected_row: Arc<Mutex<Option<String>>>,
}

#[derive(Default)]
pub(super) struct StartupAddForm {
    open: bool,
    name: String,
    command: String,
    error: String,
}

#[derive(Clone, Copy)]
pub(super) enum CommandResultStatus {
    Pending,
    Accepted,
    Failed,
}

#[derive(Clone, Copy)]
pub(super) struct TableSort {
    column: usize,
    ascending: bool,
}

pub(super) fn update_command_window(
    window: &mut CommandResultWindow,
    accepted: bool,
    detail: String,
    hostname: String,
    username: String,
) {
    window.status = if accepted {
        CommandResultStatus::Accepted
    } else {
        CommandResultStatus::Failed
    };
    window.detail = if accepted && window.command == CommandKind::RegistryManager {
        registry::merge_details(&window.detail, &detail).unwrap_or(detail)
    } else {
        detail
    };
    window.hostname = hostname;
    window.username = username;
    window.open = true;
}

pub(super) fn refresh_command_window(
    input_tx: &SyncSender<AdminInput>,
    window: &mut CommandResultWindow,
    detail: &str,
    log_prefix: &str,
    now: Instant,
    pending_logs: &mut Vec<String>,
) {
    let _ = input_tx.send(AdminInput::Command {
        target_id: window.client_id.clone(),
        command: window.command.clone(),
        payload: String::new(),
    });
    let keep_existing_performance_detail = window.command == CommandKind::PerformanceMonitor
        && !window.detail.trim().is_empty()
        && !performance_monitor_pending_detail(&window.detail);
    window.status = CommandResultStatus::Pending;
    if !keep_existing_performance_detail {
        window.detail = detail.to_string();
    }
    window.open = true;
    if window.command == CommandKind::PerformanceMonitor {
        window.last_auto_refresh_at = Some(now);
    }
    pending_logs.push(format!(
        "{log_prefix} command={} to {}",
        window.command.as_str(),
        window.client_id
    ));
}

pub(super) fn performance_auto_refresh_due(window: &mut CommandResultWindow, now: Instant) -> bool {
    if window.command != CommandKind::PerformanceMonitor || !window.open {
        window.last_auto_refresh_at = None;
        return false;
    }
    if !window.auto_refresh_enabled.load(Ordering::Relaxed) {
        window.last_auto_refresh_at = None;
        return false;
    }
    if matches!(window.status, CommandResultStatus::Pending) {
        return false;
    }

    let Some(last_refresh) = window.last_auto_refresh_at else {
        window.last_auto_refresh_at = Some(now);
        return false;
    };
    now.duration_since(last_refresh) >= PERFORMANCE_AUTO_REFRESH_INTERVAL
}

pub(super) fn render_command_window_status_bar(
    ui: &mut egui::Ui,
    status: &CommandResultStatus,
    notice: Option<&str>,
) {
    let (status_text, default_progress_text, color) = command_window_status(status);
    let progress_text = notice.unwrap_or(default_progress_text);
    crate::theme::status_frame().show(ui, |ui| {
        ui.set_min_height(26.0);
        crate::theme::render_status_line(ui, status_text, color, progress_text, |_| {});
    });
}

fn command_window_status(
    status: &CommandResultStatus,
) -> (&'static str, &'static str, egui::Color32) {
    match status {
        CommandResultStatus::Pending => (t("Pending"), t("Waiting for client result"), COLOR_WARN),
        CommandResultStatus::Accepted => (t("Done"), t("Result received"), COLOR_GOOD),
        CommandResultStatus::Failed => (t("Failed"), t("Command failed"), COLOR_BAD),
    }
}

pub(super) fn command_window_identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

pub(super) fn command_status_notice(
    command: &CommandKind,
    status: CommandResultStatus,
    detail: &str,
) -> Option<String> {
    if matches!(status, CommandResultStatus::Accepted) {
        if command_expects_result_table(command) && parse_result_table(detail).is_none() {
            return Some(t("Table data could not be parsed").to_string());
        }
        if matches!(command, CommandKind::PerformanceMonitor)
            && !detail.trim().is_empty()
            && parse_performance_metrics(detail).is_empty()
        {
            return Some(t("Performance metrics could not be parsed").to_string());
        }
        if command_allows_plain_detail(command, detail) {
            return Some(t("Result received").to_string());
        }
    }
    None
}

fn command_expects_result_table(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::ProcessManager
            | CommandKind::WindowManager
            | CommandKind::StartupManager
            | CommandKind::RegistryManager
            | CommandKind::DriverManager
            | CommandKind::EventLog
            | CommandKind::ActiveConnections
    )
}

fn command_allows_plain_detail(command: &CommandKind, detail: &str) -> bool {
    if detail.trim().is_empty() || command_expects_result_table(command) {
        return false;
    }
    if matches!(command, CommandKind::Camera) && parse_camera_frame(detail).is_some() {
        return false;
    }
    true
}

pub(super) fn kill_target_process_succeeded(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("ok")
        && !detail.contains("refused")
        && !detail.contains("requires")
        && !detail.contains("failed")
        && !detail.contains("exited with error")
}

pub(super) fn quiet_user_interaction_command(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::OpenTextInNotepad
    )
}

pub(super) fn session_command_requires_confirmation(command: &CommandKind) -> bool {
    matches!(
        command,
        CommandKind::UpdateClient
            | CommandKind::UninstallClient
            | CommandKind::KillClientProcess
            | CommandKind::Shutdown
            | CommandKind::Reboot
            | CommandKind::ClientConfig
            | CommandKind::DeleteClient
    )
}

pub(super) fn detail_status(detail: &str) -> Option<String> {
    payload_field(detail, "status")
}

pub(super) struct CommandResultRenderState<'a> {
    pub(super) table_filter: &'a Arc<Mutex<String>>,
    pub(super) table_sort: &'a Arc<Mutex<Option<TableSort>>>,
    pub(super) table_selected_row: &'a Arc<Mutex<Option<String>>>,
    pub(super) refresh_requested: &'a Arc<AtomicBool>,
    pub(super) auto_refresh_enabled: &'a Arc<AtomicBool>,
    pub(super) refresh_in_flight: bool,
    pub(super) process_kill_requested: &'a Arc<Mutex<Option<String>>>,
    pub(super) startup_action_requested: &'a Arc<Mutex<Option<String>>>,
    pub(super) registry_key_requested: &'a Arc<Mutex<Option<String>>>,
    pub(super) startup_add_form: &'a Arc<Mutex<StartupAddForm>>,
}

pub(super) fn render_command_result(
    ui: &mut egui::Ui,
    command: &CommandKind,
    detail: &mut String,
    state: CommandResultRenderState<'_>,
) {
    if command_expects_result_table(command) {
        let table = parse_result_table(detail);
        let startup_client_status = if matches!(command, CommandKind::StartupManager) {
            Some(
                table
                    .as_ref()
                    .map(startup_client_autostart_status)
                    .unwrap_or(StartupClientAutostartStatus::Unknown),
            )
        } else {
            None
        };
        render_table_toolbar(
            ui,
            command,
            state.table_filter,
            state.refresh_requested,
            state.refresh_in_flight,
            state.startup_add_form,
            state.startup_action_requested,
            startup_client_status,
        );
        if matches!(command, CommandKind::StartupManager) {
            render_startup_add_form(
                ui,
                state.startup_add_form,
                state.startup_action_requested,
                state.refresh_in_flight,
            );
        }
        ui.add_space(8.0);
        if let Some(table) = table.as_ref() {
            if matches!(command, CommandKind::RegistryManager) {
                registry::render_result(
                    ui,
                    &table,
                    state.table_filter,
                    state.table_selected_row,
                    state.registry_key_requested,
                );
                return;
            }
            render_result_table(ui, command, &table, &state);
            return;
        }
        return;
    }
    if matches!(command, CommandKind::PerformanceMonitor) {
        render_performance_monitor_toolbar(
            ui,
            state.refresh_requested,
            state.auto_refresh_enabled,
            state.refresh_in_flight,
        );
        ui.add_space(8.0);
        render_performance_monitor_detail(ui, detail, state.refresh_in_flight);
        return;
    }
    if matches!(command, CommandKind::Camera) {
        if render_camera_result(ui, detail) {
            ui.add_space(8.0);
        } else {
            render_plain_command_detail(ui, detail);
        }
        return;
    }
    render_plain_command_detail(ui, detail);
}

struct PerformanceMetrics {
    cpu: Option<PerformanceMetric>,
    memory: Option<PerformanceMetric>,
    disk: Option<PerformanceMetric>,
}

impl PerformanceMetrics {
    fn is_empty(&self) -> bool {
        self.cpu.is_none() && self.memory.is_none() && self.disk.is_none()
    }

    fn bars(&self) -> Vec<&PerformanceMetric> {
        [&self.cpu, &self.memory, &self.disk]
            .into_iter()
            .flatten()
            .collect()
    }
}

struct PerformanceMetric {
    label: &'static str,
    percent: f32,
    value: String,
    color: egui::Color32,
}

fn render_performance_monitor_detail(
    ui: &mut egui::Ui,
    detail: &mut String,
    refresh_in_flight: bool,
) {
    let metrics = parse_performance_metrics(detail);
    if !metrics.is_empty() {
        render_performance_metric_bars(ui, &metrics);
        ui.add_space(10.0);
    }
    if refresh_in_flight && performance_monitor_pending_detail(detail) {
        return;
    }
    render_plain_command_detail(ui, detail);
}

fn performance_monitor_pending_detail(detail: &str) -> bool {
    matches!(
        detail.trim(),
        "Waiting for client result..."
            | "Refreshing command result..."
            | "Auto refreshing performance monitor..."
    )
}

fn render_performance_metric_bars(ui: &mut egui::Ui, metrics: &PerformanceMetrics) {
    egui::Frame::default()
        .fill(crate::theme::palette().panel)
        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(t("Resource usage"))
                    .size(12.0)
                    .color(crate::theme::palette().text)
                    .strong(),
            );
            ui.add_space(8.0);
            for metric in metrics.bars() {
                render_performance_metric_bar(ui, metric);
                ui.add_space(6.0);
            }
        });
}

fn render_performance_metric_bar(ui: &mut egui::Ui, metric: &PerformanceMetric) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [82.0, 20.0],
            egui::Label::new(
                egui::RichText::new(metric.label)
                    .size(12.0)
                    .color(crate::theme::palette().muted),
            )
            .truncate(),
        );
        let bar_width = (ui.available_width() - 4.0).max(120.0);
        ui.add_sized(
            [bar_width, 20.0],
            egui::ProgressBar::new((metric.percent / 100.0).clamp(0.0, 1.0))
                .fill(metric.color)
                .text(metric.value.clone()),
        );
    });
}

fn render_plain_command_detail(ui: &mut egui::Ui, detail: &mut String) {
    if detail.trim().is_empty() {
        return;
    }
    ui.add(
        egui::TextEdit::multiline(detail)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .desired_rows(18)
            .interactive(true),
    );
}

fn parse_performance_metrics(detail: &str) -> PerformanceMetrics {
    PerformanceMetrics {
        cpu: parse_cpu_metric(detail),
        memory: parse_memory_metric(detail),
        disk: parse_disk_metric(detail),
    }
}

fn parse_cpu_metric(detail: &str) -> Option<PerformanceMetric> {
    parse_named_number(
        detail,
        &["cpu_percent", "cpupercent", "LoadPercent", "LoadPercentage"],
    )
    .map(|value| percent_metric(t("CPU"), value, crate::theme::COLOR_METRIC_CPU))
    .or_else(|| {
        let load = parse_load_average(detail)?;
        let cores = std::thread::available_parallelism()
            .map(|value| value.get() as f32)
            .unwrap_or(1.0)
            .max(1.0);
        Some(PerformanceMetric {
            label: t("CPU Load"),
            percent: clamp_percent(load * 100.0 / cores),
            value: format!("{load:.2} load"),
            color: crate::theme::COLOR_METRIC_CPU,
        })
    })
}

fn parse_memory_metric(detail: &str) -> Option<PerformanceMetric> {
    parse_named_number(
        detail,
        &["memory_percent", "memorypercent", "MemoryPercent"],
    )
    .or_else(|| parse_windows_memory_percent(detail))
    .or_else(|| parse_linux_memory_percent(detail))
    .or_else(|| parse_macos_memory_percent(detail))
    .map(|value| percent_metric(t("Memory"), value, crate::theme::COLOR_METRIC_MEMORY))
}

fn parse_disk_metric(detail: &str) -> Option<PerformanceMetric> {
    parse_named_number(detail, &["disk_percent", "diskpercent", "DiskPercent"])
        .or_else(|| parse_df_disk_percent(detail))
        .map(|value| percent_metric(t("Disk"), value, crate::theme::COLOR_METRIC_DISK))
}

fn percent_metric(label: &'static str, value: f32, color: egui::Color32) -> PerformanceMetric {
    let percent = clamp_percent(value);
    PerformanceMetric {
        label,
        percent,
        value: format!("{percent:.1}%"),
        color,
    }
}

fn clamp_percent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn parse_named_number(detail: &str, keys: &[&str]) -> Option<f32> {
    detail.lines().find_map(|line| {
        keys.iter()
            .find_map(|key| parse_number_after_key(line.trim(), key))
    })
}

fn parse_number_after_key(line: &str, key: &str) -> Option<f32> {
    let key_len = key.len();
    let prefix = line.get(..key_len)?;
    if !prefix.eq_ignore_ascii_case(key) {
        return None;
    }
    let rest = line[key_len..]
        .trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '='));
    first_number(rest)
}

fn parse_load_average(detail: &str) -> Option<f32> {
    detail.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        ["load average:", "load averages:"]
            .into_iter()
            .find_map(|marker| {
                let index = lower.find(marker)?;
                first_number(&line[index + marker.len()..])
            })
    })
}

fn parse_windows_memory_percent(detail: &str) -> Option<f32> {
    let total = parse_named_number(detail, &["TotalMemoryMB"])?;
    let free = parse_named_number(detail, &["FreeMemoryMB"])?;
    (total > 0.0).then_some((total - free) * 100.0 / total)
}

fn parse_linux_memory_percent(detail: &str) -> Option<f32> {
    detail.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("Mem:")?;
        let values = rest
            .split_whitespace()
            .filter_map(first_number)
            .collect::<Vec<_>>();
        let total = *values.first()?;
        let used = *values.get(1)?;
        (total > 0.0).then_some(used * 100.0 / total)
    })
}

fn parse_macos_memory_percent(detail: &str) -> Option<f32> {
    let free = parse_vm_stat_pages(detail, "Pages free")?;
    let active = parse_vm_stat_pages(detail, "Pages active").unwrap_or(0.0);
    let inactive = parse_vm_stat_pages(detail, "Pages inactive").unwrap_or(0.0);
    let speculative = parse_vm_stat_pages(detail, "Pages speculative").unwrap_or(0.0);
    let wired = parse_vm_stat_pages(detail, "Pages wired down").unwrap_or(0.0);
    let compressed = parse_vm_stat_pages(detail, "Pages occupied by compressor").unwrap_or(0.0);
    let used = active + inactive + wired + compressed;
    let total = used + free + speculative;
    (total > 0.0).then_some(used * 100.0 / total)
}

fn parse_vm_stat_pages(detail: &str, label: &str) -> Option<f32> {
    detail.lines().find_map(|line| {
        let trimmed = line.trim_start();
        let prefix = trimmed.get(..label.len())?;
        if !prefix.eq_ignore_ascii_case(label) {
            return None;
        }
        first_number(&trimmed[label.len()..])
    })
}

fn parse_df_disk_percent(detail: &str) -> Option<f32> {
    let mut lines = detail.lines();
    while lines
        .next()
        .is_some_and(|line| !line.split_whitespace().any(|cell| cell == "Filesystem"))
    {}
    lines.find_map(|line| {
        line.split_whitespace()
            .find_map(|cell| cell.strip_suffix('%').and_then(first_number))
    })
}

fn first_number(value: &str) -> Option<f32> {
    let mut number = String::new();
    let mut started = false;
    for ch in value.chars() {
        if ch.is_ascii_digit() || matches!(ch, '.' | '-') {
            number.push(ch);
            started = true;
        } else if ch == ',' && started {
            continue;
        } else if started {
            break;
        }
    }
    if number.is_empty() || number == "." || number == "-" {
        return None;
    }
    number.parse::<f32>().ok()
}

fn render_camera_result(ui: &mut egui::Ui, detail: &str) -> bool {
    let Some(frame) = parse_camera_frame(detail) else {
        return false;
    };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(frame.image_base64) {
        Ok(bytes) => bytes,
        Err(error) => {
            ui.label(
                egui::RichText::new(format!("decode camera frame failed: {error}"))
                    .color(COLOR_BAD),
            );
            return true;
        }
    };
    let image = match image::load_from_memory(&bytes) {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            ui.label(
                egui::RichText::new(format!("load camera frame failed: {error}")).color(COLOR_BAD),
            );
            return true;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    let texture = ui.ctx().load_texture(
        format!("camera_frame:{}", stable_hash(detail)),
        color_image,
        egui::TextureOptions::LINEAR,
    );
    let available_width = ui.available_width().max(1.0);
    let scale = (available_width / size[0] as f32).min(1.0);
    let display_size = egui::vec2(size[0] as f32 * scale, size[1] as f32 * scale);
    ui.add(egui::Image::new(&texture).fit_to_exact_size(display_size));
    true
}

struct CameraFrame<'a> {
    image_base64: &'a str,
}

fn parse_camera_frame(detail: &str) -> Option<CameraFrame<'_>> {
    let mut lines = detail.lines();
    if lines.next()?.trim() != "camera_frame" {
        return None;
    }
    let image_base64 = lines.find_map(|line| line.strip_prefix("image_base64="))?;
    Some(CameraFrame { image_base64 })
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn render_table_toolbar(
    ui: &mut egui::Ui,
    command: &CommandKind,
    table_filter: &Arc<Mutex<String>>,
    refresh_requested: &Arc<AtomicBool>,
    refresh_in_flight: bool,
    startup_add_form: &Arc<Mutex<StartupAddForm>>,
    startup_action_requested: &Arc<Mutex<Option<String>>>,
    startup_client_status: Option<StartupClientAutostartStatus>,
) {
    let mut filter = table_filter
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        ui.allocate_ui_with_layout(
            egui::vec2(38.0, TOOLBAR_CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(t("Filter"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
            },
        );
        let response = ui.add_sized(
            [240.0, TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut filter)
                .hint_text(t("Filter table content"))
                .vertical_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(mut value) = table_filter.lock() {
                *value = filter.clone();
            }
        }
        let label = if refresh_in_flight {
            t("Refreshing...")
        } else {
            t("Refresh")
        };
        if ui
            .add_enabled(!refresh_in_flight, egui::Button::new(label))
            .clicked()
        {
            refresh_requested.store(true, Ordering::Relaxed);
        }
        if matches!(command, CommandKind::StartupManager) {
            if ui
                .add_enabled(!refresh_in_flight, egui::Button::new(t("Add Item")))
                .clicked()
            {
                if let Ok(mut form) = startup_add_form.lock() {
                    form.open = true;
                    form.error.clear();
                }
            }
            ui.add_enabled_ui(!refresh_in_flight, |ui| {
                render_client_autostart_menu(
                    ui,
                    startup_client_status.unwrap_or(StartupClientAutostartStatus::Unknown),
                    startup_action_requested,
                );
            });
        }
    });
}

fn render_client_autostart_menu(
    ui: &mut egui::Ui,
    status: StartupClientAutostartStatus,
    startup_action_requested: &Arc<Mutex<Option<String>>>,
) {
    let style = startup_client_autostart_style(status);
    let button = egui::Button::new(
        egui::RichText::new(t(style.label))
            .size(12.0)
            .color(style.text),
    )
    .fill(style.fill)
    .stroke(egui::Stroke::new(1.0, style.stroke));

    let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        if ui.button(t("Enable")).clicked() {
            queue_startup_action(startup_action_requested, "enable_client_autostart");
            ui.close();
        }
        if ui.button(t("Disable")).clicked() {
            queue_startup_action(startup_action_requested, "disable_client_autostart");
            ui.close();
        }
    });
    response.on_hover_text(t("Configure login autostart for this client"));
}

fn queue_startup_action(startup_action_requested: &Arc<Mutex<Option<String>>>, action: &str) {
    if let Ok(mut value) = startup_action_requested.lock() {
        *value = Some(format!("action={action}"));
    }
}

fn render_startup_add_form(
    ui: &mut egui::Ui,
    startup_add_form: &Arc<Mutex<StartupAddForm>>,
    startup_action_requested: &Arc<Mutex<Option<String>>>,
    refresh_in_flight: bool,
) {
    let mut queued_payload = None;
    if let Ok(mut form) = startup_add_form.lock() {
        if !form.open {
            return;
        }

        ui.add_space(8.0);
        egui::Frame::default()
            .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
                    ui.label(
                        egui::RichText::new(t("Name"))
                            .size(12.0)
                            .color(crate::theme::palette().muted),
                    );
                    ui.add_sized(
                        [160.0, TOOLBAR_CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut form.name)
                            .hint_text(t("Item name"))
                            .vertical_align(egui::Align::Center),
                    );
                    ui.label(
                        egui::RichText::new(t("Command"))
                            .size(12.0)
                            .color(crate::theme::palette().muted),
                    );
                    ui.add_sized(
                        [360.0, TOOLBAR_CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut form.command)
                            .hint_text(t("Command or executable path"))
                            .vertical_align(egui::Align::Center),
                    );

                    let can_submit = !refresh_in_flight
                        && !form.name.trim().is_empty()
                        && !form.command.trim().is_empty();
                    if ui
                        .add_enabled(can_submit, egui::Button::new(t("Add")))
                        .clicked()
                    {
                        queued_payload = Some(startup_add_payload(&form.name, &form.command));
                        form.name.clear();
                        form.command.clear();
                        form.error.clear();
                        form.open = false;
                    }
                    if ui.button(t("Cancel")).clicked() {
                        form.error.clear();
                        form.open = false;
                    }
                });
                if !form.error.trim().is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(&form.error).size(12.0).color(COLOR_BAD));
                } else if form.name.trim().is_empty() || form.command.trim().is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(t("Name and command are required."))
                            .size(12.0)
                            .color(crate::theme::palette().muted),
                    );
                }
            });
    }

    if let Some(payload) = queued_payload {
        if let Ok(mut action) = startup_action_requested.lock() {
            *action = Some(payload);
        }
    }
}

fn render_performance_monitor_toolbar(
    ui: &mut egui::Ui,
    refresh_requested: &Arc<AtomicBool>,
    auto_refresh_enabled: &Arc<AtomicBool>,
    refresh_in_flight: bool,
) {
    let mut auto_refresh = auto_refresh_enabled.load(Ordering::Relaxed);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        let label = if refresh_in_flight {
            t("Refreshing...")
        } else {
            t("Refresh")
        };
        if ui
            .add_enabled(!refresh_in_flight, egui::Button::new(label))
            .clicked()
        {
            refresh_requested.store(true, Ordering::Relaxed);
        }
        if ui
            .checkbox(&mut auto_refresh, t("Auto refresh (5s)"))
            .changed()
        {
            auto_refresh_enabled.store(auto_refresh, Ordering::Relaxed);
        }
    });
}

struct ResultTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

#[derive(Clone)]
struct DisplayTableRow {
    source_index: usize,
    cells: Vec<String>,
}

fn parse_result_table(detail: &str) -> Option<ResultTable> {
    let normalized = detail.replace("`t", "\t");
    let body = normalized
        .lines()
        .skip_while(|line| line.trim().is_empty() || line.trim_end().ends_with(':'))
        .collect::<Vec<_>>();
    if body.len() < 2 {
        return None;
    }

    if body.iter().any(|line| line.contains('\t')) {
        return parse_tab_table(&body);
    }

    parse_whitespace_table(&body)
}

fn parse_tab_table(lines: &[&str]) -> Option<ResultTable> {
    let headers = split_tab_row(lines.first()?)
        .into_iter()
        .map(clean_cell)
        .collect();
    let rows = lines
        .iter()
        .skip(1)
        .map(|line| {
            split_tab_row(line)
                .into_iter()
                .map(clean_cell)
                .collect::<Vec<_>>()
        })
        .filter(|row| row.len() >= 2)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        None
    } else {
        Some(ResultTable { headers, rows })
    }
}

fn parse_whitespace_table(lines: &[&str]) -> Option<ResultTable> {
    let headers = split_ws_row(lines.first()?);
    if headers.len() < 2 {
        return None;
    }
    let rows = lines
        .iter()
        .skip(1)
        .map(|line| split_ws_row(line))
        .filter(|row| row.len() >= 2)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        None
    } else {
        Some(ResultTable { headers, rows })
    }
}

fn split_tab_row(line: &str) -> Vec<&str> {
    line.split('\t')
        .filter(|cell| !cell.trim().is_empty())
        .collect()
}

fn split_ws_row(line: &str) -> Vec<String> {
    line.split_whitespace().map(clean_cell).collect()
}

fn clean_cell(value: impl AsRef<str>) -> String {
    value.as_ref().trim().to_string()
}

fn render_result_table(
    ui: &mut egui::Ui,
    command: &CommandKind,
    table: &ResultTable,
    state: &CommandResultRenderState<'_>,
) {
    let filter = state
        .table_filter
        .lock()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let mut sort = state.table_sort.lock().map(|value| *value).unwrap_or(None);
    let selected_row = state
        .table_selected_row
        .lock()
        .map(|value| value.clone())
        .unwrap_or(None);
    let mut rows = filtered_table_rows(table, &filter);
    sort_table_rows(&mut rows, sort);
    let row_cells = rows.iter().map(|row| row.cells.clone()).collect::<Vec<_>>();
    let widths = table_column_widths(command, &table.headers, &row_cells, ui.available_width());
    let alignments = table_column_alignments(command, &table.headers);
    let specs = table
        .headers
        .iter()
        .map(|header| table_column_spec(command, header))
        .collect::<Vec<_>>();

    egui::Frame::default()
        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
        .corner_radius(6.0)
        .show(ui, |ui| {
            let mut table_builder = crate::theme::clickable_table(
                ui,
                command_result_table_id(command, &table.headers),
                true,
            );
            for (width, spec) in widths.iter().zip(specs.iter()) {
                table_builder =
                    table_builder.column(Column::initial(*width).at_least(spec.min).clip(true));
            }

            table_builder
                .header(TABLE_HEADER_CELL_HEIGHT + 7.0, |mut header| {
                    for (index, cell) in table.headers.iter().enumerate() {
                        let align = alignments.get(index).copied().unwrap_or(egui::Align::Min);
                        let marker = match sort {
                            Some(current) if current.column == index && current.ascending => " ^",
                            Some(current) if current.column == index => " v",
                            _ => "",
                        };
                        header.col(|ui| {
                            let response = table_cell_label(
                                ui,
                                &format!("{cell}{marker}"),
                                TABLE_HEADER_TEXT_SIZE,
                                crate::theme::palette().muted,
                                align,
                                egui::Sense::click(),
                            );
                            if response.clicked() {
                                sort = match sort {
                                    Some(current) if current.column == index => Some(TableSort {
                                        column: index,
                                        ascending: !current.ascending,
                                    }),
                                    _ => Some(TableSort {
                                        column: index,
                                        ascending: true,
                                    }),
                                };
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(TABLE_BODY_CELL_HEIGHT + 7.0, rows.len(), |mut row| {
                        let row_data = &rows[row.index()];
                        let row_key = table_row_key(row_data);
                        let is_selected = selected_row.as_deref() == Some(row_key.as_str());
                        row.set_selected(is_selected);
                        let row_text = row_data.cells.join("\t");
                        let process_id = process_row_pid(command, &table.headers, &row_data.cells);
                        let startup_action =
                            startup_row_action(command, &table.headers, &row_data.cells);
                        let startup_delete_payload =
                            startup_row_delete_payload(command, &table.headers, &row_data.cells);
                        let startup_row_fill =
                            startup_client_row_fill(command, &table.headers, &row_data.cells);

                        for (index, _header) in table.headers.iter().enumerate() {
                            let cell = row_data.cells.get(index).map(String::as_str).unwrap_or("");
                            let align = alignments.get(index).copied().unwrap_or(egui::Align::Min);
                            let (_, cell_response) = row.col(|ui| {
                                if !is_selected {
                                    if let Some(fill) = startup_row_fill {
                                        paint_table_cell_background(ui, fill);
                                    }
                                }
                                let _ = table_cell_label(
                                    ui,
                                    cell,
                                    TABLE_BODY_TEXT_SIZE,
                                    crate::theme::palette().text,
                                    align,
                                    egui::Sense::hover(),
                                );
                            });
                            let cell_text = cell.to_string();
                            let row_text = row_text.clone();
                            let row_key = row_key.clone();
                            let process_id = process_id.clone();
                            let startup_action = startup_action.clone();
                            let startup_delete_payload = startup_delete_payload.clone();
                            cell_response.context_menu(|ui| {
                                if ui.button(t("Copy Cell")).clicked() {
                                    ui.ctx().copy_text(cell_text.clone());
                                    ui.close();
                                }
                                if ui.button(t("Copy Row")).clicked() {
                                    ui.ctx().copy_text(row_text.clone());
                                    ui.close();
                                }
                                if let Some(process_id) = process_id.clone() {
                                    ui.separator();
                                    if ui.button(t("Kill Process")).clicked() {
                                        if let Ok(mut selected) = state.table_selected_row.lock() {
                                            *selected = Some(row_key.clone());
                                        }
                                        if let Ok(mut value) = state.process_kill_requested.lock() {
                                            *value = Some(process_id.clone());
                                        }
                                        ui.close();
                                    }
                                }
                                if let Some(startup_action) = startup_action.clone() {
                                    ui.separator();
                                    if ui.button(t(startup_action.label)).clicked() {
                                        if let Ok(mut selected) = state.table_selected_row.lock() {
                                            *selected = Some(row_key.clone());
                                        }
                                        if let Ok(mut value) = state.startup_action_requested.lock()
                                        {
                                            *value = Some(startup_action.payload.clone());
                                        }
                                        ui.close();
                                    }
                                }
                                if let Some(startup_delete_payload) = startup_delete_payload.clone()
                                {
                                    ui.separator();
                                    if ui.button(t("Delete Startup Item")).clicked() {
                                        if let Ok(mut selected) = state.table_selected_row.lock() {
                                            *selected = Some(row_key.clone());
                                        }
                                        if let Ok(mut value) = state.startup_action_requested.lock()
                                        {
                                            *value = Some(startup_delete_payload);
                                        }
                                        ui.close();
                                    }
                                }
                            });
                        }

                        let response = row.response();
                        if response.hovered() {
                            response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() || response.secondary_clicked() {
                            if let Ok(mut value) = state.table_selected_row.lock() {
                                *value = Some(row_key.clone());
                            }
                        }
                    });
                });
        });

    if let Ok(mut value) = state.table_sort.lock() {
        *value = sort;
    }
    if rows.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(t("No rows match the current filter."))
                .size(12.0)
                .color(crate::theme::palette().muted),
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupClientAutostartStatus {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Clone, Copy)]
struct StartupClientAutostartStyle {
    label: &'static str,
    fill: egui::Color32,
    stroke: egui::Color32,
    text: egui::Color32,
}

fn startup_client_autostart_style(
    status: StartupClientAutostartStatus,
) -> StartupClientAutostartStyle {
    let palette = crate::theme::palette();
    match status {
        StartupClientAutostartStatus::Enabled => StartupClientAutostartStyle {
            label: "Client Autostart: On",
            fill: palette.success_bg,
            stroke: palette.border,
            text: palette.good,
        },
        StartupClientAutostartStatus::Disabled => StartupClientAutostartStyle {
            label: "Client Autostart: Off",
            fill: palette.danger_bg,
            stroke: palette.border,
            text: palette.bad,
        },
        StartupClientAutostartStatus::Unknown => StartupClientAutostartStyle {
            label: "Client Autostart: Unknown",
            fill: palette.neutral_bg,
            stroke: palette.border,
            text: palette.muted,
        },
    }
}

fn startup_client_autostart_status(table: &ResultTable) -> StartupClientAutostartStatus {
    let mut saw_disabled = false;
    for row in &table.rows {
        if !startup_row_is_client_autostart(&table.headers, row) {
            continue;
        }
        match startup_row_status(&table.headers, row) {
            Some(StartupClientAutostartStatus::Enabled) => {
                return StartupClientAutostartStatus::Enabled;
            }
            Some(StartupClientAutostartStatus::Disabled) => saw_disabled = true,
            _ => {}
        }
    }

    if saw_disabled {
        StartupClientAutostartStatus::Disabled
    } else if table.rows.iter().any(|row| {
        startup_row_status(&table.headers, row) == Some(StartupClientAutostartStatus::Unknown)
    }) {
        StartupClientAutostartStatus::Unknown
    } else {
        StartupClientAutostartStatus::Disabled
    }
}

fn startup_client_row_fill(
    command: &CommandKind,
    headers: &[String],
    row: &[String],
) -> Option<egui::Color32> {
    if !matches!(command, CommandKind::StartupManager)
        || !startup_row_is_client_autostart(headers, row)
    {
        return None;
    }
    let status = startup_row_status(headers, row)?;
    if status == StartupClientAutostartStatus::Unknown {
        return None;
    }
    Some(startup_client_autostart_style(status).fill)
}

fn paint_table_cell_background(ui: &mut egui::Ui, fill: egui::Color32) {
    let rect = ui.max_rect().intersect(ui.clip_rect());
    if rect.is_positive() {
        ui.painter().rect_filled(rect, 0.0, fill);
    }
}

fn startup_row_status(headers: &[String], row: &[String]) -> Option<StartupClientAutostartStatus> {
    let status = table_value(headers, row, "status")?;
    match status.trim().to_ascii_lowercase().as_str() {
        "enabled" | "registry" | "file" | "present" | "desktopentry" => {
            Some(StartupClientAutostartStatus::Enabled)
        }
        "disabled" => Some(StartupClientAutostartStatus::Disabled),
        "error" => Some(StartupClientAutostartStatus::Unknown),
        _ => None,
    }
}

fn startup_row_is_client_autostart(headers: &[String], row: &[String]) -> bool {
    let Some(name) = table_value(headers, row, "name") else {
        return false;
    };
    matches!(
        compact_startup_identity(name).as_str(),
        "rustdesklightclient"
            | "rustdesklightclientdesktop"
            | "rustdesklightclientdesktopdisabled"
            | "rustdesklightclientservice"
            | "comrustdesklightclientplist"
            | "comrustdesklightclientplistdisabled"
    )
}

fn compact_startup_identity(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

#[derive(Clone)]
struct StartupRowAction {
    label: &'static str,
    payload: String,
}

fn startup_row_action(
    command: &CommandKind,
    headers: &[String],
    row: &[String],
) -> Option<StartupRowAction> {
    if !matches!(command, CommandKind::StartupManager) {
        return None;
    }

    let status = table_value(headers, row, "status")?;
    let status_key = status.trim().to_ascii_lowercase();
    if status_key == "info" || status_key == "error" {
        return None;
    }

    let (action, label) = match status_key.as_str() {
        "disabled" => ("enable", "Enable Startup Item"),
        "enabled" | "registry" | "file" | "present" | "desktopentry" => {
            ("disable", "Disable Startup Item")
        }
        _ => return None,
    };

    let source = table_value(headers, row, "source")?;
    let name = table_value(headers, row, "name")?;
    if !startup_cell_is_actionable(source) || !startup_cell_is_actionable(name) {
        return None;
    }

    let scope = table_value(headers, row, "scope").unwrap_or_default();
    let startup_command = table_value(headers, row, "command").unwrap_or_default();
    Some(StartupRowAction {
        label,
        payload: startup_action_payload(action, scope, source, name, startup_command),
    })
}

fn startup_row_delete_payload(
    command: &CommandKind,
    headers: &[String],
    row: &[String],
) -> Option<String> {
    if !matches!(command, CommandKind::StartupManager) {
        return None;
    }

    let status = table_value(headers, row, "status")?;
    let status_key = status.trim().to_ascii_lowercase();
    if status_key == "info" || status_key == "error" {
        return None;
    }

    let source = table_value(headers, row, "source")?;
    let name = table_value(headers, row, "name")?;
    if !startup_cell_is_actionable(source)
        || !startup_cell_is_actionable(name)
        || !startup_row_is_deleteable(source, name)
    {
        return None;
    }

    let scope = table_value(headers, row, "scope").unwrap_or_default();
    let startup_command = table_value(headers, row, "command").unwrap_or_default();
    Some(startup_action_payload(
        "delete",
        scope,
        source,
        name,
        startup_command,
    ))
}

fn startup_row_is_deleteable(source: &str, name: &str) -> bool {
    let source = source.trim();
    let name = name.trim();
    if matches!(source, "systemd" | "systemd-user") {
        return false;
    }
    !source.is_empty() && source != "-" && !name.is_empty() && name != "-"
}

fn table_value<'a>(headers: &[String], row: &'a [String], name: &str) -> Option<&'a str> {
    let wanted = normalized_table_header(name);
    let index = headers
        .iter()
        .position(|header| normalized_table_header(header) == wanted)?;
    row.get(index).map(String::as_str)
}

fn startup_cell_is_actionable(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "-"
}

fn startup_action_payload(
    action: &str,
    scope: &str,
    source: &str,
    name: &str,
    command: &str,
) -> String {
    format!(
        "action={action}\nscope_b64={}\nsource_b64={}\nname_b64={}\ncommand_b64={}",
        STANDARD.encode(scope),
        STANDARD.encode(source),
        STANDARD.encode(name),
        STANDARD.encode(command)
    )
}

fn startup_add_payload(name: &str, command: &str) -> String {
    format!(
        "action=add\nscope=CurrentUser\nname_b64={}\ncommand_b64={}",
        STANDARD.encode(name.trim()),
        STANDARD.encode(command.trim())
    )
}

fn filtered_table_rows(table: &ResultTable, filter: &str) -> Vec<DisplayTableRow> {
    table
        .rows
        .iter()
        .enumerate()
        .filter(|row| {
            let row = row.1;
            filter.is_empty()
                || row
                    .iter()
                    .any(|cell| cell.to_ascii_lowercase().contains(filter))
        })
        .map(|(source_index, cells)| DisplayTableRow {
            source_index,
            cells: cells.clone(),
        })
        .collect()
}

fn sort_table_rows(rows: &mut [DisplayTableRow], sort: Option<TableSort>) {
    let Some(sort) = sort else {
        return;
    };
    rows.sort_by(|left, right| {
        let left_cell = left
            .cells
            .get(sort.column)
            .map(String::as_str)
            .unwrap_or("");
        let right_cell = right
            .cells
            .get(sort.column)
            .map(String::as_str)
            .unwrap_or("");
        let ordering = compare_table_cells(left_cell, right_cell)
            .then_with(|| left.source_index.cmp(&right.source_index));
        if sort.ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn compare_table_cells(left: &str, right: &str) -> std::cmp::Ordering {
    match (left.trim().parse::<f64>(), right.trim().parse::<f64>()) {
        (Ok(left), Ok(right)) => left
            .partial_cmp(&right)
            .unwrap_or(std::cmp::Ordering::Equal),
        _ => left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase()),
    }
}

fn table_row_key(row: &DisplayTableRow) -> String {
    format!("{}\t{}", row.source_index, row.cells.join("\t"))
}

fn command_result_table_id(
    command: &CommandKind,
    headers: &[String],
) -> (&'static str, &'static str, u64) {
    (
        "command_result_table_resizable",
        command.as_str(),
        stable_hash(&headers.join("\t")),
    )
}

fn table_cell_label(
    ui: &mut egui::Ui,
    text: &str,
    size: f32,
    color: egui::Color32,
    align: egui::Align,
    sense: egui::Sense,
) -> egui::Response {
    ui.add_sized(
        [ui.available_width(), ui.available_height()],
        egui::Label::new(egui::RichText::new(text).size(size).color(color))
            .selectable(false)
            .truncate()
            .halign(align)
            .sense(sense),
    )
}

fn process_row_pid(command: &CommandKind, headers: &[String], row: &[String]) -> Option<String> {
    if !matches!(
        command,
        CommandKind::ProcessManager | CommandKind::WindowManager
    ) {
        return None;
    }
    let pid_index = headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case("pid"))?;
    let pid = row.get(pid_index)?.trim();
    if pid != "0" && pid.chars().all(|ch| ch.is_ascii_digit()) {
        Some(pid.to_string())
    } else {
        None
    }
}

fn table_column_widths(
    command: &CommandKind,
    headers: &[String],
    rows: &[Vec<String>],
    available_width: f32,
) -> Vec<f32> {
    let specs = headers
        .iter()
        .map(|header| table_column_spec(command, header))
        .collect::<Vec<_>>();
    let mut widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            let spec = specs[index];
            let header_width = estimated_table_text_width(header) + TABLE_SORT_MARKER_WIDTH;
            let content_width = rows
                .iter()
                .take(TABLE_WIDTH_SAMPLE_ROWS)
                .filter_map(|row| row.get(index))
                .map(|cell| estimated_table_text_width(cell))
                .fold(0.0, f32::max);

            header_width.max(content_width).clamp(spec.min, spec.max)
        })
        .collect::<Vec<_>>();

    if available_width.is_finite() {
        distribute_extra_table_width(&mut widths, &specs, available_width);
    }

    widths
}

fn distribute_extra_table_width(
    widths: &mut [f32],
    specs: &[TableColumnSpec],
    available_width: f32,
) {
    let mut extra = available_width - widths.iter().sum::<f32>();
    while extra > 1.0 {
        let total_stretch = specs
            .iter()
            .enumerate()
            .filter(|(index, spec)| spec.stretch > 0.0 && widths[*index] < spec.max)
            .map(|(_, spec)| spec.stretch)
            .sum::<f32>();
        if total_stretch <= 0.0 {
            break;
        }

        let mut used = 0.0;
        for (width, spec) in widths.iter_mut().zip(specs.iter()) {
            if spec.stretch <= 0.0 || *width >= spec.max {
                continue;
            }

            let room = spec.max - *width;
            let grow = (extra * spec.stretch / total_stretch).min(room);
            *width += grow;
            used += grow;
        }

        if used <= 0.5 {
            break;
        }
        extra -= used;
    }
}

fn table_column_alignments(command: &CommandKind, headers: &[String]) -> Vec<egui::Align> {
    headers
        .iter()
        .map(|header| table_column_spec(command, header).align)
        .collect()
}

#[derive(Clone, Copy)]
struct TableColumnSpec {
    min: f32,
    max: f32,
    stretch: f32,
    align: egui::Align,
}

fn table_column_spec(command: &CommandKind, header: &str) -> TableColumnSpec {
    match command {
        CommandKind::ProcessManager => process_column_spec(header),
        CommandKind::WindowManager => window_column_spec(header),
        CommandKind::StartupManager => startup_column_spec(header),
        CommandKind::DriverManager => driver_column_spec(header),
        CommandKind::EventLog => event_log_column_spec(header),
        CommandKind::ActiveConnections => connection_column_spec(header),
        _ => default_column_spec(header),
    }
}

fn column_spec(min: f32, max: f32, stretch: f32, align: egui::Align) -> TableColumnSpec {
    TableColumnSpec {
        min,
        max,
        stretch,
        align,
    }
}

fn process_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "pid" | "ppid" => column_spec(42.0, 64.0, 0.0, egui::Align::Max),
        "cpu" | "pcpu" | "%cpu" | "mem" | "pmem" | "%mem" => {
            column_spec(48.0, 76.0, 0.0, egui::Align::Max)
        }
        "memorymb" => column_spec(70.0, 96.0, 0.0, egui::Align::Max),
        "name" | "processname" | "comm" => column_spec(110.0, 260.0, 1.0, egui::Align::Min),
        "command" => column_spec(180.0, 560.0, 3.0, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn window_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "windowid" => column_spec(78.0, 130.0, 0.0, egui::Align::Min),
        "desktop" | "pid" => column_spec(48.0, 74.0, 0.0, egui::Align::Max),
        "responding" | "visible" | "status" => column_spec(70.0, 96.0, 0.0, egui::Align::Min),
        "process" | "class" => column_spec(110.0, 220.0, 0.6, egui::Align::Min),
        "title" => column_spec(220.0, 620.0, 2.4, egui::Align::Min),
        "path" => column_spec(180.0, 520.0, 1.6, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn startup_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "scope" | "source" | "status" => column_spec(86.0, 150.0, 0.0, egui::Align::Min),
        "name" => column_spec(150.0, 320.0, 0.8, egui::Align::Min),
        "command" => column_spec(220.0, 720.0, 2.6, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn driver_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "index" | "refs" | "size" | "usedby" => column_spec(48.0, 80.0, 0.0, egui::Align::Max),
        "state" | "status" | "startmode" | "version" => {
            column_spec(74.0, 120.0, 0.0, egui::Align::Min)
        }
        "name" => column_spec(160.0, 360.0, 1.2, egui::Align::Min),
        "path" | "description" | "dependencies" => column_spec(220.0, 720.0, 2.2, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn event_log_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "time" | "timecreated" => column_spec(130.0, 190.0, 0.8, egui::Align::Min),
        "level" | "leveldisplayname" => column_spec(70.0, 115.0, 0.0, egui::Align::Min),
        "provider" | "providername" => column_spec(110.0, 260.0, 1.0, egui::Align::Min),
        "id" => column_spec(42.0, 70.0, 0.0, egui::Align::Max),
        "message" => column_spec(220.0, 720.0, 3.0, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn connection_column_spec(header: &str) -> TableColumnSpec {
    match normalized_table_header(header).as_str() {
        "proto" | "netid" | "protocol" => column_spec(48.0, 72.0, 0.0, egui::Align::Min),
        "local" | "localaddress" => column_spec(140.0, 320.0, 1.0, egui::Align::Min),
        "foreign" | "peer" | "peeraddress" | "foreignaddress" => {
            column_spec(140.0, 320.0, 1.0, egui::Align::Min)
        }
        "state" => column_spec(64.0, 120.0, 0.0, egui::Align::Min),
        "pid" => column_spec(42.0, 70.0, 0.0, egui::Align::Max),
        "pid/program" | "pid/programname" => column_spec(88.0, 180.0, 0.0, egui::Align::Min),
        _ => default_column_spec(header),
    }
}

fn default_column_spec(header: &str) -> TableColumnSpec {
    let key = normalized_table_header(header);
    if numeric_like_header(&key) {
        column_spec(48.0, 96.0, 0.0, egui::Align::Max)
    } else {
        column_spec(72.0, 240.0, 0.3, egui::Align::Min)
    }
}

fn normalized_table_header(header: &str) -> String {
    header
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_', '-'], "")
}

fn numeric_like_header(header: &str) -> bool {
    matches!(
        header,
        "id" | "pid" | "ppid" | "cpu" | "pcpu" | "%cpu" | "mem" | "pmem" | "%mem" | "memorymb"
    ) || header.ends_with("id")
        || header.ends_with("count")
        || header.ends_with("bytes")
        || header.ends_with("mb")
}

fn estimated_table_text_width(value: &str) -> f32 {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_whitespace() {
                3.5
            } else if ch.is_ascii() {
                6.7
            } else {
                11.0
            }
        })
        .sum::<f32>()
}

pub(super) fn command_title(command: &CommandKind) -> String {
    i18n::command_title(command).to_string()
}

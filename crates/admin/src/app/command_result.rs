use super::{
    event::AdminInput,
    payload::payload_field,
    ui::{
        COLOR_BAD, COLOR_BORDER, COLOR_GOOD, COLOR_MUTED, COLOR_PANEL, COLOR_TEXT, COLOR_WARN,
        TOOLBAR_CONTROL_HEIGHT,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
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
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_min_height(26.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(
                    egui::RichText::new(status_text)
                        .size(12.0)
                        .color(COLOR_TEXT)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(progress_text)
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
            });
        });
}

fn command_window_status(
    status: &CommandResultStatus,
) -> (&'static str, &'static str, egui::Color32) {
    match status {
        CommandResultStatus::Pending => ("Pending", "Waiting for client result", COLOR_WARN),
        CommandResultStatus::Accepted => ("Done", "Result received", COLOR_GOOD),
        CommandResultStatus::Failed => ("Failed", "Command failed", COLOR_BAD),
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
            return Some("Table data could not be parsed".to_string());
        }
        if matches!(command, CommandKind::PerformanceMonitor)
            && !detail.trim().is_empty()
            && parse_performance_metrics(detail).is_empty()
        {
            return Some("Performance metrics could not be parsed".to_string());
        }
        if command_allows_plain_detail(command, detail) {
            return Some("Result received".to_string());
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
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("Resource usage")
                    .size(12.0)
                    .color(COLOR_TEXT)
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
                    .color(COLOR_MUTED),
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
    .map(|value| percent_metric("CPU", value, egui::Color32::from_rgb(35, 99, 188)))
    .or_else(|| {
        let load = parse_load_average(detail)?;
        let cores = std::thread::available_parallelism()
            .map(|value| value.get() as f32)
            .unwrap_or(1.0)
            .max(1.0);
        Some(PerformanceMetric {
            label: "CPU Load",
            percent: clamp_percent(load * 100.0 / cores),
            value: format!("{load:.2} load"),
            color: egui::Color32::from_rgb(35, 99, 188),
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
    .map(|value| percent_metric("Memory", value, egui::Color32::from_rgb(24, 135, 84)))
}

fn parse_disk_metric(detail: &str) -> Option<PerformanceMetric> {
    parse_named_number(detail, &["disk_percent", "diskpercent", "DiskPercent"])
        .or_else(|| parse_df_disk_percent(detail))
        .map(|value| percent_metric("Disk", value, egui::Color32::from_rgb(179, 116, 28)))
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
                ui.label(egui::RichText::new("Filter").size(12.0).color(COLOR_MUTED));
            },
        );
        let response = ui.add_sized(
            [240.0, TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(&mut filter)
                .hint_text("Filter table content")
                .vertical_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(mut value) = table_filter.lock() {
                *value = filter.clone();
            }
        }
        let label = if refresh_in_flight {
            "Refreshing..."
        } else {
            "Refresh"
        };
        if ui
            .add_enabled(!refresh_in_flight, egui::Button::new(label))
            .clicked()
        {
            refresh_requested.store(true, Ordering::Relaxed);
        }
        if matches!(command, CommandKind::StartupManager) {
            if ui
                .add_enabled(!refresh_in_flight, egui::Button::new("Add Item"))
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
        egui::RichText::new(style.label)
            .size(12.0)
            .color(style.text),
    )
    .fill(style.fill)
    .stroke(egui::Stroke::new(1.0, style.stroke));

    let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        if ui.button("Enable").clicked() {
            queue_startup_action(startup_action_requested, "enable_client_autostart");
            ui.close();
        }
        if ui.button("Disable").clicked() {
            queue_startup_action(startup_action_requested, "disable_client_autostart");
            ui.close();
        }
    });
    response.on_hover_text("Configure login autostart for this client");
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
            .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
                    ui.label(egui::RichText::new("Name").size(12.0).color(COLOR_MUTED));
                    ui.add_sized(
                        [160.0, TOOLBAR_CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut form.name)
                            .hint_text("Item name")
                            .vertical_align(egui::Align::Center),
                    );
                    ui.label(egui::RichText::new("Command").size(12.0).color(COLOR_MUTED));
                    ui.add_sized(
                        [360.0, TOOLBAR_CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut form.command)
                            .hint_text("Command or executable path")
                            .vertical_align(egui::Align::Center),
                    );

                    let can_submit = !refresh_in_flight
                        && !form.name.trim().is_empty()
                        && !form.command.trim().is_empty();
                    if ui
                        .add_enabled(can_submit, egui::Button::new("Add"))
                        .clicked()
                    {
                        queued_payload = Some(startup_add_payload(&form.name, &form.command));
                        form.name.clear();
                        form.command.clear();
                        form.error.clear();
                        form.open = false;
                    }
                    if ui.button("Cancel").clicked() {
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
                        egui::RichText::new("Name and command are required.")
                            .size(12.0)
                            .color(COLOR_MUTED),
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
            "Refreshing..."
        } else {
            "Refresh"
        };
        if ui
            .add_enabled(!refresh_in_flight, egui::Button::new(label))
            .clicked()
        {
            refresh_requested.store(true, Ordering::Relaxed);
        }
        if ui
            .checkbox(&mut auto_refresh, "Auto refresh (5s)")
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
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .corner_radius(6.0)
        .show(ui, |ui| {
            let mut table_builder = TableBuilder::new(ui)
                .id_salt(command_result_table_id(command, &table.headers))
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
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
                                COLOR_MUTED,
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
                                    COLOR_TEXT,
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
                                if ui.button("Copy Cell").clicked() {
                                    ui.ctx().copy_text(cell_text.clone());
                                    ui.close();
                                }
                                if ui.button("Copy Row").clicked() {
                                    ui.ctx().copy_text(row_text.clone());
                                    ui.close();
                                }
                                if let Some(process_id) = process_id.clone() {
                                    ui.separator();
                                    if ui.button("Kill Process").clicked() {
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
                                    if ui.button(startup_action.label).clicked() {
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
                                    if ui.button("Delete Startup Item").clicked() {
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
            egui::RichText::new("No rows match the current filter.")
                .size(12.0)
                .color(COLOR_MUTED),
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
    match status {
        StartupClientAutostartStatus::Enabled => StartupClientAutostartStyle {
            label: "Client Autostart: On",
            fill: egui::Color32::from_rgb(224, 246, 235),
            stroke: COLOR_BORDER,
            text: COLOR_GOOD,
        },
        StartupClientAutostartStatus::Disabled => StartupClientAutostartStyle {
            label: "Client Autostart: Off",
            fill: egui::Color32::from_rgb(255, 238, 238),
            stroke: COLOR_BORDER,
            text: COLOR_BAD,
        },
        StartupClientAutostartStatus::Unknown => StartupClientAutostartStyle {
            label: "Client Autostart: Unknown",
            fill: egui::Color32::from_rgb(243, 246, 250),
            stroke: COLOR_BORDER,
            text: COLOR_MUTED,
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
    command
        .as_str()
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn process_table_keeps_numeric_columns_compact() {
        let headers = strings(["PID", "Name", "CPU", "MemoryMB"]);
        let rows = vec![
            strings(["7", "launchd", "0.0", "13.5"]),
            strings(["12345", "very-long-process-name", "12.3", "1024.0"]),
        ];

        let widths = table_column_widths(&CommandKind::ProcessManager, &headers, &rows, 760.0);

        assert!(widths[0] <= 64.0);
        assert!(widths[2] <= 76.0);
        assert!(widths[3] <= 96.0);
        assert!(widths[1] > widths[0]);
    }

    #[test]
    fn process_table_ignores_infinite_scroll_width() {
        let headers = strings(["PID", "Name", "CPU"]);
        let rows = vec![strings(["1", "init", "0.0"])];

        let widths =
            table_column_widths(&CommandKind::ProcessManager, &headers, &rows, f32::INFINITY);

        assert!(widths.iter().all(|width| width.is_finite()));
        assert!(widths[0] <= 64.0);
    }

    #[test]
    fn detail_status_reads_session_result_status() {
        assert_eq!(
            detail_status("delete_client\nstatus=scheduled\nmessage=ok").as_deref(),
            Some("scheduled")
        );
        assert_eq!(
            detail_status("delete_client\nstatus=dry_run\nmessage=ok").as_deref(),
            Some("dry_run")
        );
    }

    #[test]
    fn table_row_keys_stay_unique_for_duplicate_display_values() {
        let table = ResultTable {
            headers: strings(["Time", "Level", "Message"]),
            rows: vec![
                strings(["2026-05-16 12:00:00", "Info", "same"]),
                strings(["2026-05-16 12:00:00", "Info", "same"]),
            ],
        };

        let rows = filtered_table_rows(&table, "");

        assert_eq!(rows.len(), 2);
        assert_ne!(table_row_key(&rows[0]), table_row_key(&rows[1]));
    }

    #[test]
    fn table_row_keys_stay_unique_after_sorting_duplicate_times() {
        let table = ResultTable {
            headers: strings(["Time", "Level", "Message"]),
            rows: vec![
                strings(["2026-05-16 12:00:00", "Info", "first"]),
                strings(["2026-05-16 12:00:00", "Warn", "second"]),
                strings(["2026-05-16 12:01:00", "Info", "third"]),
            ],
        };
        let mut rows = filtered_table_rows(&table, "");

        sort_table_rows(
            &mut rows,
            Some(TableSort {
                column: 0,
                ascending: true,
            }),
        );
        let keys = rows.iter().map(table_row_key).collect::<Vec<_>>();
        let unique = keys.iter().collect::<std::collections::HashSet<_>>();

        assert_eq!(keys.len(), unique.len());
    }

    #[test]
    fn window_manager_rows_can_request_process_kill() {
        let headers = strings(["PID", "Process", "Title"]);
        let row = strings(["1234", "Terminal", "shell"]);

        assert_eq!(
            process_row_pid(&CommandKind::WindowManager, &headers, &row).as_deref(),
            Some("1234")
        );
    }

    #[test]
    fn process_kill_action_ignores_pid_zero_rows() {
        let headers = strings(["PID", "Process", "Title"]);
        let row = strings(["0", "Info", "No windows"]);

        assert!(process_row_pid(&CommandKind::WindowManager, &headers, &row).is_none());
        assert!(process_row_pid(&CommandKind::ProcessManager, &headers, &row).is_none());
    }

    #[test]
    fn startup_enabled_rows_request_disable_action() {
        let headers = strings(["Scope", "Source", "Name", "Command", "Status"]);
        let row = strings([
            "CurrentUser",
            "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "App",
            "C:\\App\\app.exe",
            "Enabled",
        ]);

        let action = startup_row_action(&CommandKind::StartupManager, &headers, &row)
            .expect("startup row should be actionable");

        assert_eq!(action.label, "Disable Startup Item");
        assert!(action.payload.contains("action=disable"));
        assert!(action
            .payload
            .contains(&format!("name_b64={}", STANDARD.encode("App"))));
    }

    #[test]
    fn startup_disabled_rows_request_enable_action() {
        let headers = strings(["Scope", "Source", "Name", "Command", "Status"]);
        let row = strings([
            "CurrentUser",
            "/Users/me/Library/LaunchAgents",
            "app.plist.disabled",
            "/Users/me/Library/LaunchAgents/app.plist.disabled",
            "Disabled",
        ]);

        let action = startup_row_action(&CommandKind::StartupManager, &headers, &row)
            .expect("startup row should be actionable");

        assert_eq!(action.label, "Enable Startup Item");
        assert!(action.payload.contains("action=enable"));
    }

    #[test]
    fn startup_client_autostart_status_detects_enabled_run_value() {
        let table = ResultTable {
            headers: strings(["Scope", "Source", "Name", "Command", "Status"]),
            rows: vec![strings([
                "CurrentUser",
                "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "RustDeskLightClient",
                "C:\\Users\\me\\AppData\\Local\\rust-desk-light\\rdl-client.exe",
                "Enabled",
            ])],
        };

        assert_eq!(
            startup_client_autostart_status(&table),
            StartupClientAutostartStatus::Enabled
        );
    }

    #[test]
    fn startup_client_autostart_status_detects_linux_systemd_service() {
        let table = ResultTable {
            headers: strings(["Scope", "Source", "Name", "Command", "Status"]),
            rows: vec![strings([
                "System",
                "systemd",
                "rust-desk-light-client.service",
                "-",
                "Enabled",
            ])],
        };

        assert_eq!(
            startup_client_autostart_status(&table),
            StartupClientAutostartStatus::Enabled
        );
    }

    #[test]
    fn startup_client_autostart_status_treats_missing_row_as_disabled() {
        let table = ResultTable {
            headers: strings(["Scope", "Source", "Name", "Command", "Status"]),
            rows: vec![strings([
                "CurrentUser",
                "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "OtherApp",
                "C:\\App\\app.exe",
                "Enabled",
            ])],
        };

        assert_eq!(
            startup_client_autostart_status(&table),
            StartupClientAutostartStatus::Disabled
        );
    }

    #[test]
    fn table_parse_failures_only_report_status_notice() {
        assert_eq!(
            command_status_notice(
                &CommandKind::ProcessManager,
                CommandResultStatus::Accepted,
                "not a table"
            )
            .as_deref(),
            Some("Table data could not be parsed")
        );
    }

    #[test]
    fn manager_commands_expect_table_results() {
        for command in [
            CommandKind::WindowManager,
            CommandKind::StartupManager,
            CommandKind::RegistryManager,
            CommandKind::DriverManager,
        ] {
            assert!(command_expects_result_table(&command));
            assert!(!command_allows_plain_detail(&command, "not a table"));
        }
    }

    #[test]
    fn plain_detail_is_suppressed_for_table_commands() {
        assert!(!command_allows_plain_detail(
            &CommandKind::EventLog,
            "not a table"
        ));
    }

    #[test]
    fn performance_monitor_keeps_result_detail_but_hides_pending_placeholders() {
        assert!(command_allows_plain_detail(
            &CommandKind::PerformanceMonitor,
            "performance_snapshot:\ncpu_percent=12.5"
        ));
        assert!(performance_monitor_pending_detail(
            "Auto refreshing performance monitor..."
        ));
        assert_eq!(
            command_status_notice(
                &CommandKind::PerformanceMonitor,
                CommandResultStatus::Accepted,
                "not metrics"
            )
            .as_deref(),
            Some("Performance metrics could not be parsed")
        );
    }

    #[test]
    fn performance_refresh_preserves_existing_result_detail() {
        let now = Instant::now();
        let mut window = test_command_window(CommandKind::PerformanceMonitor);
        window.detail = "performance_snapshot:\ncpu_percent=12.5".to_string();
        let (input_tx, _input_rx) = std::sync::mpsc::sync_channel(1);
        let mut pending_logs = Vec::new();

        refresh_command_window(
            &input_tx,
            &mut window,
            "Auto refreshing performance monitor...",
            "auto_refresh",
            now,
            &mut pending_logs,
        );

        assert!(matches!(window.status, CommandResultStatus::Pending));
        assert_eq!(
            window.detail,
            "performance_snapshot:\ncpu_percent=12.5".to_string()
        );
        assert_eq!(window.last_auto_refresh_at, Some(now));
    }

    #[test]
    fn plain_detail_is_available_for_regular_command_results() {
        assert!(command_allows_plain_detail(
            &CommandKind::ComputerInfo,
            "computer_info:\nhostname=test"
        ));
        assert_eq!(
            command_status_notice(
                &CommandKind::ComputerInfo,
                CommandResultStatus::Accepted,
                "computer_info:\nhostname=test"
            )
            .as_deref(),
            Some("Result received")
        );
    }

    #[test]
    fn camera_text_errors_fall_back_to_plain_detail() {
        assert!(command_allows_plain_detail(
            &CommandKind::Camera,
            "camera_error\nmessage=no device"
        ));
    }

    #[test]
    fn camera_frames_do_not_show_raw_base64_detail() {
        assert!(!command_allows_plain_detail(
            &CommandKind::Camera,
            "camera_frame\nimage_base64=abcd"
        ));
    }

    #[test]
    fn performance_metrics_parse_structured_percent_fields() {
        let detail =
            "performance_snapshot:\ncpu_percent=12.5\nmemory_percent=45.0\ndisk_percent=67.2";

        let metrics = parse_performance_metrics(detail);

        assert_metric(metrics.cpu.as_ref(), "CPU", 12.5);
        assert_metric(metrics.memory.as_ref(), "Memory", 45.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 67.2);
    }

    #[test]
    fn performance_metrics_parse_windows_format_list() {
        let detail = "\
Cpu           : Intel
LoadPercent   : 23
TotalMemoryMB : 16000
FreeMemoryMB  : 4000
DiskPercent   : 55";

        let metrics = parse_performance_metrics(detail);

        assert_metric(metrics.cpu.as_ref(), "CPU", 23.0);
        assert_metric(metrics.memory.as_ref(), "Memory", 75.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 55.0);
    }

    #[test]
    fn performance_metrics_parse_unix_command_output() {
        let detail = "\
performance_snapshot:
 18:19:21 up 1 day,  4:51,  load average: 1.50, 1.20, 0.90
              total        used        free      shared  buff/cache   available
Mem:           1000         250         500          10         250         700
Filesystem     1024-blocks Used Available Capacity Mounted on
/dev/disk1       100000000 4200  5800     42%      .";

        let metrics = parse_performance_metrics(detail);

        assert_eq!(
            metrics.cpu.as_ref().map(|metric| metric.label),
            Some("CPU Load")
        );
        assert_eq!(
            metrics.cpu.as_ref().map(|metric| metric.value.as_str()),
            Some("1.50 load")
        );
        assert_metric(metrics.memory.as_ref(), "Memory", 25.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 42.0);
    }

    #[test]
    fn performance_metrics_parse_macos_vm_stat_memory() {
        let detail = "\
performance_snapshot:
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               100.
Pages active:                             100.
Pages inactive:                           100.
Pages speculative:                          0.
Pages wired down:                         100.
Pages occupied by compressor:             100.
Filesystem 512-blocks Used Available Capacity iused ifree %iused Mounted on
/dev/disk3s1 1000000 610000 390000 61% 1 2 1% .";

        let metrics = parse_performance_metrics(detail);

        assert_metric(metrics.memory.as_ref(), "Memory", 80.0);
        assert_metric(metrics.disk.as_ref(), "Disk", 61.0);
    }

    #[test]
    fn performance_auto_refresh_waits_for_interval() {
        let now = Instant::now();
        let mut window = test_command_window(CommandKind::PerformanceMonitor);
        window.auto_refresh_enabled.store(true, Ordering::Relaxed);

        assert!(!performance_auto_refresh_due(&mut window, now));
        assert!(!performance_auto_refresh_due(
            &mut window,
            now + PERFORMANCE_AUTO_REFRESH_INTERVAL - Duration::from_millis(1)
        ));
        assert!(performance_auto_refresh_due(
            &mut window,
            now + PERFORMANCE_AUTO_REFRESH_INTERVAL
        ));
    }

    #[test]
    fn performance_auto_refresh_stops_when_window_closes() {
        let now = Instant::now();
        let mut window = test_command_window(CommandKind::PerformanceMonitor);
        window.auto_refresh_enabled.store(true, Ordering::Relaxed);
        window.last_auto_refresh_at = Some(now - PERFORMANCE_AUTO_REFRESH_INTERVAL);
        window.open = false;

        assert!(!performance_auto_refresh_due(&mut window, now));
        assert!(window.last_auto_refresh_at.is_none());
    }

    fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
        values.into_iter().map(str::to_string).collect()
    }

    fn assert_metric(metric: Option<&PerformanceMetric>, label: &str, percent: f32) {
        let metric = metric.expect("metric should parse");
        assert_eq!(metric.label, label);
        assert!(
            (metric.percent - percent).abs() < 0.1,
            "expected {percent}, got {}",
            metric.percent
        );
    }

    fn test_command_window(command: CommandKind) -> CommandResultWindow {
        CommandResultWindow {
            id: 1,
            client_id: "client".to_string(),
            hostname: "host".to_string(),
            username: "user".to_string(),
            command,
            status: CommandResultStatus::Accepted,
            detail: String::new(),
            open: true,
            close_requested: Arc::new(AtomicBool::new(false)),
            refresh_requested: Arc::new(AtomicBool::new(false)),
            auto_refresh_enabled: Arc::new(AtomicBool::new(false)),
            last_auto_refresh_at: None,
            process_kill_requested: Arc::new(Mutex::new(None)),
            startup_action_requested: Arc::new(Mutex::new(None)),
            registry_key_requested: Arc::new(Mutex::new(None)),
            startup_add_form: Arc::new(Mutex::new(StartupAddForm::default())),
            table_filter: Arc::new(Mutex::new(String::new())),
            table_sort: Arc::new(Mutex::new(None)),
            table_selected_row: Arc::new(Mutex::new(None)),
        }
    }
}

use super::ui;
use crate::i18n::t;
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use egui_extras::Column;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const ACTION_LIST: &str = "list";
const ACTION_CREATE: &str = "create";
const ACTION_DELETE: &str = "delete";
const ACTION_ENABLE: &str = "enable";
const ACTION_DISABLE: &str = "disable";
const ACTION_RUN: &str = "run";
const TRIGGER_STARTUP: &str = "startup";
const TRIGGER_DAILY: &str = "daily";

#[derive(Clone)]
pub(super) struct TaskManagerState {
    name: Arc<Mutex<String>>,
    command: Arc<Mutex<String>>,
    trigger: Arc<Mutex<String>>,
    time: Arc<Mutex<String>>,
    selected: Arc<Mutex<String>>,
    action: Arc<Mutex<String>>,
}

impl Default for TaskManagerState {
    fn default() -> Self {
        Self {
            name: Arc::new(Mutex::new("rdl-task".to_string())),
            command: Arc::new(Mutex::new(String::new())),
            trigger: Arc::new(Mutex::new(TRIGGER_STARTUP.to_string())),
            time: Arc::new(Mutex::new("09:00".to_string())),
            selected: Arc::new(Mutex::new(String::new())),
            action: Arc::new(Mutex::new(ACTION_LIST.to_string())),
        }
    }
}

impl TaskManagerState {
    pub(super) fn queue_refresh(&self, send_requested: &Arc<AtomicBool>) {
        queue_action(&self.action, send_requested, ACTION_LIST);
    }

    pub(super) fn payload(&self) -> String {
        payload_for(
            &lock_string(&self.action),
            &lock_string(&self.selected),
            &lock_string(&self.name),
            &lock_string(&self.command),
            &lock_string(&self.trigger),
            &lock_string(&self.time),
        )
    }
}

pub(super) fn render(
    ui: &mut egui::Ui,
    state: &TaskManagerState,
    result_detail: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
) {
    let detail = result_detail
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let rows = parse_task_rows(&detail);
    render_manager_toolbar(
        ui,
        &state.selected,
        &state.action,
        send_requested,
        !rows.is_empty(),
    );
    ui.add_space(crate::theme::SECTION_GAP);
    render_task_table(ui, &rows, state);
    ui.add_space(crate::theme::SECTION_GAP);
    ui.separator();
    ui.add_space(crate::theme::SECTION_GAP);
    render_create_form(ui, state, send_requested);
}

fn render_manager_toolbar(
    ui: &mut egui::Ui,
    task_selected: &Arc<Mutex<String>>,
    task_action: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
    has_rows: bool,
) {
    let selected = selected_task(task_selected);
    let has_selected = !selected.is_empty();
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = crate::theme::COMPACT_CONTROL_HEIGHT;
        if ui.button(t("Refresh")).clicked() {
            queue_action(task_action, send_requested, ACTION_LIST);
        }
        ui.separator();
        if ui
            .add_enabled(has_selected, egui::Button::new(t("Run Task")))
            .clicked()
        {
            queue_action(task_action, send_requested, ACTION_RUN);
        }
        if ui
            .add_enabled(has_selected, egui::Button::new(t("Enable")))
            .clicked()
        {
            queue_action(task_action, send_requested, ACTION_ENABLE);
        }
        if ui
            .add_enabled(has_selected, egui::Button::new(t("Disable")))
            .clicked()
        {
            queue_action(task_action, send_requested, ACTION_DISABLE);
        }
        if ui
            .add_enabled(has_selected, egui::Button::new(t("Delete")))
            .clicked()
        {
            queue_action(task_action, send_requested, ACTION_DELETE);
        }
        ui.separator();
        let label = if has_selected {
            format!("{}: {selected}", t("Selected"))
        } else if has_rows {
            t("not selected").to_string()
        } else {
            t("No managed tasks").to_string()
        };
        ui.label(crate::theme::muted_text(label));
    });
}

fn render_task_table(ui: &mut egui::Ui, rows: &[TaskRow], state: &TaskManagerState) {
    let selected = selected_task(&state.selected);
    let table_height = (ui.available_height() * 0.48).clamp(150.0, 260.0);
    egui::Frame::default()
        .fill(crate::theme::palette().panel)
        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
        .corner_radius(6.0)
        .inner_margin(crate::theme::PANEL_MARGIN)
        .show(ui, |ui| {
            if rows.is_empty() {
                ui.set_min_height(table_height);
                ui.centered_and_justified(|ui| {
                    ui.label(crate::theme::muted_text(t("No managed tasks")));
                });
                return;
            }

            crate::theme::clickable_table(ui, "task_manager_table", true)
                .max_scroll_height(table_height)
                .column(Column::initial(150.0).at_least(110.0))
                .column(Column::initial(90.0).at_least(72.0))
                .column(Column::initial(92.0).at_least(72.0))
                .column(Column::initial(92.0).at_least(72.0))
                .column(Column::remainder().at_least(220.0))
                .header(crate::theme::TABLE_HEADER_HEIGHT, |mut header| {
                    header.col(|ui| table_header(ui, t("Name")));
                    header.col(|ui| table_header(ui, t("Trigger")));
                    header.col(|ui| table_header(ui, t("Schedule")));
                    header.col(|ui| table_header(ui, t("Status")));
                    header.col(|ui| table_header(ui, t("Command")));
                })
                .body(|mut body| {
                    for row in rows {
                        let is_selected = selected == row.name;
                        body.row(crate::theme::TABLE_ROW_HEIGHT, |mut table_row| {
                            table_row.set_selected(is_selected);
                            table_row.col(|ui| table_cell(ui, &row.name));
                            table_row.col(|ui| table_cell(ui, &trigger_label(&row.trigger)));
                            table_row.col(|ui| table_cell(ui, &row.schedule));
                            table_row.col(|ui| table_cell(ui, &task_status_label(&row.status)));
                            table_row.col(|ui| table_cell(ui, &row.command));
                            if table_row.response().clicked() {
                                select_task(row, state);
                            }
                        });
                    }
                });
        });
}

fn render_create_form(
    ui: &mut egui::Ui,
    state: &TaskManagerState,
    send_requested: &Arc<AtomicBool>,
) {
    ui.label(crate::theme::strong_body_text(t("Create or update task")));
    ui.add_space(crate::theme::SECTION_GAP);
    ui::render_text_field(ui, t("Task Name"), &state.name, "rdl-task");
    ui.add_space(crate::theme::SECTION_GAP);
    ui::render_text_field(
        ui,
        t("Command"),
        &state.command,
        t("Command or executable path"),
    );
    ui.add_space(crate::theme::SECTION_GAP);
    render_trigger(ui, &state.trigger, &state.time);
    ui.add_space(crate::theme::PANEL_MARGIN);

    let disabled_message = create_disabled_message(state);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = ui::TOOLBAR_CONTROL_HEIGHT;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    disabled_message.is_empty(),
                    egui::Button::new(t("Save Task")),
                )
                .clicked()
            {
                queue_action(&state.action, send_requested, ACTION_CREATE);
            }
            if !disabled_message.is_empty() {
                ui.label(
                    egui::RichText::new(disabled_message)
                        .size(12.0)
                        .color(crate::theme::palette().text),
                );
            }
        });
    });
}

fn render_trigger(
    ui: &mut egui::Ui,
    task_trigger: &Arc<Mutex<String>>,
    task_time: &Arc<Mutex<String>>,
) {
    let mut selected = task_trigger
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| TRIGGER_STARTUP.to_string());
    if selected.is_empty() {
        selected = TRIGGER_STARTUP.to_string();
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = ui::TOOLBAR_CONTROL_HEIGHT;
        ui::render_inline_label(ui, t("Trigger"));
        egui::ComboBox::from_id_salt(("task_manager_trigger", Arc::as_ptr(task_trigger)))
            .width(180.0)
            .selected_text(trigger_label(&selected))
            .show_ui(ui, |ui| {
                for (value, label) in trigger_options() {
                    if ui.selectable_label(selected == value, label).clicked() {
                        selected = value.to_string();
                        if let Ok(mut target) = task_trigger.lock() {
                            *target = selected.clone();
                        }
                    }
                }
            });
    });

    if selected == TRIGGER_DAILY {
        ui.add_space(crate::theme::SECTION_GAP);
        ui::render_inline_text_field(ui, t("Start Time"), task_time, "09:00");
    }
}

fn payload_for(
    action: &str,
    selected: &str,
    name: &str,
    command: &str,
    trigger: &str,
    time: &str,
) -> String {
    let action = sanitize_single_line(action);
    match action.as_str() {
        ACTION_LIST => "action=list".to_string(),
        ACTION_DELETE | ACTION_ENABLE | ACTION_DISABLE | ACTION_RUN => {
            format!(
                "action={}\nname={}",
                action,
                sanitize_single_line(if selected.trim().is_empty() {
                    name
                } else {
                    selected
                })
            )
        }
        _ => create_payload(name, command, trigger, time),
    }
}

fn create_payload(name: &str, command: &str, trigger: &str, time: &str) -> String {
    let trigger = if trigger.trim() == TRIGGER_DAILY {
        TRIGGER_DAILY
    } else {
        TRIGGER_STARTUP
    };
    let mut lines = vec![
        "action=create".to_string(),
        format!("name={}", sanitize_single_line(name)),
        format!("trigger={trigger}"),
        format!("command_b64={}", STANDARD.encode(command)),
    ];
    if trigger == TRIGGER_DAILY {
        lines.push(format!("time={}", sanitize_single_line(time)));
    }
    lines.join("\n")
}

fn create_disabled_message(state: &TaskManagerState) -> &'static str {
    let name_missing = state
        .name
        .lock()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true);
    let command_missing = state
        .command
        .lock()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true);
    let time_invalid = state
        .trigger
        .lock()
        .map(|value| value.as_str() == TRIGGER_DAILY)
        .unwrap_or(false)
        && state
            .time
            .lock()
            .map(|value| !valid_hhmm(&value))
            .unwrap_or(true);
    if name_missing {
        t("Task name is required")
    } else if command_missing {
        t("Command is required")
    } else if time_invalid {
        t("Time must be HH:MM")
    } else {
        ""
    }
}

fn queue_action(
    task_action: &Arc<Mutex<String>>,
    send_requested: &Arc<AtomicBool>,
    action: &'static str,
) {
    if let Ok(mut target) = task_action.lock() {
        *target = action.to_string();
    }
    send_requested.store(true, Ordering::Relaxed);
}

fn select_task(row: &TaskRow, state: &TaskManagerState) {
    if let Ok(mut target) = state.selected.lock() {
        *target = row.name.clone();
    }
    if let Ok(mut target) = state.name.lock() {
        *target = row.name.clone();
    }
    if let Ok(mut target) = state.command.lock() {
        *target = row.command.clone();
    }
    if let Ok(mut target) = state.trigger.lock() {
        *target = if row.trigger == TRIGGER_DAILY {
            TRIGGER_DAILY.to_string()
        } else {
            TRIGGER_STARTUP.to_string()
        };
    }
    if row.trigger == TRIGGER_DAILY && row.schedule != "-" {
        if let Ok(mut target) = state.time.lock() {
            *target = row.schedule.clone();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TaskRow {
    name: String,
    trigger: String,
    schedule: String,
    status: String,
    command: String,
}

fn parse_task_rows(detail: &str) -> Vec<TaskRow> {
    detail
        .lines()
        .skip_while(|line| !line.starts_with("Name\t"))
        .skip(1)
        .filter_map(parse_task_row)
        .collect()
}

fn parse_task_row(line: &str) -> Option<TaskRow> {
    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    Some(TaskRow {
        name: parts[0].trim().to_string(),
        trigger: parts[1].trim().to_string(),
        schedule: parts[2].trim().to_string(),
        status: parts[3].trim().to_string(),
        command: parts[4..].join("\t").trim().to_string(),
    })
}

fn selected_task(task_selected: &Arc<Mutex<String>>) -> String {
    task_selected
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default()
}

fn lock_string(value: &Arc<Mutex<String>>) -> String {
    value.lock().map(|value| value.clone()).unwrap_or_default()
}

fn trigger_options() -> [(&'static str, &'static str); 2] {
    [
        (TRIGGER_STARTUP, t("At startup")),
        (TRIGGER_DAILY, t("Daily")),
    ]
}

fn trigger_label(value: &str) -> String {
    match value {
        TRIGGER_DAILY => t("Daily").to_string(),
        TRIGGER_STARTUP => t("At startup").to_string(),
        _ => value.to_string(),
    }
}

fn task_status_label(value: &str) -> String {
    match value {
        "enabled" | "Ready" => t("Enabled").to_string(),
        "disabled" | "Disabled" => t("Disabled").to_string(),
        "Running" => t("Running").to_string(),
        _ => value.to_string(),
    }
}

fn table_header(ui: &mut egui::Ui, label: &str) {
    ui.label(crate::theme::muted_text(label).strong());
}

fn table_cell(ui: &mut egui::Ui, value: &str) {
    ui.label(crate::theme::body_text(value));
}

fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

fn valid_hhmm(value: &str) -> bool {
    let Some((hour, minute)) = value.trim().split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().map(|value| value <= 23).unwrap_or(false)
        && minute
            .parse::<u8>()
            .map(|value| value <= 59)
            .unwrap_or(false)
}

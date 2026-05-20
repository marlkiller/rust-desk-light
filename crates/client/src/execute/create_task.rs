use super::shared::{clean_value, payload_field, run_shell};
use base64::{engine::general_purpose::STANDARD, Engine};
#[cfg(unix)]
use std::io::Write;
use std::process::{Command, Stdio};

const WINDOWS_TASK_PREFIX: &str = "RustDeskLight-";
#[cfg(unix)]
const CRON_MARKER_PREFIX: &str = "# rdl-task:";
#[cfg(unix)]
const CRON_DISABLED_PREFIX: &str = "# disabled: ";

pub(super) fn handle(payload: &str) -> String {
    let action = payload_field(payload, "action").unwrap_or_else(|| "list".to_string());
    let result = match action.as_str() {
        "list" | "" => list_tasks().map(|tasks| list_result("list", "tasks refreshed", tasks)),
        "create" => create_task(payload),
        "delete" => named_task_action(payload, "delete", delete_task),
        "enable" => named_task_action(payload, "enable", enable_task),
        "disable" => named_task_action(payload, "disable", disable_task),
        "run" => named_task_action(payload, "run", run_task),
        other => Err(format!("unsupported task action: {other}")),
    };

    match result {
        Ok(detail) => detail,
        Err(message) => format!(
            "create_task\nstatus=failed\naction={}\nmessage={}",
            clean_value(&action),
            clean_value(&message)
        ),
    }
}

fn create_task(payload: &str) -> Result<String, String> {
    let request = TaskRequest::parse(payload)?;
    create_platform_task(&request)?;
    list_tasks()
        .map(|tasks| list_result("create", &format!("task {} created", request.name), tasks))
}

fn named_task_action(
    payload: &str,
    action: &str,
    apply: fn(&str) -> Result<String, String>,
) -> Result<String, String> {
    let name = payload_field(payload, "name")
        .map(|value| sanitize_single_line(&value))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "task name is required".to_string())?;
    let message = apply(&name)?;
    list_tasks().map(|tasks| list_result(action, &message, tasks))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TaskRequest {
    name: String,
    command: String,
    trigger: TaskTrigger,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TaskTrigger {
    Startup,
    Daily { time: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManagedTask {
    name: String,
    trigger: String,
    schedule: String,
    status: String,
    command: String,
}

impl TaskRequest {
    fn parse(payload: &str) -> Result<Self, String> {
        let name = payload_field(payload, "name")
            .map(|value| sanitize_single_line(&value))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "task name is required".to_string())?;
        let command = payload_field(payload, "command_b64")
            .and_then(|value| STANDARD.decode(value).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .or_else(|| payload_field(payload, "command"))
            .map(|value| sanitize_single_line(&value))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "command is required".to_string())?;
        let trigger = match payload_field(payload, "trigger").as_deref() {
            Some("daily") => {
                let time = payload_field(payload, "time")
                    .map(|value| sanitize_single_line(&value))
                    .filter(|value| valid_hhmm(value))
                    .ok_or_else(|| "daily task time must be HH:MM".to_string())?;
                TaskTrigger::Daily { time }
            }
            Some("startup") | None | Some("") => TaskTrigger::Startup,
            Some(other) => return Err(format!("unsupported trigger: {other}")),
        };
        Ok(Self {
            name,
            command,
            trigger,
        })
    }
}

fn create_platform_task(request: &TaskRequest) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        create_windows_task(request)
    } else if cfg!(unix) {
        create_cron_task(request)
    } else {
        Err("task management is not supported on this platform".to_string())
    }
}

fn list_tasks() -> Result<Vec<ManagedTask>, String> {
    if cfg!(target_os = "windows") {
        list_windows_tasks()
    } else if cfg!(unix) {
        list_cron_tasks()
    } else {
        Err("task management is not supported on this platform".to_string())
    }
}

fn delete_task(name: &str) -> Result<String, String> {
    if cfg!(target_os = "windows") {
        run_schtasks(&["/Delete", "/F", "/TN", &windows_task_name(name)])?;
    } else if cfg!(unix) {
        update_crontab_entry(name, CronEntryAction::Delete)?;
    } else {
        return Err("task management is not supported on this platform".to_string());
    }
    Ok(format!("task {name} deleted"))
}

fn enable_task(name: &str) -> Result<String, String> {
    if cfg!(target_os = "windows") {
        run_schtasks(&["/Change", "/ENABLE", "/TN", &windows_task_name(name)])?;
    } else if cfg!(unix) {
        update_crontab_entry(name, CronEntryAction::Enable)?;
    } else {
        return Err("task management is not supported on this platform".to_string());
    }
    Ok(format!("task {name} enabled"))
}

fn disable_task(name: &str) -> Result<String, String> {
    if cfg!(target_os = "windows") {
        run_schtasks(&["/Change", "/DISABLE", "/TN", &windows_task_name(name)])?;
    } else if cfg!(unix) {
        update_crontab_entry(name, CronEntryAction::Disable)?;
    } else {
        return Err("task management is not supported on this platform".to_string());
    }
    Ok(format!("task {name} disabled"))
}

fn run_task(name: &str) -> Result<String, String> {
    if cfg!(target_os = "windows") {
        run_schtasks(&["/Run", "/TN", &windows_task_name(name)])?;
        Ok(format!("task {name} started"))
    } else if cfg!(unix) {
        let task = list_cron_tasks()?
            .into_iter()
            .find(|task| task.name == name)
            .ok_or_else(|| format!("task not found: {name}"))?;
        let output = run_shell(&task.command);
        if output.lines().any(|line| line == "status=failed") {
            Err(format!("task command failed: {}", clean_value(&output)))
        } else {
            Ok(format!("task {name} executed"))
        }
    } else {
        Err("task management is not supported on this platform".to_string())
    }
}

#[cfg(target_os = "windows")]
fn create_windows_task(request: &TaskRequest) -> Result<(), String> {
    let task_name = windows_task_name(&request.name);
    let mut args = vec!["/Create", "/F", "/TN", &task_name, "/TR", &request.command];
    let time;
    match &request.trigger {
        TaskTrigger::Startup => {
            args.push("/SC");
            args.push("ONLOGON");
        }
        TaskTrigger::Daily { time: task_time } => {
            time = task_time.clone();
            args.push("/SC");
            args.push("DAILY");
            args.push("/ST");
            args.push(&time);
        }
    }
    run_schtasks(&args).map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn create_windows_task(_request: &TaskRequest) -> Result<(), String> {
    Err("Windows scheduled tasks are not supported on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn list_windows_tasks() -> Result<Vec<ManagedTask>, String> {
    let script = format!(
        "$ErrorActionPreference='Stop'; Get-ScheduledTask | Where-Object {{$_.TaskName -like '{}*'}} | ForEach-Object {{$trigger=$_.Triggers | Select-Object -First 1; $action=$_.Actions | Select-Object -First 1; $rawName=$_.TaskName; $name=$rawName.Substring({}); $triggerLabel=if ($trigger -and $trigger.CimClass.CimClassName -like '*Boot*') {{'startup'}} elseif ($trigger -and $trigger.CimClass.CimClassName -like '*Logon*') {{'startup'}} else {{'daily'}}; $schedule=if ($trigger -and $trigger.StartBoundary) {{try {{([datetime]$trigger.StartBoundary).ToString('HH:mm')}} catch {{'-'}}}} else {{'-'}}; $command=if ($action) {{(($action.Execute + ' ' + $action.Arguments).Trim())}} else {{''}}; [Console]::WriteLine(($name,$triggerLabel,$schedule,$_.State,$command -join \"`t\"))}}",
        WINDOWS_TASK_PREFIX,
        WINDOWS_TASK_PREFIX.len()
    );
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("powershell failed to start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "list scheduled tasks failed: {}",
            clean_value(&decode_output(&output.stderr))
        ));
    }
    Ok(decode_output(&output.stdout)
        .lines()
        .filter_map(parse_task_row)
        .collect())
}

#[cfg(not(target_os = "windows"))]
fn list_windows_tasks() -> Result<Vec<ManagedTask>, String> {
    Err("Windows scheduled tasks are not supported on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn run_schtasks(args: &[&str]) -> Result<String, String> {
    let output = Command::new("schtasks")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("schtasks failed to start: {error}"))?;
    if output.status.success() {
        Ok(decode_output(&output.stdout))
    } else {
        Err(format!(
            "schtasks failed: {}",
            clean_value(&decode_output(&output.stderr))
        ))
    }
}

#[cfg(not(target_os = "windows"))]
fn run_schtasks(_args: &[&str]) -> Result<String, String> {
    Err("Windows scheduled tasks are not supported on this platform".to_string())
}

#[cfg(unix)]
fn create_cron_task(request: &TaskRequest) -> Result<(), String> {
    let marker = cron_marker(&request.name);
    let line = cron_line(request, &marker);
    let current = current_crontab()?;
    let mut lines = current
        .lines()
        .filter(|line| !line.contains(&marker))
        .map(str::to_string)
        .collect::<Vec<_>>();
    lines.push(line);
    install_crontab(&lines.join("\n"))
}

#[cfg(not(unix))]
fn create_cron_task(_request: &TaskRequest) -> Result<(), String> {
    Err("cron tasks are not supported on this platform".to_string())
}

#[cfg(unix)]
fn list_cron_tasks() -> Result<Vec<ManagedTask>, String> {
    Ok(current_crontab()?
        .lines()
        .filter_map(parse_cron_task)
        .collect())
}

#[cfg(not(unix))]
fn list_cron_tasks() -> Result<Vec<ManagedTask>, String> {
    Err("cron tasks are not supported on this platform".to_string())
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum CronEntryAction {
    Delete,
    Enable,
    Disable,
}

#[cfg(not(unix))]
#[derive(Clone, Copy)]
enum CronEntryAction {
    Delete,
    Enable,
    Disable,
}

#[cfg(unix)]
fn update_crontab_entry(name: &str, action: CronEntryAction) -> Result<(), String> {
    let marker = cron_marker(name);
    let current = current_crontab()?;
    let mut found = false;
    let mut lines = Vec::new();
    for line in current.lines() {
        if !line.contains(&marker) {
            lines.push(line.to_string());
            continue;
        }
        found = true;
        match action {
            CronEntryAction::Delete => {}
            CronEntryAction::Enable => {
                lines.push(
                    line.trim_start()
                        .strip_prefix(CRON_DISABLED_PREFIX)
                        .unwrap_or(line)
                        .to_string(),
                );
            }
            CronEntryAction::Disable => {
                if line.trim_start().starts_with(CRON_DISABLED_PREFIX) {
                    lines.push(line.to_string());
                } else {
                    lines.push(format!("{CRON_DISABLED_PREFIX}{}", line.trim_start()));
                }
            }
        }
    }
    if !found {
        return Err(format!("task not found: {name}"));
    }
    install_crontab(&lines.join("\n"))
}

#[cfg(not(unix))]
fn update_crontab_entry(_name: &str, _action: CronEntryAction) -> Result<(), String> {
    Err("cron tasks are not supported on this platform".to_string())
}

#[cfg(unix)]
fn current_crontab() -> Result<String, String> {
    let output = Command::new("crontab")
        .arg("-l")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("crontab -l failed to start: {error}"))?;
    if output.status.success() {
        return Ok(decode_output(&output.stdout));
    }
    let stderr = decode_output(&output.stderr);
    if stderr.to_ascii_lowercase().contains("no crontab") {
        Ok(String::new())
    } else {
        Err(format!("crontab -l failed: {}", clean_value(&stderr)))
    }
}

#[cfg(unix)]
fn install_crontab(content: &str) -> Result<(), String> {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("crontab install failed to start: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|error| format!("write crontab failed: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for crontab failed: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "crontab install failed: {}",
            clean_value(&decode_output(&output.stderr))
        ))
    }
}

#[cfg(unix)]
fn cron_line(request: &TaskRequest, marker: &str) -> String {
    let command = cron_command(&request.command);
    match &request.trigger {
        TaskTrigger::Startup => format!("@reboot {command} {marker}"),
        TaskTrigger::Daily { time } => {
            let (hour, minute) = time.split_once(':').unwrap_or(("00", "00"));
            format!("{minute} {hour} * * * {command} {marker}")
        }
    }
}

#[cfg(unix)]
fn parse_cron_task(raw_line: &str) -> Option<ManagedTask> {
    let line = raw_line.trim();
    let (enabled, line) = if let Some(rest) = line.strip_prefix(CRON_DISABLED_PREFIX) {
        (false, rest.trim())
    } else {
        (true, line)
    };
    let marker_at = line.find(CRON_MARKER_PREFIX)?;
    let body = line[..marker_at].trim();
    let name = line[marker_at + CRON_MARKER_PREFIX.len()..].trim();
    if name.is_empty() {
        return None;
    }
    let (trigger, schedule, command) = if let Some(command) = body.strip_prefix("@reboot") {
        (
            "startup".to_string(),
            "-".to_string(),
            command.trim().to_string(),
        )
    } else {
        let parts = body.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 6 {
            return None;
        }
        (
            "daily".to_string(),
            format!("{}:{}", two_digit(parts[1]), two_digit(parts[0])),
            parts[5..].join(" "),
        )
    };
    Some(ManagedTask {
        name: name.to_string(),
        trigger,
        schedule,
        status: if enabled { "enabled" } else { "disabled" }.to_string(),
        command,
    })
}

#[cfg(unix)]
fn cron_marker(name: &str) -> String {
    format!("# rdl-task:{}", name.replace(['#', '\r', '\n'], " ").trim())
}

#[cfg(unix)]
fn cron_command(command: &str) -> String {
    sanitize_single_line(command).replace('%', r"\%")
}

#[cfg(unix)]
fn two_digit(value: &str) -> String {
    value
        .parse::<u8>()
        .map(|value| format!("{value:02}"))
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(target_os = "windows")]
fn windows_task_name(name: &str) -> String {
    format!(
        "{}{}",
        WINDOWS_TASK_PREFIX,
        name.replace(['\\', '/', ':'], "-").trim()
    )
}

#[cfg(not(target_os = "windows"))]
fn windows_task_name(name: &str) -> String {
    format!(
        "{WINDOWS_TASK_PREFIX}{}",
        name.replace(['\\', '/', ':'], "-").trim()
    )
}

fn list_result(action: &str, message: &str, tasks: Vec<ManagedTask>) -> String {
    let mut lines = vec![
        "create_task".to_string(),
        "status=success".to_string(),
        format!("action={}", clean_value(action)),
        format!("message={}", clean_value(message)),
        "stdout:".to_string(),
        "Name\tTrigger\tSchedule\tStatus\tCommand".to_string(),
    ];
    for task in tasks {
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}",
            clean_value(&task.name),
            clean_value(&task.trigger),
            clean_value(&task.schedule),
            clean_value(&task.status),
            clean_value(&task.command)
        ));
    }
    lines.join("\n")
}

fn parse_task_row(line: &str) -> Option<ManagedTask> {
    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    Some(ManagedTask {
        name: parts[0].trim().to_string(),
        trigger: parts[1].trim().to_string(),
        schedule: parts[2].trim().to_string(),
        status: parts[3].trim().to_string(),
        command: parts[4..].join("\t").trim().to_string(),
    })
}

fn sanitize_single_line(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

fn valid_hhmm(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
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

fn decode_output(bytes: &[u8]) -> String {
    crate::text_decode::command_output(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_daily_task_request() {
        let request =
            TaskRequest::parse("name=Nightly\ntrigger=daily\ntime=23:15\ncommand_b64=ZWNobyBoaQ==")
                .unwrap();
        assert_eq!(request.name, "Nightly");
        assert_eq!(request.command, "echo hi");
        assert_eq!(
            request.trigger,
            TaskTrigger::Daily {
                time: "23:15".to_string()
            }
        );
    }

    #[test]
    fn rejects_invalid_daily_time() {
        let error =
            TaskRequest::parse("name=Nightly\ntrigger=daily\ntime=25:99\ncommand_b64=ZWNobyBoaQ==")
                .unwrap_err();
        assert!(error.contains("HH:MM"));
    }

    #[cfg(unix)]
    #[test]
    fn parses_enabled_cron_task() {
        let task = parse_cron_task("15 23 * * * echo hi # rdl-task:Nightly").unwrap();
        assert_eq!(task.name, "Nightly");
        assert_eq!(task.trigger, "daily");
        assert_eq!(task.schedule, "23:15");
        assert_eq!(task.status, "enabled");
        assert_eq!(task.command, "echo hi");
    }

    #[cfg(unix)]
    #[test]
    fn parses_disabled_startup_cron_task() {
        let task = parse_cron_task("# disabled: @reboot echo hi # rdl-task:Startup Task").unwrap();
        assert_eq!(task.name, "Startup Task");
        assert_eq!(task.trigger, "startup");
        assert_eq!(task.schedule, "-");
        assert_eq!(task.status, "disabled");
        assert_eq!(task.command, "echo hi");
    }
}

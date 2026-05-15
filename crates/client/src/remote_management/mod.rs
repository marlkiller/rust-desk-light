use crate::support::{join_sections, run_command, run_first_available, run_powershell};
use rdl_protocol::CommandKind;

mod file_manager;
mod remote_terminal;

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::ActiveConnections => active_connections(),
        CommandKind::FileManager => file_manager::handle(payload),
        CommandKind::KillTargetProcess => kill_target_process(payload),
        CommandKind::ProcessManager => process_list(),
        CommandKind::RemoteTerminal => remote_terminal::execute(payload),
        CommandKind::PerformanceMonitor => performance_snapshot(),
        CommandKind::EventLog => event_log_summary(),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

fn active_connections() -> String {
    let output = if cfg!(target_os = "windows") {
        run_command("netstat", &["-ano"], 40)
    } else {
        run_first_available(
            &[
                ("ss", &["-tunap"][..]),
                ("netstat", &["-tunap"][..]),
                ("lsof", &["-i", "-n", "-P"][..]),
            ],
            40,
        )
    };
    join_sections("active_connections", vec![output])
}

fn process_list() -> String {
    let output = if cfg!(target_os = "windows") {
        run_powershell(
            r#"Write-Output "PID`tName`tCPU`tMemoryMB"; Get-Process | Sort-Object CPU -Descending | ForEach-Object { "{0}`t{1}`t{2:N1}`t{3:N1}" -f $_.Id,$_.ProcessName,$_.CPU,($_.WorkingSet64/1MB) }"#,
            10_000,
        )
    } else if cfg!(target_os = "macos") {
        macos_process_list()
    } else {
        let output = run_command(
            "ps",
            &["-eo", "pid,ppid,comm,pcpu,pmem", "--sort=-pcpu"],
            10_000,
        );
        if output.contains("failed:") || output.contains("error") {
            run_command("ps", &["-eo", "pid,ppid,comm"], 10_000)
        } else {
            output
        }
    };
    join_sections("process_list", vec![output])
}

fn macos_process_list() -> String {
    let output = run_command(
        "ps",
        &["-axo", "pid=,ppid=,pcpu=,pmem=,command=", "-r", "-ww"],
        10_000,
    );
    if output.starts_with("ps failed:") || output.starts_with("ps timed out") {
        return output;
    }

    let mut rows = vec!["PID\tPPID\tCPU\tMEM\tCommand".to_string()];
    rows.extend(output.lines().filter_map(|line| {
        let mut cells = line.split_whitespace();
        let pid = cells.next()?.trim();
        let ppid = cells.next()?.trim();
        let cpu = cells.next()?.trim();
        let mem = cells.next()?.trim();
        let command = cells.collect::<Vec<_>>().join(" ");
        if pid.is_empty() || command.is_empty() {
            None
        } else {
            Some(format!("{pid}\t{ppid}\t{cpu}\t{mem}\t{command}"))
        }
    }));
    rows.join("\n")
}

fn kill_target_process(payload: &str) -> String {
    let pid = payload.trim();
    if pid.is_empty() || !pid.chars().all(|ch| ch.is_ascii_digit()) {
        return "kill_target_process requires numeric pid payload".to_string();
    }
    if pid == std::process::id().to_string() {
        return format!("kill_target_process refused: pid {pid} is this client process");
    }

    let output = if cfg!(target_os = "windows") {
        run_powershell(&format!("Stop-Process -Id {pid} -Force"), 20)
    } else {
        run_command("kill", &[pid], 20)
    };
    join_sections("kill_target_process", vec![output])
}

fn performance_snapshot() -> String {
    if cfg!(target_os = "windows") {
        run_powershell(
            "$os=Get-CimInstance Win32_OperatingSystem; $cpu=Get-CimInstance Win32_Processor | Select-Object -First 1; [pscustomobject]@{Cpu=$cpu.Name; LoadPercent=$cpu.LoadPercentage; TotalMemoryMB=[math]::Round($os.TotalVisibleMemorySize/1024); FreeMemoryMB=[math]::Round($os.FreePhysicalMemory/1024); LastBoot=$os.LastBootUpTime} | Format-List",
            30,
        )
    } else if cfg!(target_os = "macos") {
        join_sections(
            "performance_snapshot",
            vec![
                run_command("uptime", &[], 5),
                run_command("vm_stat", &[], 20),
                run_command("df", &["-h", "."], 10),
            ],
        )
    } else {
        join_sections(
            "performance_snapshot",
            vec![
                run_command("uptime", &[], 5),
                run_command("free", &["-m"], 10),
                run_command("df", &["-h", "."], 10),
            ],
        )
    }
}

fn event_log_summary() -> String {
    let output = if cfg!(target_os = "windows") {
        run_powershell(
            r#"Write-Output "Time`tLevel`tProvider`tId`tMessage"; Get-WinEvent -LogName System -MaxEvents 20 | ForEach-Object { $message=($_.Message -replace "`r|`n|`t", " "); "{0}`t{1}`t{2}`t{3}`t{4}" -f $_.TimeCreated,$_.LevelDisplayName,$_.ProviderName,$_.Id,$message }"#,
            80,
        )
    } else if cfg!(target_os = "macos") {
        macos_event_log_summary()
    } else {
        run_first_available(
            &[
                ("journalctl", &["-n", "20", "--no-pager"][..]),
                ("dmesg", &["-T"][..]),
            ],
            80,
        )
    };
    join_sections("event_log_summary", vec![output])
}

fn macos_event_log_summary() -> String {
    let output = run_command(
        "sh",
        &[
            "-lc",
            "/usr/bin/log show --style compact --last 15m --predicate 'eventType == logEvent AND (messageType == error OR messageType == fault)' 2>/dev/null | head -n 80",
        ],
        100,
    );
    if output.starts_with("sh failed:") || output.starts_with("sh timed out") {
        return output;
    }

    let mut rows = vec!["Time\tLevel\tProvider\tId\tMessage".to_string()];
    rows.extend(output.lines().filter_map(macos_log_row));
    if rows.len() == 1 {
        rows.push("none\tInfo\tmacOS\t-\tNo recent error or fault events found".to_string());
    }
    rows.join("\n")
}

fn macos_log_row(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("Timestamp") {
        return None;
    }

    let (time, rest) = split_at_checked(line, 23)?;
    if !is_macos_compact_timestamp(time) {
        return None;
    }
    let rest = rest.trim_start();
    let level_end = rest.find(char::is_whitespace)?;
    let level = rest[..level_end].trim();
    let rest = rest[level_end..].trim_start();
    let process_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let process = rest[..process_end].trim();
    let message = rest[process_end..].trim_start();
    if time.trim().is_empty() || level.is_empty() || process.is_empty() {
        return None;
    }

    let provider = message
        .split_once(']')
        .and_then(|(prefix, _)| prefix.strip_prefix('['))
        .unwrap_or(process)
        .trim();
    let message = sanitize_table_cell(message);
    Some(format!(
        "{}\t{}\t{}\t-\t{}",
        time.trim(),
        level,
        sanitize_table_cell(provider),
        message
    ))
}

fn is_macos_compact_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 23
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

fn split_at_checked(value: &str, mid: usize) -> Option<(&str, &str)> {
    if value.len() < mid || !value.is_char_boundary(mid) {
        return None;
    }
    Some(value.split_at(mid))
}

fn sanitize_table_cell(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::macos_log_row;

    #[test]
    fn macos_log_row_parses_compact_error_lines_with_extra_spacing() {
        let row = macos_log_row(
            "2026-05-15 22:21:40.247 E  WindowServer[596:11a3] [com.apple.SkyLight:default] _CGXPackagesSetWindowConstraints: Invalid window",
        )
        .expect("row should parse");

        assert_eq!(
            row,
            "2026-05-15 22:21:40.247\tE\tcom.apple.SkyLight:default\t-\t[com.apple.SkyLight:default] _CGXPackagesSetWindowConstraints: Invalid window"
        );
    }

    #[test]
    fn macos_log_row_ignores_log_show_status_lines() {
        assert!(macos_log_row("Filtering the log data using \"type == 1024\"").is_none());
        assert!(macos_log_row("Skipping info and debug messages").is_none());
    }
}

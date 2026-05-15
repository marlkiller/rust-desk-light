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

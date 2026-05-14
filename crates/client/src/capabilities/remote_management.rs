use super::support::{join_sections, run_command, run_first_available, run_powershell};
use rdl_protocol::CommandKind;

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::ActiveConnections => active_connections(),
        CommandKind::ProcessManager => process_list(),
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
            r#"Write-Output "PID`tName`tCPU`tMemoryMB"; Get-Process | Sort-Object CPU -Descending | Select-Object -First 25 | ForEach-Object { "{0}`t{1}`t{2:N1}`t{3:N1}" -f $_.Id,$_.ProcessName,$_.CPU,($_.WorkingSet64/1MB) }"#,
            40,
        )
    } else {
        let output = run_command(
            "ps",
            &["-eo", "pid,ppid,comm,pcpu,pmem", "--sort=-pcpu"],
            30,
        );
        if output.contains("failed:") || output.contains("error") {
            run_command("ps", &["-eo", "pid,ppid,comm"], 30)
        } else {
            output
        }
    };
    join_sections("process_list", vec![output])
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

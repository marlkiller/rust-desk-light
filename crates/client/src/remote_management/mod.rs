use crate::support::{join_sections, run_command, run_first_available, run_powershell};
use rdl_protocol::CommandKind;

mod file_manager;
mod remote_terminal;

pub(crate) use file_manager::handle_transfer as handle_file_transfer;
pub(crate) use remote_terminal::execute_streaming as execute_terminal_streaming;

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::ActiveConnections => active_connections(),
        CommandKind::DriverManager => driver_manager(),
        CommandKind::FileManager => file_manager::handle(payload),
        CommandKind::KillTargetProcess => kill_target_process(payload),
        CommandKind::ProcessManager => process_list(),
        CommandKind::RegistryManager => registry_manager(),
        CommandKind::RemoteTerminal => remote_terminal::execute(payload),
        CommandKind::StartupManager => startup_manager(),
        CommandKind::WindowManager => window_manager(),
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
    } else if cfg!(target_os = "macos") {
        macos_active_connections()
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

fn window_manager() -> String {
    let output = if cfg!(target_os = "windows") {
        windows_window_manager()
    } else if cfg!(target_os = "macos") {
        macos_window_manager()
    } else {
        linux_window_manager()
    };
    join_sections("window_manager", vec![output])
}

fn startup_manager() -> String {
    let output = if cfg!(target_os = "windows") {
        windows_startup_manager()
    } else if cfg!(target_os = "macos") {
        macos_startup_manager()
    } else {
        linux_startup_manager()
    };
    join_sections("startup_manager", vec![output])
}

fn registry_manager() -> String {
    let output = if cfg!(target_os = "windows") {
        windows_registry_manager()
    } else {
        unsupported_registry_table()
    };
    join_sections("registry_manager", vec![output])
}

fn driver_manager() -> String {
    let output = if cfg!(target_os = "windows") {
        windows_driver_manager()
    } else if cfg!(target_os = "macos") {
        macos_driver_manager()
    } else {
        linux_driver_manager()
    };
    join_sections("driver_manager", vec![output])
}

fn windows_window_manager() -> String {
    run_powershell(
        r#"
function Clean($value) {
  if ($null -eq $value) { return "-" }
  $text = [string]$value
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
Write-Output "PID`tProcess`tTitle`tResponding`tPath"
$count = 0
Get-Process | Where-Object { $_.MainWindowTitle } | Sort-Object ProcessName | ForEach-Object {
  $count += 1
  $path = try { $_.Path } catch { "" }
  "{0}`t{1}`t{2}`t{3}`t{4}" -f $_.Id,(Clean $_.ProcessName),(Clean $_.MainWindowTitle),$_.Responding,(Clean $path)
}
if ($count -eq 0) {
  "0`tInfo`tNo visible top-level windows found`t-`t-"
}
"#,
        300,
    )
}

fn macos_window_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
printf 'PID\tProcess\tTitle\tVisible\tPath\n'
if command -v lsappinfo >/dev/null 2>&1; then
  rows="$(lsappinfo visibleProcessList 2>/dev/null | grep -Eo 'ASN:0x[0-9a-fA-F]+-0x[0-9a-fA-F]+' | while read -r asn; do
    info="$(lsappinfo info -only pid,name,bundlepath "$asn" 2>/dev/null)"
    pid="$(printf '%s\n' "$info" | sed -n 's/^"pid"=//p' | head -n 1)"
    name="$(printf '%s\n' "$info" | sed -n 's/^"LSDisplayName"="\(.*\)"/\1/p' | head -n 1 | tr '\t\r\n' '   ')"
    path="$(printf '%s\n' "$info" | sed -n 's/^"LSBundlePath"="\(.*\)"/\1/p' | head -n 1 | tr '\t\r\n' '   ')"
    [ -n "$pid" ] || continue
    [ -n "$name" ] || name="-"
    [ -n "$path" ] || path="-"
    printf '%s\t%s\t-\ttrue\t%s\n' "$pid" "$name" "$path"
  done)"
  if [ -n "$rows" ]; then
    printf '%s\n' "$rows"
    exit 0
  fi
fi
output="$(osascript -e 'tell application "System Events" to get the unix id & tab & name of every process whose background only is false' 2>/dev/null || true)"
if [ -n "$output" ]; then
  printf '%s\n' "$output" | tr ',' '\n' | while IFS="$(printf '\t')" read -r pid name; do
    pid="$(printf '%s' "$pid" | tr -dc '0-9')"
    name="$(printf '%s' "$name" | sed 's/^ *//; s/ *$//' | tr '\t\r\n' '   ')"
    [ -n "$pid" ] || continue
    [ -n "$name" ] || name="-"
    printf '%s\t%s\t-\ttrue\t-\n' "$pid" "$name"
  done
  exit 0
fi
printf '0\tInfo\tFast window listing returned no visible apps\t-\t-\n'
"#,
        ],
        300,
    )
}

fn linux_window_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
if command -v wmctrl >/dev/null 2>&1; then
  wmctrl -lxp 2>/dev/null | awk 'BEGIN { OFS="\t"; print "WindowId","Desktop","PID","Class","Title" } { count++; title=""; for (i=6; i<=NF; i++) title=title (i>6 ? " " : "") $i; if (title=="") title="-"; print $1,$2,$3,$4,title } END { if (count == 0) print "-","-","-","Info","No desktop windows found" }'
else
  printf 'WindowId\tDesktop\tPID\tClass\tTitle\n-\t-\t-\tUnavailable\tInstall wmctrl to list desktop windows\n'
fi
"#,
        ],
        300,
    )
}

fn windows_startup_manager() -> String {
    run_powershell(
        r#"
function Clean($value) {
  if ($null -eq $value) { return "-" }
  $text = [string]$value
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
Write-Output "Scope`tSource`tName`tCommand`tStatus"
$count = 0
function EmitRow($scope, $source, $name, $command, $status) {
  $script:count += 1
  "{0}`t{1}`t{2}`t{3}`t{4}" -f (Clean $scope),(Clean $source),(Clean $name),(Clean $command),(Clean $status)
}
$runKeys = @(
  @{ Scope = "CurrentUser"; Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" },
  @{ Scope = "CurrentUser"; Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\RunOnce" },
  @{ Scope = "LocalMachine"; Path = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" },
  @{ Scope = "LocalMachine"; Path = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce" }
)
foreach ($entry in $runKeys) {
  if (Test-Path $entry.Path) {
    $props = Get-ItemProperty -Path $entry.Path
    $props.PSObject.Properties | Where-Object { $_.Name -notmatch '^PS' } | ForEach-Object {
      EmitRow $entry.Scope $entry.Path $_.Name $_.Value "Registry"
    }
  }
}
$folders = @(
  @{ Scope = "CurrentUser"; Path = [Environment]::GetFolderPath("Startup") },
  @{ Scope = "AllUsers"; Path = [Environment]::GetFolderPath("CommonStartup") }
)
foreach ($folder in $folders) {
  if ($folder.Path -and (Test-Path $folder.Path)) {
    Get-ChildItem -Path $folder.Path -File -ErrorAction SilentlyContinue | ForEach-Object {
      EmitRow $folder.Scope $folder.Path $_.Name $_.FullName "File"
    }
  }
}
if ($count -eq 0) {
  EmitRow "-" "-" "No startup items found" "-" "Info"
}
"#,
        300,
    )
}

fn macos_startup_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
printf 'Scope\tSource\tName\tCommand\tStatus\n'
count=0
emit() {
  count=$((count + 1))
  printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5"
}
for dir in "$HOME/Library/LaunchAgents" "/Library/LaunchAgents" "/Library/LaunchDaemons" "/System/Library/LaunchAgents" "/System/Library/LaunchDaemons"; do
  [ -d "$dir" ] || continue
  case "$dir" in
    "$HOME"/*) scope="CurrentUser" ;;
    /System/*) scope="System" ;;
    *) scope="LocalMachine" ;;
  esac
  for file in "$dir"/*.plist; do
    [ -e "$file" ] || continue
    emit "$scope" "$dir" "$(basename "$file")" "$file" "Present"
  done
done
if [ "$count" -eq 0 ]; then
  emit "-" "-" "No launch agents or daemons found" "-" "Info"
fi
"#,
        ],
        300,
    )
}

fn linux_startup_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
printf 'Scope\tSource\tName\tCommand\tStatus\n'
count=0
emit() {
  count=$((count + 1))
  printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5"
}
for dir in "$HOME/.config/autostart" "/etc/xdg/autostart"; do
  [ -d "$dir" ] || continue
  case "$dir" in
    "$HOME"/*) scope="CurrentUser" ;;
    *) scope="System" ;;
  esac
  for file in "$dir"/*.desktop; do
    [ -e "$file" ] || continue
    name="$(basename "$file")"
    command="$(sed -n 's/^Exec=//p' "$file" | head -n 1)"
    emit "$scope" "$dir" "$name" "${command:-$file}" "DesktopEntry"
  done
done
if command -v systemctl >/dev/null 2>&1; then
  system_rows="$(systemctl list-unit-files --type=service --state=enabled --no-legend --no-pager 2>/dev/null | awk 'NF > 0 { printf "System\tsystemd\t%s\t-\t%s\n", $1, ($2 == "" ? "enabled" : $2) }')"
  if [ -n "$system_rows" ]; then
    printf '%s\n' "$system_rows"
    row_count="$(printf '%s\n' "$system_rows" | wc -l | tr -d ' ')"
    count=$((count + row_count))
  fi
fi
if [ "$count" -eq 0 ]; then
  emit "-" "-" "No startup items found" "-" "Info"
fi
"#,
        ],
        300,
    )
}

fn windows_registry_manager() -> String {
    run_powershell(
        r#"
function Clean($value) {
  if ($null -eq $value) { return "-" }
  $text = [string]$value
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
Write-Output "Hive`tPath`tName`tType`tValue"
$count = 0
function EmitRow($hive, $path, $name, $type, $value) {
  $script:count += 1
  "{0}`t{1}`t{2}`t{3}`t{4}" -f (Clean $hive),(Clean $path),(Clean $name),(Clean $type),(Clean $value)
}
$keys = @(
  "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run",
  "HKCU:\Software\Microsoft\Windows\CurrentVersion\RunOnce",
  "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
  "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
  "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion"
)
foreach ($key in $keys) {
  if (!(Test-Path $key)) { continue }
  $item = Get-Item -Path $key
  $props = Get-ItemProperty -Path $key
  $hive = if ($key.StartsWith("HKCU:")) { "HKCU" } elseif ($key.StartsWith("HKLM:")) { "HKLM" } else { "-" }
  $path = $key -replace "^[^:]+:\\", ""
  $props.PSObject.Properties | Where-Object { $_.Name -notmatch '^PS' } | Select-Object -First 120 | ForEach-Object {
    $kind = try { $item.GetValueKind($_.Name) } catch { "Unknown" }
    EmitRow $hive $path $_.Name $kind $_.Value
  }
}
if ($count -eq 0) {
  EmitRow "-" "-" "No registry values found" "Info" "-"
}
"#,
        300,
    )
}

fn unsupported_registry_table() -> String {
    format!(
        "Hive\tPath\tName\tType\tValue\n{}",
        table_row(&[
            "-",
            "-",
            "Unsupported",
            "Info",
            "Registry Manager is only available on Windows",
        ])
    )
}

fn windows_driver_manager() -> String {
    run_powershell(
        r#"
function Clean($value) {
  if ($null -eq $value) { return "-" }
  $text = [string]$value
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
Write-Output "Name`tState`tStartMode`tPath`tDescription"
$count = 0
Get-CimInstance Win32_SystemDriver | Sort-Object Name | Select-Object -First 250 | ForEach-Object {
  $count += 1
  "{0}`t{1}`t{2}`t{3}`t{4}" -f (Clean $_.Name),(Clean $_.State),(Clean $_.StartMode),(Clean $_.PathName),(Clean $_.Description)
}
if ($count -eq 0) {
  "Info`t-`t-`t-`tNo system drivers found"
}
"#,
        300,
    )
}

fn macos_driver_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
if command -v kmutil >/dev/null 2>&1; then
  kmutil showloaded 2>/dev/null | awk 'BEGIN { OFS="\t"; print "Index","Refs","Name","Version","Status" } NR > 1 { version=$7; gsub(/[()]/, "", version); print $1,$2,$6,version,"Loaded"; count++ } END { if (count == 0) print "-","-","Info","-","No loaded kernel extensions found" }'
elif command -v kextstat >/dev/null 2>&1; then
  kextstat 2>/dev/null | awk 'BEGIN { OFS="\t"; print "Index","Refs","Name","Version","Status" } NR > 1 { version=$7; gsub(/[()]/, "", version); print $1,$2,$6,version,"Loaded"; count++ } END { if (count == 0) print "-","-","Info","-","No loaded kernel extensions found" }'
else
  printf 'Index\tRefs\tName\tVersion\tStatus\n-\t-\tUnavailable\t-\tNo macOS driver listing tool found\n'
fi
"#,
        ],
        300,
    )
}

fn linux_driver_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
if command -v lsmod >/dev/null 2>&1; then
  lsmod | awk 'BEGIN { OFS="\t"; print "Name","Size","UsedBy","Dependencies","Status" } NR > 1 { deps=$4; if (deps == "") deps="-"; print $1,$2,$3,deps,"Loaded"; count++ } END { if (count == 0) print "Info","-","-","-","No loaded kernel modules found" }'
else
  printf 'Name\tSize\tUsedBy\tDependencies\tStatus\nUnavailable\t-\t-\t-\tNo lsmod command found\n'
fi
"#,
        ],
        300,
    )
}

fn macos_active_connections() -> String {
    let output = run_command("lsof", &["-nP", "-iTCP", "-iUDP"], 200);
    if output.starts_with("lsof failed:")
        || output.starts_with("lsof timed out")
        || output.starts_with("lsof exited with error")
    {
        return output;
    }

    let mut rows = vec!["Proto\tLocal\tForeign\tState\tPID\tProgram".to_string()];
    rows.extend(output.lines().filter_map(macos_lsof_connection_row));
    if rows.len() == 1 {
        rows.push("none\t-\t-\tInfo\t-\tNo active TCP/UDP connections found".to_string());
    }
    rows.join("\n")
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

fn table_row(cells: &[&str]) -> String {
    cells
        .iter()
        .map(|cell| sanitize_table_cell(cell))
        .collect::<Vec<_>>()
        .join("\t")
}

fn macos_lsof_connection_row(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("COMMAND") {
        return None;
    }

    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 9 {
        return None;
    }

    let program = fields[0];
    let pid = fields[1];
    if !pid.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let proto = fields[7];
    if !matches!(proto, "TCP" | "UDP") {
        return None;
    }

    let mut endpoint_fields = &fields[8..];
    let mut state = if proto == "UDP" { "UDP" } else { "-" };
    if let Some(last) = endpoint_fields
        .last()
        .filter(|value| value.starts_with('(') && value.ends_with(')'))
    {
        state = last.trim_start_matches('(').trim_end_matches(')');
        endpoint_fields = &endpoint_fields[..endpoint_fields.len().saturating_sub(1)];
    }

    let endpoint = endpoint_fields.join(" ");
    if endpoint.trim().is_empty() {
        return None;
    }
    let (local, foreign) = endpoint
        .split_once("->")
        .map(|(local, foreign)| (local, foreign))
        .unwrap_or((endpoint.as_str(), "*"));

    Some(format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        proto,
        sanitize_table_cell(local),
        sanitize_table_cell(foreign),
        sanitize_table_cell(state),
        pid,
        sanitize_table_cell(program)
    ))
}

#[cfg(test)]
mod tests {
    use super::{macos_log_row, macos_lsof_connection_row, table_row, unsupported_registry_table};

    #[test]
    fn table_row_sanitizes_embedded_line_breaks_and_tabs() {
        assert_eq!(table_row(&["a\tb", "c\rd", "e\nf"]), "a b\tc d\te f");
    }

    #[test]
    fn unsupported_registry_response_stays_tabular() {
        let table = unsupported_registry_table();
        let rows = table.lines().collect::<Vec<_>>();

        assert_eq!(rows.first(), Some(&"Hive\tPath\tName\tType\tValue"));
        assert_eq!(rows.len(), 2);
        assert!(rows[1].contains("Registry Manager is only available on Windows"));
    }

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

    #[test]
    fn macos_lsof_connection_row_parses_tcp_listen_and_established() {
        let listen = macos_lsof_connection_row(
            "rapportd    972 voidm    9u  IPv4 0x28097430b3206655      0t0  TCP *:57828 (LISTEN)",
        )
        .expect("listen row should parse");
        assert_eq!(listen, "TCP\t*:57828\t*\tLISTEN\t972\trapportd");

        let established = macos_lsof_connection_row(
            "Telegram   1399 voidm   47u  IPv4 0x342b2389781194e5      0t0  TCP 198.18.0.1:57823->91.108.56.142:443 (ESTABLISHED)",
        )
        .expect("established row should parse");
        assert_eq!(
            established,
            "TCP\t198.18.0.1:57823\t91.108.56.142:443\tESTABLISHED\t1399\tTelegram"
        );
    }

    #[test]
    fn macos_lsof_connection_row_parses_udp() {
        let row = macos_lsof_connection_row(
            "rapportd    972 voidm   29u  IPv6 0x4663fa592f34f80f      0t0  UDP *:3722",
        )
        .expect("udp row should parse");

        assert_eq!(row, "UDP\t*:3722\t*\tUDP\t972\trapportd");
    }
}

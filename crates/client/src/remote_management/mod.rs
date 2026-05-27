use crate::support::{
    join_sections, run_command, run_command_with_env, run_first_available, run_powershell,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rdl_protocol::CommandKind;
use std::fs;
use std::path::{Path, PathBuf};

mod client_autostart;
mod file_manager;
mod registry_manager;
mod remote_terminal;
mod reverse_proxy;

pub(crate) use file_manager::handle_transfer as handle_file_transfer;
pub(crate) use remote_terminal::execute_streaming as execute_terminal_streaming;
pub(crate) use reverse_proxy::{client_proxy_stream_loop, ClientProxyStream};

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::ActiveConnections => active_connections(),
        CommandKind::DriverManager => driver_manager(),
        CommandKind::FileManager => file_manager::handle(payload),
        CommandKind::KillTargetProcess => kill_target_process(payload),
        CommandKind::ProcessManager => process_list(),
        CommandKind::RegistryManager => registry_manager::handle(payload),
        CommandKind::RemoteTerminal => remote_terminal::execute(payload),
        CommandKind::StartupManager => startup_manager(payload),
        CommandKind::ServiceManager => service_manager(payload),
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

fn service_manager(payload: &str) -> String {
    let request = ServiceRequest::parse(payload);
    let output = match request.action.as_str() {
        "list" => service_manager_list(),
        "details" => service_details(&request),
        "start" | "stop" | "restart" | "enable" | "disable" | "delete" => match apply_service_action(&request) {
            Ok(()) => service_manager_list(),
            Err(error) => return format!("service_manager_error\nstatus=error\nmessage={error}"),
        },
        "enable_client_service" => {
            if cfg!(target_os = "windows") {
                match windows_enable_client_service() {
                    Ok(()) => service_manager_list(),
                    Err(error) => return format!("service_manager_error\nstatus=error\nmessage={error}"),
                }
            } else {
                match client_autostart::apply_service_manager_action("enable") {
                    Ok(()) => service_manager_list(),
                    Err(error) => return format!("service_manager_error\nstatus=error\nmessage={error}"),
                }
            }
        },
        "disable_client_service" => {
            if cfg!(target_os = "windows") {
                match windows_disable_client_service() {
                    Ok(()) => service_manager_list(),
                    Err(error) => return format!("service_manager_error\nstatus=error\nmessage={error}"),
                }
            } else {
                match client_autostart::apply_service_manager_action("disable") {
                    Ok(()) => service_manager_list(),
                    Err(error) => return format!("service_manager_error\nstatus=error\nmessage={error}"),
                }
            }
        }
        action => {
            return format!("service_manager_error\nstatus=unsupported\nmessage=unsupported service_manager action: {action}");
        }
    };
    join_sections("service_manager", vec![output])
}

fn service_manager_list() -> String {
    if cfg!(target_os = "windows") {
        windows_service_manager()
    } else if cfg!(target_os = "macos") {
        macos_service_manager()
    } else {
        linux_service_manager()
    }
}

#[derive(Debug)]
struct ServiceRequest {
    action: String,
    name: Option<String>,
}

impl ServiceRequest {
    fn parse(payload: &str) -> Self {
        let action = startup_payload_field(payload, "action")
            .unwrap_or_else(|| "list".to_string())
            .to_ascii_lowercase();
        Self {
            action,
            name: startup_payload_field(payload, "name"),
        }
    }
}

fn apply_service_action(request: &ServiceRequest) -> Result<(), String> {
    let name = request
        .name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "service_manager action requires name".to_string())?;

    if cfg!(target_os = "windows") {
        windows_apply_service_action(name, &request.action)
    } else if cfg!(target_os = "macos") {
        macos_apply_service_action(name, &request.action)
    } else {
        linux_apply_service_action(name, &request.action)
    }
}

fn service_details(request: &ServiceRequest) -> String {
    let name = match request.name.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(name) => name,
        None => return startup_detail_error_text("service_manager details requires name"),
    };

    let output = if cfg!(target_os = "windows") {
        windows_service_details(name)
    } else if cfg!(target_os = "macos") {
        macos_service_details(name)
    } else {
        linux_service_details(name)
    };
    startup_detail_output(output)
}

fn windows_service_manager() -> String {
    run_powershell(
        r#"
function Clean($value) {
  if ($null -eq $value) { return "-" }
  $text = [string]$value
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
Write-Output "Name`tDisplayName`tStatus`tStartType"
Get-Service | ForEach-Object {
  "{0}`t{1}`t{2}`t{3}" -f (Clean $_.Name),(Clean $_.DisplayName),(Clean $_.Status),(Clean $_.StartType)
}
"#,
        600,
    )
}

fn windows_apply_service_action(name: &str, action: &str) -> Result<(), String> {
    let cmd = match action {
        "start" => format!("Start-Service -Name \"{name}\" -ErrorAction Stop"),
        "stop" => format!("Stop-Service -Name \"{name}\" -Force -ErrorAction Stop"),
        "restart" => format!("Restart-Service -Name \"{name}\" -Force -ErrorAction Stop"),
        "enable" => format!("Set-Service -Name \"{name}\" -StartupType Automatic -ErrorAction Stop"),
        "disable" => format!("Set-Service -Name \"{name}\" -StartupType Disabled -ErrorAction Stop"),
        "delete" => format!("sc.exe delete \"{name}\""),
        _ => return Err(format!("unsupported Windows service action: {action}")),
    };
    let script = if action == "delete" {
        cmd
    } else {
        format!("try {{ {cmd} }} catch {{ Write-Host $_.Exception.Message; exit 1 }}")
    };
    startup_command_result(run_powershell(&script, 60), &format!("{action} Windows service"))
}

fn windows_service_details(name: &str) -> String {
    run_powershell(
        &format!("Get-Service -Name \"{name}\" | Format-List *"),
        160,
    )
}

fn windows_enable_client_service() -> Result<(), String> {
    let current_paths = client_autostart::AutostartPaths::detect()?;
    let system_paths = client_autostart::AutostartPaths::detect_system()?;

    // 1. Install binary to system-level location
    client_autostart::install_current_binary(&system_paths)?;

    // 2. Sync current user's config to system profile directory
    client_autostart::install_config(&current_paths.config_path, &system_paths.config_path)?;

    let exe = system_paths.target_exe.display().to_string();
    let config_path = system_paths.config_path.display().to_string();
    let name = "RustDeskLightClientService";
    let desc = "rust-desk-light Client";
    let script = format!(
        r#"
try {{
    if (Get-Service -Name '{name}' -ErrorAction SilentlyContinue) {{
        Set-Service -Name '{name}' -StartupType Automatic -ErrorAction Stop
        Write-Host "Service already exists, startup type set to Automatic."
    }} else {{
        $binPath = '"{exe}" --service --config "{config_path}"'
        New-Service -Name '{name}' -BinaryPathName $binPath -DisplayName '{desc}' -Description '{desc}' -StartupType Automatic -ErrorAction Stop | Out-Null
        Write-Host "Service created successfully."
    }}
}} catch {{
    Write-Host $_.Exception.Message
    exit 1
}}
        "#,
    );
    startup_command_result(run_powershell(&script, 60), "enable Windows client service")
}

fn windows_disable_client_service() -> Result<(), String> {
    let name = "RustDeskLightClientService";
    let script = format!(
        r#"
try {{
    if (Get-Service -Name '{name}' -ErrorAction SilentlyContinue) {{
        Stop-Service -Name '{name}' -Force -ErrorAction SilentlyContinue
        if ($PSVersionTable.PSVersion.Major -ge 6) {{
            Remove-Service -Name '{name}' -ErrorAction Stop
        }} else {{
            sc.exe delete '{name}' | Out-Null
        }}
        Write-Host "Service removed successfully."
    }} else {{
        Write-Host "Service does not exist."
    }}
}} catch {{
    Write-Host $_.Exception.Message
    exit 1
}}
        "#
    );
    startup_command_result(run_powershell(&script, 60), "disable Windows client service")
}

fn macos_service_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
printf 'PID\tStatus\tLabel\n'
launchctl list | awk 'NR > 1 { printf "%s\t%s\t%s\n", $1, $2, $3 }'
"#,
        ],
        600,
    )
}

fn macos_apply_service_action(name: &str, action: &str) -> Result<(), String> {
    // macOS launchctl actions are complex because they often require the full path to the plist.
    // If only the label is provided, we can try some common commands.
    let output = match action {
        "start" => run_command("launchctl", &["start", name], 40),
        "stop" => run_command("launchctl", &["stop", name], 40),
        "restart" => {
            run_command("launchctl", &["stop", name], 40);
            run_command("launchctl", &["start", name], 40)
        }
        "delete" => run_command("launchctl", &["remove", name], 40),
        _ => return Err(format!("unsupported macOS service action: {action}")),
    };
    startup_command_result(output, &format!("{action} macOS service"))
}

fn macos_service_details(name: &str) -> String {
    run_command("launchctl", &["list", name], 160)
}

fn linux_service_manager() -> String {
    run_command(
        "sh",
        &[
            "-lc",
            r#"
if command -v systemctl >/dev/null 2>&1; then
  printf 'Unit\tLoad\tActive\tSub\tDescription\n'
  systemctl list-units --type=service --all --no-legend --no-pager | awk '{ description=""; for(i=5; i<=NF; i++) description=description (i>5?" ":"") $i; printf "%s\t%s\t%s\t%s\t%s\n", $1, $2, $3, $4, description }'
else
  printf 'Unit\tLoad\tActive\tSub\tDescription\n-\t-\t-\t-\tSystemctl not found\n'
fi
"#,
        ],
        600,
    )
}

fn linux_apply_service_action(name: &str, action: &str) -> Result<(), String> {
    let output = match action {
        "start" => run_command("systemctl", &["start", name], 40),
        "stop" => run_command("systemctl", &["stop", name], 40),
        "restart" => run_command("systemctl", &["restart", name], 40),
        "enable" => run_command("systemctl", &["enable", name], 40),
        "disable" => run_command("systemctl", &["disable", name], 40),
        "delete" => {
            run_command("systemctl", &["stop", name], 40);
            run_command("systemctl", &["disable", name], 40);
            return Ok(());
        }
        _ => return Err(format!("unsupported Linux service action: {action}")),
    };
    startup_command_result(output, &format!("{action} Linux service"))
}

fn linux_service_details(name: &str) -> String {
    run_command("systemctl", &["show", name, "--no-pager"], 160)
}

fn startup_manager(payload: &str) -> String {
    let request = StartupRequest::parse(payload);
    let output = match request.action.as_str() {
        "list" => startup_manager_list(),
        "add" | "enable" | "disable" | "delete" => match apply_startup_action(&request) {
            Ok(()) => startup_manager_list(),
            Err(error) => startup_action_error_table(&error),
        },
        "details" => startup_item_details(&request),
        "enable_client_autostart" => match client_autostart::apply_startup_manager_action("enable")
        {
            Ok(()) => startup_manager_list(),
            Err(error) => startup_action_error_table(&error),
        },
        "disable_client_autostart" => {
            match client_autostart::apply_startup_manager_action("disable") {
                Ok(()) => startup_manager_list(),
                Err(error) => startup_action_error_table(&error),
            }
        }
        action => {
            startup_action_error_table(&format!("unsupported startup_manager action: {action}"))
        }
    };
    join_sections("startup_manager", vec![output])
}

fn startup_manager_list() -> String {
    if cfg!(target_os = "windows") {
        windows_startup_manager()
    } else if cfg!(target_os = "macos") {
        macos_startup_manager()
    } else {
        linux_startup_manager()
    }
}

#[derive(Debug)]
struct StartupRequest {
    action: String,
    scope: Option<String>,
    source: Option<String>,
    name: Option<String>,
    command: Option<String>,
    status: Option<String>,
}

impl StartupRequest {
    fn parse(payload: &str) -> Self {
        let action = startup_payload_field(payload, "action")
            .unwrap_or_else(|| "list".to_string())
            .to_ascii_lowercase();
        Self {
            action,
            scope: startup_payload_field(payload, "scope"),
            source: startup_payload_field(payload, "source"),
            name: startup_payload_field(payload, "name"),
            command: startup_payload_field(payload, "command"),
            status: startup_payload_field(payload, "status"),
        }
    }
}

fn startup_payload_field(payload: &str, key: &str) -> Option<String> {
    let encoded_key = format!("{key}_b64");
    let raw_key = format!("{key}=");
    let encoded_prefix = format!("{encoded_key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&encoded_prefix))
        .and_then(|value| decode_payload_value(value.trim()).ok())
        .or_else(|| {
            payload
                .lines()
                .find_map(|line| line.strip_prefix(&raw_key))
                .map(|value| value.trim().to_string())
        })
}

fn decode_payload_value(value: &str) -> Result<String, String> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|error| format!("invalid base64 payload field: {error}"))?;
    String::from_utf8(bytes).map_err(|error| format!("invalid utf8 payload field: {error}"))
}

fn apply_startup_action(request: &StartupRequest) -> Result<(), String> {
    match request.action.as_str() {
        "add" => add_startup_item(request),
        "enable" | "disable" => set_startup_item_enabled(request),
        "delete" => delete_startup_item(request),
        action => Err(format!("unsupported startup_manager action: {action}")),
    }
}

fn add_startup_item(request: &StartupRequest) -> Result<(), String> {
    let name = required_startup_value(request.name.as_deref(), "name")?;
    let command = required_startup_value(request.command.as_deref(), "command")?;
    if cfg!(target_os = "windows") {
        windows_add_startup_item(name, command)
    } else if cfg!(target_os = "macos") {
        macos_add_startup_item(name, command)
    } else {
        linux_add_startup_item(name, command)
    }
}

fn set_startup_item_enabled(request: &StartupRequest) -> Result<(), String> {
    let source = required_startup_value(request.source.as_deref(), "source")?;
    let name = required_startup_value(request.name.as_deref(), "name")?;
    let enabled = request.action == "enable";
    if cfg!(target_os = "windows") {
        windows_set_startup_item_enabled(source, name, enabled)
    } else if cfg!(target_os = "macos") {
        macos_set_startup_item_enabled(source, name, enabled)
    } else {
        linux_set_startup_item_enabled(source, name, enabled)
    }
}

fn delete_startup_item(request: &StartupRequest) -> Result<(), String> {
    let source = required_startup_value(request.source.as_deref(), "source")?;
    let name = required_startup_value(request.name.as_deref(), "name")?;
    if cfg!(target_os = "windows") {
        windows_delete_startup_item(source, name)
    } else if cfg!(target_os = "macos") {
        delete_startup_file(source, name)
    } else {
        linux_delete_startup_item(source, name)
    }
}

fn startup_item_details(request: &StartupRequest) -> String {
    if let Err(error) = required_startup_value(request.source.as_deref(), "source") {
        return startup_detail_error_text(&error);
    }
    if let Err(error) = required_startup_value(request.name.as_deref(), "name") {
        return startup_detail_error_text(&error);
    }

    if cfg!(target_os = "windows") {
        windows_startup_item_details(request)
    } else if cfg!(target_os = "macos") {
        macos_startup_item_details(request)
    } else {
        linux_startup_item_details(request)
    }
}

fn required_startup_value<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "-")
        .ok_or_else(|| format!("startup_manager requires {name}"))
}

fn startup_action_error_table(message: &str) -> String {
    format!(
        "Scope\tSource\tName\tCommand\tStatus\n{}",
        table_row(&["-", "-", "Startup action failed", message, "Error"])
    )
}

fn startup_detail_error_text(message: &str) -> String {
    format!("startup_item_details\n[Error]\n{message}")
}

fn startup_detail_output(output: String) -> String {
    if output
        .lines()
        .skip_while(|line| line.trim().is_empty() || line.trim_end().ends_with(':'))
        .next()
        .map(|line| line.trim() == "startup_item_details")
        .unwrap_or(false)
    {
        output
    } else {
        format!("startup_item_details\n{output}")
    }
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
    let current_exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let script = r#"
function Decode($value) {
  [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($value))
}
function Clean($value) {
  if ($null -eq $value) { return "-" }
  $text = [string]$value
  $text = $text -replace "`r|`n|`t", " "
  if ([string]::IsNullOrWhiteSpace($text)) { "-" } else { $text.Trim() }
}
$currentExe = Decode "__CURRENT_EXE_B64__"
Write-Output "Scope`tSource`tName`tCommand`tStatus"
$count = 0
function EmitRow($scope, $source, $name, $command, $status) {
  $script:count += 1
  "{0}`t{1}`t{2}`t{3}`t{4}" -f (Clean $scope),(Clean $source),(Clean $name),(Clean $command),(Clean $status)
}
function Compact($value) {
  if ($null -eq $value) { return "" }
  [regex]::Replace(([string]$value).ToLowerInvariant(), "[^a-z0-9]", "")
}
function IsClientAutostart($name) {
  $id = Compact $name
  @(
    "rustdesklightclient",
    "rustdesklightclientdesktop",
    "rustdesklightclientdesktopdisabled",
    "rustdesklightclientservice"
  ) -contains $id
}
function StartupRowStatus($source, $name, $command, $status) {
  $status
}
$runKeys = @(
  @{ Scope = "CurrentUser"; Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"; Status = "Enabled" },
  @{ Scope = "CurrentUser"; Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\RunOnce"; Status = "Enabled" },
  @{ Scope = "CurrentUser"; Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\RunDisabled"; Status = "Disabled" },
  @{ Scope = "CurrentUser"; Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\RunOnceDisabled"; Status = "Disabled" },
  @{ Scope = "LocalMachine"; Path = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run"; Status = "Enabled" },
  @{ Scope = "LocalMachine"; Path = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce"; Status = "Enabled" },
  @{ Scope = "LocalMachine"; Path = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunDisabled"; Status = "Disabled" },
  @{ Scope = "LocalMachine"; Path = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnceDisabled"; Status = "Disabled" }
)
foreach ($entry in $runKeys) {
  if (Test-Path $entry.Path) {
    $props = Get-ItemProperty -Path $entry.Path
    $props.PSObject.Properties | Where-Object { $_.Name -notmatch '^PS' } | ForEach-Object {
      $status = StartupRowStatus $entry.Path $_.Name $_.Value $entry.Status
      EmitRow $entry.Scope $entry.Path $_.Name $_.Value $status
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
      $status = if ($_.Name.EndsWith(".disabled")) { "Disabled" } else { "Enabled" }
      $command = ShortcutTarget $_.FullName
      $status = StartupRowStatus $folder.Path $_.Name $command $status
      EmitRow $folder.Scope $folder.Path $_.Name $command $status
    }
  }
}
if ($count -eq 0) {
  EmitRow "-" "-" "No startup items found" "-" "Info"
}
"#
    .replace("__CURRENT_EXE_B64__", &STANDARD.encode(current_exe));
    run_powershell(&script, 300)
}

fn windows_startup_item_details(request: &StartupRequest) -> String {
    let scope = request.scope.as_deref().unwrap_or_default();
    let source = request.source.as_deref().unwrap_or_default();
    let name = request.name.as_deref().unwrap_or_default();
    let command = request.command.as_deref().unwrap_or_default();
    let status = request.status.as_deref().unwrap_or_default();
    let script = r#"
$ErrorActionPreference = "Continue"
function Decode($value) {
  [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($value))
}
function Section($name) { "`n[$name]" }
$scope = Decode "__SCOPE_B64__"
$source = Decode "__SOURCE_B64__"
$name = Decode "__NAME_B64__"
$command = Decode "__COMMAND_B64__"
$status = Decode "__STATUS_B64__"
"startup_item_details"
Section "Selected Item"
"scope=$scope"
"source=$source"
"name=$name"
"command=$command"
"status=$status"
if ($source -match '^HK(CU|LM):\\') {
  Section "Registry Startup Value"
  if (!(Test-Path $source)) {
    "startup registry source does not exist: $source"
    exit 0
  }
  $property = (Get-ItemProperty -Path $source).PSObject.Properties | Where-Object { $_.Name -eq $name } | Select-Object -First 1
  if ($null -eq $property) {
    "startup registry value not found: $name"
    exit 0
  }
  "path=$source"
  "name=$name"
  "type=$($property.TypeNameOfValue)"
  "value=$($property.Value)"
  exit 0
}
Section "Startup Folder Item"
$itemPath = Join-Path $source $name
"path=$itemPath"
if (!(Test-Path -LiteralPath $itemPath -PathType Leaf)) {
  "startup file not found: $itemPath"
  exit 0
}
if ($itemPath.EndsWith(".lnk")) {
  $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($itemPath)
  "target=$($shortcut.TargetPath)"
  "arguments=$($shortcut.Arguments)"
  "working_directory=$($shortcut.WorkingDirectory)"
} else {
  Get-Content -LiteralPath $itemPath -Raw -ErrorAction SilentlyContinue
}
"#
    .replace("__SCOPE_B64__", &STANDARD.encode(scope))
    .replace("__SOURCE_B64__", &STANDARD.encode(source))
    .replace("__NAME_B64__", &STANDARD.encode(name))
    .replace("__COMMAND_B64__", &STANDARD.encode(command))
    .replace("__STATUS_B64__", &STANDARD.encode(status));
    startup_detail_output(run_powershell(&script, 120))
}

fn windows_add_startup_item(name: &str, command: &str) -> Result<(), String> {
    let script = r#"
$ErrorActionPreference = "Stop"
function Decode($value) {
  [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($value))
}
$name = Decode "__NAME_B64__"
$command = Decode "__COMMAND_B64__"
if ([string]::IsNullOrWhiteSpace($name)) { throw "startup item name is required" }
if ([string]::IsNullOrWhiteSpace($command)) { throw "startup item command is required" }
$key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
if (!(Test-Path $key)) {
  New-Item -Path $key -Force | Out-Null
}
New-ItemProperty -Path $key -Name $name -Value $command -PropertyType String -Force | Out-Null
Write-Output "ok"
"#
    .replace("__NAME_B64__", &STANDARD.encode(name))
    .replace("__COMMAND_B64__", &STANDARD.encode(command));
    startup_command_result(run_powershell(&script, 20), "add Windows startup item")
}

fn windows_set_startup_item_enabled(source: &str, name: &str, enabled: bool) -> Result<(), String> {
    let script = r#"
$ErrorActionPreference = "Stop"
function Decode($value) {
  [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($value))
}
function RegistryDestination($source, $enable) {
  if ($enable) {
    if ($source -match "\\RunOnceDisabled$") { return ($source -replace "RunOnceDisabled$", "RunOnce") }
    if ($source -match "\\RunDisabled$") { return ($source -replace "RunDisabled$", "Run") }
    if ($source -match "\\RunOnce$" -or $source -match "\\Run$") { return $source }
  } else {
    if ($source -match "\\RunOnceDisabled$" -or $source -match "\\RunDisabled$") { return $source }
    if ($source -match "\\RunOnce$") { return ($source -replace "RunOnce$", "RunOnceDisabled") }
    if ($source -match "\\Run$") { return ($source -replace "Run$", "RunDisabled") }
  }
  return $null
}
$source = Decode "__SOURCE_B64__"
$name = Decode "__NAME_B64__"
$enable = "__ENABLE__" -eq "true"
if ([string]::IsNullOrWhiteSpace($source)) { throw "startup item source is required" }
if ([string]::IsNullOrWhiteSpace($name)) { throw "startup item name is required" }
if ($source -match "^HK(CU|LM):\\") {
  $destination = RegistryDestination $source $enable
  if ($null -eq $destination) { throw "unsupported registry startup source: $source" }
  if ($destination -eq $source) {
    Write-Output "ok"
    exit 0
  }
  if (!(Test-Path $source)) { throw "startup registry source does not exist: $source" }
  $property = (Get-ItemProperty -Path $source).PSObject.Properties | Where-Object { $_.Name -eq $name } | Select-Object -First 1
  if ($null -eq $property) { throw "startup registry value not found: $name" }
  if (!(Test-Path $destination)) {
    New-Item -Path $destination -Force | Out-Null
  }
  New-ItemProperty -Path $destination -Name $name -Value $property.Value -PropertyType String -Force | Out-Null
  Remove-ItemProperty -Path $source -Name $name -ErrorAction Stop
  Write-Output "ok"
  exit 0
}
if (!(Test-Path -LiteralPath $source -PathType Container)) {
  throw "unsupported startup source: $source"
}
$path = Join-Path $source $name
if (!(Test-Path -LiteralPath $path -PathType Leaf)) {
  throw "startup file not found: $path"
}
if ($enable) {
  if (!$name.EndsWith(".disabled")) {
    Write-Output "ok"
    exit 0
  }
  $newName = $name.Substring(0, $name.Length - ".disabled".Length)
} else {
  if ($name.EndsWith(".disabled")) {
    Write-Output "ok"
    exit 0
  }
  $newName = "$name.disabled"
}
$target = Join-Path $source $newName
if (Test-Path -LiteralPath $target) { throw "target startup file already exists: $target" }
Rename-Item -LiteralPath $path -NewName $newName
Write-Output "ok"
"#
    .replace("__SOURCE_B64__", &STANDARD.encode(source))
    .replace("__NAME_B64__", &STANDARD.encode(name))
    .replace("__ENABLE__", if enabled { "true" } else { "false" });
    startup_command_result(run_powershell(&script, 40), "update Windows startup item")
}

fn windows_delete_startup_item(source: &str, name: &str) -> Result<(), String> {
    let script = r#"
$ErrorActionPreference = "Stop"
function Decode($value) {
  [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($value))
}
$source = Decode "__SOURCE_B64__"
$name = Decode "__NAME_B64__"
if ([string]::IsNullOrWhiteSpace($source)) { throw "startup item source is required" }
if ([string]::IsNullOrWhiteSpace($name)) { throw "startup item name is required" }
if ($source -match "^HK(CU|LM):\\") {
  if (!(Test-Path $source)) { throw "startup registry source does not exist: $source" }
  $property = (Get-ItemProperty -Path $source).PSObject.Properties | Where-Object { $_.Name -eq $name } | Select-Object -First 1
  if ($null -eq $property) { throw "startup registry value not found: $name" }
  Remove-ItemProperty -Path $source -Name $name -ErrorAction Stop
  Write-Output "ok"
  exit 0
}
if (!(Test-Path -LiteralPath $source -PathType Container)) {
  throw "unsupported startup source: $source"
}
$path = Join-Path $source $name
if (!(Test-Path -LiteralPath $path -PathType Leaf)) {
  throw "startup file not found: $path"
}
Remove-Item -LiteralPath $path -Force
Write-Output "ok"
"#
    .replace("__SOURCE_B64__", &STANDARD.encode(source))
    .replace("__NAME_B64__", &STANDARD.encode(name));
    startup_command_result(run_powershell(&script, 40), "delete Windows startup item")
}

fn macos_startup_manager() -> String {
    let current_exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_default();

    run_command_with_env(
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

compact_identity() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -cd '[:alnum:]'
}

is_client_autostart() {
  case "$(compact_identity "$1")" in
    rustdesklightclient|\
    rustdesklightclientdesktop|\
    rustdesklightclientdesktopdisabled|\
    rustdesklightclientservice|\
    comrustdesklightclientplist|\
    comrustdesklightclientplistdisabled)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

plist_program_file() {
  file="$1"
  awk '
    /<key>Program<\/key>/ {
      getline
      gsub(/.*<string>|<\/string>.*/, "")
      print
      exit
    }
    /<key>ProgramArguments<\/key>/ {
      in_args=1
      next
    }
    in_args && /<string>/ {
      gsub(/.*<string>|<\/string>.*/, "")
      print
      exit
    }
  ' "$file" 2>/dev/null
}

startup_status() {
  name="$1"
  command="$2"
  status="$3"
  printf '%s' "$status"
}
for dir in \
  "$HOME/Library/LaunchAgents" \
  "/Library/LaunchAgents" \
  "/Library/LaunchDaemons" \
  "/System/Library/LaunchAgents" \
  "/System/Library/LaunchDaemons"
do
  [ -d "$dir" ] || continue
  case "$dir" in
    "$HOME"/*)
      scope="CurrentUser"
      ;;
    /System/*)
      scope="System"
      ;;
    *)
      scope="LocalMachine"
      ;;
  esac
  for file in "$dir"/*.plist "$dir"/*.plist.disabled; do
    [ -e "$file" ] || continue
    name="$(basename "$file")"
    case "$name" in
      *.disabled)
        status="Disabled"
        ;;
      *)
        status="Enabled"
        ;;
    esac
    command="$(plist_program_file "$file")"
    [ -n "$command" ] || command="$file"
    status="$(startup_status "$name" "$command" "$status")"
    emit "$scope" "$dir" "$name" "$command" "$status"
  done
done
if [ "$count" -eq 0 ]; then
  emit "-" "-" "No launch agents or daemons found" "-" "Info"
fi
"#,
        ],
        &[("RDL_CURRENT_EXE", current_exe.as_str())],
        300,
    )
}

fn macos_startup_item_details(request: &StartupRequest) -> String {
    let scope = request.scope.as_deref().unwrap_or_default();
    let source = request.source.as_deref().unwrap_or_default();
    let name = request.name.as_deref().unwrap_or_default();
    let command = request.command.as_deref().unwrap_or_default();
    let status = request.status.as_deref().unwrap_or_default();
    startup_detail_output(run_command_with_env(
        "sh",
        &[
            "-lc",
            r#"
section() {
  printf '\n[%s]\n' "$1"
}
source="$RDL_SOURCE"
name="$RDL_NAME"
command="$RDL_COMMAND"
file="$source/$name"
printf 'startup_item_details\n'
section 'Selected Item'
printf 'scope=%s\nsource=%s\nname=%s\ncommand=%s\nstatus=%s\n' "$RDL_SCOPE" "$source" "$name" "$command" "$RDL_STATUS"
section 'Launch plist path'
printf '%s\n' "$file"
section 'Launch plist content'
if [ -f "$file" ]; then
  cat "$file" 2>&1 || true
else
  printf 'startup plist not found: %s\n' "$file"
fi
"#,
        ],
        &[
            ("RDL_SCOPE", scope),
            ("RDL_SOURCE", source),
            ("RDL_NAME", name),
            ("RDL_COMMAND", command),
            ("RDL_STATUS", status),
        ],
        160,
    ))
}

fn macos_add_startup_item(name: &str, command: &str) -> Result<(), String> {
    let launch_agents = home_dir()?.join("Library").join("LaunchAgents");
    fs::create_dir_all(&launch_agents)
        .map_err(|error| format!("create LaunchAgents directory failed: {error}"))?;
    let label = format!("com.rust-desk-light.{}", safe_startup_file_stem(name));
    let path = launch_agents.join(format!("{label}.plist"));
    if path.exists() {
        return Err(format!("startup item already exists: {}", path.display()));
    }

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-lc</string>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
        xml_escape(&label),
        xml_escape(command)
    );
    fs::write(&path, plist).map_err(|error| format!("write launch agent failed: {error}"))
}

fn macos_set_startup_item_enabled(source: &str, name: &str, enabled: bool) -> Result<(), String> {
    rename_disabled_startup_file(Path::new(source), name, enabled)
}

fn linux_startup_manager() -> String {
    let current_exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    run_command_with_env(
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

compact_identity() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -cd '[:alnum:]'
}

is_client_autostart() {
  case "$(compact_identity "$1")" in
    rustdesklightclient|rustdesklightclientdesktop|rustdesklightclientdesktopdisabled|rustdesklightclientservice) return 0 ;;
    *) return 1 ;;
  esac
}
startup_status() {
  name="$1"
  command="$2"
  status="$3"
  printf '%s' "$status"
}
systemd_show_value() {
  property="$1"
  shift
  systemctl "$@" show rust-desk-light-client.service -p "$property" --value 2>/dev/null | head -n 1
}
systemd_exec_file() {
  value="$1"
  [ -n "$value" ] || return 0
  path="$(printf '%s\n' "$value" | sed -n 's/.*path=\([^ ;]*\).*/\1/p' | head -n 1)"
  if [ -n "$path" ]; then
    printf '%s\n' "$path"
    return 0
  fi
  set -- $value
  printf '%s\n' "$1"
}
systemd_display_command() {
  value="$1"
  if [ -z "$value" ]; then
    printf '-'
    return 0
  fi
  argv="$(printf '%s\n' "$value" | sed -n 's/.*argv\[\]=\(.*\) ;.*/\1/p' | head -n 1)"
  if [ -n "$argv" ]; then
    printf '%s' "$argv"
  else
    printf '%s' "$value"
  fi
}
systemd_process_file() {
  pid="$(systemd_show_value MainPID "$@" | tr -dc '0-9')"
  [ -n "$pid" ] && [ "$pid" != "0" ] || return 0
  readlink "/proc/$pid/exe" 2>/dev/null || true
}
emit_systemd_client_unit() {
  scope="$1"
  source="$2"
  shift 2
  status="$(systemctl "$@" is-enabled rust-desk-light-client.service 2>/dev/null || true)"
  row_status=""
  case "$status" in
    enabled|enabled-runtime|linked|linked-runtime) row_status="Enabled" ;;
    disabled|masked) row_status="Disabled" ;;
  esac
  [ -n "$row_status" ] || return 0
  exec_value="$(systemd_show_value ExecStart "$@")"
  command="$(systemd_display_command "$exec_value")"
  emit "$scope" "$source" "rust-desk-light-client.service" "$command" "$row_status"
}
for dir in "$HOME/.config/autostart" "/etc/xdg/autostart"; do
  [ -d "$dir" ] || continue
  case "$dir" in
    "$HOME"/*) scope="CurrentUser" ;;
    *) scope="System" ;;
  esac
  for file in "$dir"/*.desktop "$dir"/*.desktop.disabled; do
    [ -e "$file" ] || continue
    name="$(basename "$file")"
    command="$(sed -n 's/^Exec=//p' "$file" | head -n 1)"
    hidden="$(awk -F= 'tolower($1)=="hidden" { print tolower($2); exit }' "$file" 2>/dev/null)"
    autostart_enabled="$(awk -F= 'tolower($1)=="x-gnome-autostart-enabled" { print tolower($2); exit }' "$file" 2>/dev/null)"
    status="Enabled"
    case "$name:$hidden:$autostart_enabled" in
      *.disabled:*|*:true:*|*:*:false) status="Disabled" ;;
    esac
    status="$(startup_status "$name" "$command" "$status")"
    emit "$scope" "$dir" "$name" "${command:-$file}" "$status"
  done
done
if command -v systemctl >/dev/null 2>&1; then
  emit_systemd_client_unit "System" "systemd"
  system_rows="$(systemctl list-unit-files --type=service --state=enabled,disabled --no-legend --no-pager 2>/dev/null | head -n 160 | awk 'NF > 0 && $1 != "rust-desk-light-client.service" { status=tolower($2); if (status == "enabled") status="Enabled"; else if (status == "disabled") status="Disabled"; printf "System\tsystemd\t%s\t-\t%s\n", $1, status }')"
  if [ -n "$system_rows" ]; then
    printf '%s\n' "$system_rows"
    row_count="$(printf '%s\n' "$system_rows" | wc -l | tr -d ' ')"
    count=$((count + row_count))
  fi
  emit_systemd_client_unit "CurrentUser" "systemd-user" --user
  user_rows="$(systemctl --user list-unit-files --type=service --state=enabled,disabled --no-legend --no-pager 2>/dev/null | head -n 80 | awk 'NF > 0 && $1 != "rust-desk-light-client.service" { status=tolower($2); if (status == "enabled") status="Enabled"; else if (status == "disabled") status="Disabled"; printf "CurrentUser\tsystemd-user\t%s\t-\t%s\n", $1, status }')"
  if [ -n "$user_rows" ]; then
    printf '%s\n' "$user_rows"
    row_count="$(printf '%s\n' "$user_rows" | wc -l | tr -d ' ')"
    count=$((count + row_count))
  fi
fi
if [ "$count" -eq 0 ]; then
  emit "-" "-" "No startup items found" "-" "Info"
fi
"#,
        ],
        &[("RDL_CURRENT_EXE", current_exe.as_str())],
        300,
    )
}

fn linux_startup_item_details(request: &StartupRequest) -> String {
    let scope = request.scope.as_deref().unwrap_or_default();
    let source = request.source.as_deref().unwrap_or_default();
    let name = request.name.as_deref().unwrap_or_default();
    let command = request.command.as_deref().unwrap_or_default();
    let status = request.status.as_deref().unwrap_or_default();
    startup_detail_output(run_command_with_env(
        "sh",
        &[
            "-lc",
            r#"
section() {
  printf '\n[%s]\n' "$1"
}
source="$RDL_SOURCE"
name="$RDL_NAME"
command="$RDL_COMMAND"
printf 'startup_item_details\n'
section 'Selected Item'
printf 'scope=%s\nsource=%s\nname=%s\ncommand=%s\nstatus=%s\n' "$RDL_SCOPE" "$source" "$name" "$command" "$RDL_STATUS"
case "$source" in
  systemd|systemd-user)
    if [ "$source" = "systemd-user" ]; then
      user_flag="--user"
    else
      user_flag=""
    fi
    section 'systemd unit file path'
    systemctl $user_flag show "$name" -p FragmentPath --value 2>&1 || true
    section 'systemd unit file content'
    systemctl $user_flag cat "$name" --no-pager 2>&1 || true
    exit 0
    ;;
esac
file="$source/$name"
section 'desktop entry path'
printf '%s\n' "$file"
section 'desktop entry content'
if [ -f "$file" ]; then
  cat "$file" 2>&1 || true
else
  printf 'startup desktop entry not found: %s\n' "$file"
fi
"#,
        ],
        &[
            ("RDL_SCOPE", scope),
            ("RDL_SOURCE", source),
            ("RDL_NAME", name),
            ("RDL_COMMAND", command),
            ("RDL_STATUS", status),
        ],
        160,
    ))
}

fn linux_add_startup_item(name: &str, command: &str) -> Result<(), String> {
    let autostart_dir = home_dir()?.join(".config").join("autostart");
    fs::create_dir_all(&autostart_dir)
        .map_err(|error| format!("create autostart directory failed: {error}"))?;
    let path = autostart_dir.join(format!("{}.desktop", safe_startup_file_stem(name)));
    if path.exists() {
        return Err(format!("startup item already exists: {}", path.display()));
    }

    let entry = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={}\nX-GNOME-Autostart-enabled=true\n",
        desktop_entry_value(name),
        desktop_entry_value(command)
    );
    fs::write(&path, entry).map_err(|error| format!("write autostart entry failed: {error}"))
}

fn linux_set_startup_item_enabled(source: &str, name: &str, enabled: bool) -> Result<(), String> {
    match source {
        "systemd" => {
            let output = run_command(
                "systemctl",
                &[if enabled { "enable" } else { "disable" }, name],
                40,
            );
            startup_command_result(output, "update systemd startup service")
        }
        "systemd-user" => {
            let output = run_command(
                "systemctl",
                &["--user", if enabled { "enable" } else { "disable" }, name],
                40,
            );
            startup_command_result(output, "update user systemd startup service")
        }
        _ if name.ends_with(".disabled") || source.ends_with("/autostart") => {
            let source_path = Path::new(source);
            if name.ends_with(".disabled") {
                rename_disabled_startup_file(source_path, name, enabled)
            } else {
                set_desktop_entry_enabled(source_path, name, enabled)
            }
        }
        _ => Err(format!("unsupported Linux startup source: {source}")),
    }
}

fn linux_delete_startup_item(source: &str, name: &str) -> Result<(), String> {
    match source {
        "systemd" | "systemd-user" => linux_delete_systemd_client_unit(source, name),
        _ if name.ends_with(".desktop")
            || name.ends_with(".desktop.disabled")
            || source.ends_with("/autostart") =>
        {
            delete_startup_file(source, name)
        }
        _ => Err(format!("unsupported Linux startup source: {source}")),
    }
}

fn linux_delete_systemd_client_unit(source: &str, name: &str) -> Result<(), String> {
    if name != "rust-desk-light-client.service" {
        return Err(format!("delete is unsupported for {source} unit {name}"));
    }

    let (service_path, disable_args, reload_args) = match source {
        "systemd" => (
            PathBuf::from("/etc/systemd/system").join(name),
            vec!["disable", name],
            vec!["daemon-reload"],
        ),
        "systemd-user" => (
            linux_user_systemd_dir()?.join(name),
            vec!["--user", "disable", name],
            vec!["--user", "daemon-reload"],
        ),
        _ => return Err(format!("unsupported Linux startup source: {source}")),
    };

    startup_command_result(
        run_command("systemctl", &disable_args, 40),
        "disable systemd startup service",
    )?;
    if !service_path.exists() {
        return Err(format!(
            "systemd startup service not found: {}",
            service_path.display()
        ));
    }
    fs::remove_file(&service_path).map_err(|error| {
        format!(
            "delete systemd startup service {} failed: {error}",
            service_path.display()
        )
    })?;
    startup_command_result(
        run_command("systemctl", &reload_args, 40),
        "reload systemd units",
    )
}

fn linux_user_systemd_dir() -> Result<PathBuf, String> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.to_string_lossy().is_empty())
        .map(PathBuf::from)
    {
        Some(path) => path,
        None => home_dir()?.join(".config"),
    };
    Ok(config_home.join("systemd").join("user"))
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
                run_c_locale_command("uptime", &[], 5),
                run_c_locale_command("free", &["-m"], 10),
                run_c_locale_command("df", &["-h", "."], 10),
            ],
        )
    }
}

fn run_c_locale_command(program: &str, args: &[&str], max_lines: usize) -> String {
    run_command_with_env(program, args, &[("LC_ALL", "C")], max_lines)
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
        linux_event_log_summary()
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

fn linux_event_log_summary() -> String {
    let output = run_first_available(
        &[
            ("journalctl", &["-n", "20", "--no-pager", "--output=short-iso"][..]),
            ("dmesg", &["-T"][..]),
        ],
        80,
    );
    if output.starts_with("journalctl failed:") || output.starts_with("dmesg failed:") || output.starts_with("dmesg timed out") {
        return output;
    }

    let mut rows = vec!["Time\tLevel\tProvider\tId\tMessage".to_string()];
    for line in output.lines().filter(|l| !l.trim().is_empty()).take(80) {
        // journalctl --output=short-iso: "2024-01-15T10:30:00+0800 hostname process[pid]: message"
        // dmesg -T: "[Sun Jan 15 10:30:00 2024] kernel: message"
        let trimmed = line.trim();
        let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
        if parts.len() >= 3 {
            let time = parts[0]; // ISO timestamp or dmesg bracket
            // parts[1] is hostname in journalctl output; skip it
            let rest = parts[2]; // "process[pid]: message"
            let proc_msg: Vec<&str> = rest.splitn(2, ": ").collect();
            let provider = proc_msg[0];
            let message = if proc_msg.len() > 1 { proc_msg[1] } else { rest };
            rows.push(format!("{}\t-\t{}\t-\t{}", time, provider, sanitize_table_cell(message)));
        } else {
            rows.push(format!("-\t-\t-\t-\t{}", sanitize_table_cell(trimmed)));
        }
    }
    if rows.len() == 1 {
        rows.push("none\tInfo\t-\t-\tNo recent log events found".to_string());
    }
    rows.join("\n")
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

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "home directory is unavailable".to_string())
}

fn safe_startup_file_stem(name: &str) -> String {
    let mut value = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    value = value
        .trim_matches(|ch| matches!(ch, '-' | '.'))
        .chars()
        .take(64)
        .collect();
    if value.is_empty() {
        "startup-item".to_string()
    } else {
        value
    }
}

fn startup_entry_path(source: &Path, name: &str) -> Result<PathBuf, String> {
    if name.contains('/') || name.contains('\\') || matches!(name, "." | "..") {
        return Err(format!("invalid startup item name: {name}"));
    }
    Ok(source.join(name))
}

fn rename_disabled_startup_file(source: &Path, name: &str, enabled: bool) -> Result<(), String> {
    let path = startup_entry_path(source, name)?;
    if !path.is_file() {
        return Err(format!("startup file not found: {}", path.display()));
    }

    let new_name = if enabled {
        match name.strip_suffix(".disabled") {
            Some(value) => value.to_string(),
            None => return Ok(()),
        }
    } else if name.ends_with(".disabled") {
        return Ok(());
    } else {
        format!("{name}.disabled")
    };

    let target = startup_entry_path(source, &new_name)?;
    if target.exists() {
        return Err(format!(
            "target startup file already exists: {}",
            target.display()
        ));
    }
    fs::rename(&path, &target).map_err(|error| format!("rename startup file failed: {error}"))
}

fn delete_startup_file(source: &str, name: &str) -> Result<(), String> {
    let path = startup_entry_path(Path::new(source), name)?;
    if !path.is_file() {
        return Err(format!("startup file not found: {}", path.display()));
    }
    fs::remove_file(&path).map_err(|error| format!("delete startup file failed: {error}"))
}

fn set_desktop_entry_enabled(source: &Path, name: &str, enabled: bool) -> Result<(), String> {
    let path = startup_entry_path(source, name)?;
    let home_autostart = home_dir()?.join(".config").join("autostart");
    if !enabled && !path.starts_with(&home_autostart) {
        fs::create_dir_all(&home_autostart)
            .map_err(|error| format!("create autostart override directory failed: {error}"))?;
        let override_path = startup_entry_path(&home_autostart, name)?;
        let contents = fs::read_to_string(&path).unwrap_or_else(|_| {
            format!(
                "[Desktop Entry]\nType=Application\nName={}\nExec={}\n",
                desktop_entry_value(name),
                desktop_entry_value(&path.display().to_string())
            )
        });
        let contents = set_desktop_entry_key(&contents, "Hidden", "true");
        fs::write(&override_path, contents)
            .map_err(|error| format!("write autostart override failed: {error}"))?;
        return Ok(());
    }

    let contents =
        fs::read_to_string(&path).map_err(|error| format!("read desktop entry failed: {error}"))?;
    let hidden = if enabled { "false" } else { "true" };
    let autostart_enabled = if enabled { "true" } else { "false" };
    let contents = set_desktop_entry_key(&contents, "Hidden", hidden);
    let contents = set_desktop_entry_key(&contents, "X-GNOME-Autostart-enabled", autostart_enabled);
    fs::write(&path, contents).map_err(|error| format!("write desktop entry failed: {error}"))
}

fn set_desktop_entry_key(contents: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut lines = contents
        .lines()
        .map(|line| {
            if desktop_entry_key_matches(line, key) {
                found = true;
                format!("{key}={value}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>();
    if !found {
        lines.push(format!("{key}={value}"));
    }
    let mut output = lines.join("\n");
    output.push('\n');
    output
}

fn desktop_entry_key_matches(line: &str, key: &str) -> bool {
    line.split_once('=')
        .map(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(key))
        .unwrap_or(false)
}

fn desktop_entry_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_string()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn startup_command_result(output: String, context: &str) -> Result<(), String> {
    let text = output.trim();
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("powershell exited with error")
        || lower.starts_with("systemctl exited with error")
        || lower.starts_with("powershell failed:")
        || lower.starts_with("systemctl failed:")
        || lower.contains(" timed out")
    {
        let inline_text = text.replace(['\r', '\n', '\t'], " ");
        Err(format!("{context} failed: {inline_text}"))
    } else {
        Ok(())
    }
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

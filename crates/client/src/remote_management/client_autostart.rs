use crate::support::{run_command, run_powershell};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::fs;
use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "rust-desk-light";
const AUTOSTART_ITEM_NAME: &str = "rust-desk-light-client";
const WINDOWS_RUN_VALUE: &str = "RustDeskLightClient";
const WINDOWS_RUN_KEY: &str = "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const WINDOWS_RUN_DISABLED_KEY: &str =
    "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\RunDisabled";
const MACOS_LAUNCH_AGENT_LABEL: &str = "com.rust-desk-light.client";
const LINUX_SYSTEMD_SERVICE_NAME: &str = "rust-desk-light-client.service";

pub(super) fn apply_startup_manager_action(action: &str) -> Result<(), String> {
    let paths = AutostartPaths::detect()?;
    match action {
        "enable" => {
            install_current_binary(&paths)?;
            enable_autostart(&paths)
        }
        "disable" => disable_autostart(&paths),
        _ => Err(format!("unsupported client_autostart action: {action}")),
    }
}

pub(super) fn apply_service_manager_action(action: &str) -> Result<(), String> {
    let paths = AutostartPaths::detect_system()?;
    match action {
        "enable" => {
            install_current_binary(&paths)?;
            enable_service(&paths)
        }
        "disable" => disable_service(&paths),
        _ => Err(format!("unsupported client_service action: {action}")),
    }
}

fn enable_service(paths: &AutostartPaths) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        macos_enable_autostart(paths)
    } else if cfg!(target_os = "linux") {
        linux_enable_service(paths)
    } else {
        enable_autostart(paths)
    }
}

fn disable_service(paths: &AutostartPaths) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        rename_autostart_entry_disabled(paths)
    } else if cfg!(target_os = "linux") {
        linux_disable_service(paths)
    } else {
        disable_autostart(paths)
    }
}

fn linux_enable_service(paths: &AutostartPaths) -> Result<(), String> {
    if !linux_is_root_user() {
        return Err("enabling system service requires root privileges".to_string());
    }
    linux_enable_systemd_service(
        &linux_system_service_path(),
        &["daemon-reload"],
        &["enable", LINUX_SYSTEMD_SERVICE_NAME],
        linux_systemd_service_unit(
            &paths.target_exe,
            &paths.config_path,
            &paths.home_dir,
            true,
        ),
        "enable Linux systemd client service",
    )
}

fn linux_disable_service(_paths: &AutostartPaths) -> Result<(), String> {
    if !linux_is_root_user() {
        return Err("disabling system service requires root privileges".to_string());
    }
    let system_service_path = linux_system_service_path();
    if system_service_path.exists() {
        systemctl_result(
            run_command("systemctl", &["disable", LINUX_SYSTEMD_SERVICE_NAME], 40),
            "disable Linux systemd client service",
        )?;
    }
    Ok(())
}

pub(crate) struct AutostartPaths {
    pub(crate) current_exe: PathBuf,
    pub(crate) target_exe: PathBuf,
    pub(crate) entry_path: String,
    pub(crate) config_path: PathBuf,
    pub(crate) home_dir: PathBuf,
}

impl AutostartPaths {
    pub(crate) fn detect() -> Result<Self, String> {
        Self::detect_impl(false)
    }

    pub(crate) fn detect_system() -> Result<Self, String> {
        Self::detect_impl(true)
    }

    fn detect_impl(system: bool) -> Result<Self, String> {
        let current_exe =
            std::env::current_exe().map_err(|error| format!("current exe unavailable: {error}"))?;
        let file_name = current_exe
            .file_name()
            .filter(|name| !name.to_string_lossy().is_empty())
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| default_exe_name().into());
        
        let target_dir = if system {
            system_client_dir()?
        } else {
            stable_client_dir()?
        };
        let target_exe = target_dir.join(file_name);
        
        let home_dir = if system {
            system_home_dir()?
        } else {
            home_dir()?
        };
        
        let config_path = if system {
            target_dir.join(rdl_config::ConfigKind::Client.file_name())
        } else {
            current_client_config_path()
        };

        let entry_path = if cfg!(target_os = "windows") {
            format!("{WINDOWS_RUN_KEY}\\{WINDOWS_RUN_VALUE}")
        } else if cfg!(target_os = "macos") {
            let base = if system {
                PathBuf::from("/Library/LaunchDaemons")
            } else {
                home_dir.join("Library/LaunchAgents")
            };
            base.join(format!("{MACOS_LAUNCH_AGENT_LABEL}.plist"))
                .display()
                .to_string()
        } else {
            let base = if system {
                PathBuf::from("/etc/systemd/system")
            } else {
                home_dir.join(".config/autostart")
            };
            let name = if system {
                LINUX_SYSTEMD_SERVICE_NAME.to_string()
            } else {
                format!("{AUTOSTART_ITEM_NAME}.desktop")
            };
            base.join(name).display().to_string()
        };

        Ok(Self {
            current_exe,
            target_exe,
            entry_path,
            config_path,
            home_dir,
        })
    }
}

fn system_client_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        // SYSTEM AppData
        Ok(PathBuf::from(r"C:\Windows\System32\config\systemprofile\AppData\Roaming").join(APP_DIR_NAME))
    } else if cfg!(target_os = "macos") {
        Ok(PathBuf::from("/Library/Application Support").join(APP_DIR_NAME))
    } else {
        Ok(PathBuf::from("/etc").join(APP_DIR_NAME))
    }
}

fn system_home_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        Ok(PathBuf::from(r"C:\Windows\System32\config\systemprofile"))
    } else if cfg!(target_os = "macos") {
        Ok(PathBuf::from("/var/root"))
    } else {
        Ok(PathBuf::from("/root"))
    }
}

pub(crate) fn install_config(source: &Path, target: &Path) -> Result<(), String> {
    if source == target {
        return Ok(());
    }
    if !source.exists() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create config target directory {} failed: {error}",
                parent.display()
            )
        })?;
    }
    fs::copy(source, target).map_err(|error| {
        format!(
            "copy config from {} to {} failed: {error}",
            source.display(),
            target.display()
        )
    })?;
    Ok(())
}

pub(crate) fn install_current_binary(paths: &AutostartPaths) -> Result<(), String> {
    if same_path(&paths.current_exe, &paths.target_exe) {
        return Ok(());
    }
    if let Some(parent) = paths.target_exe.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create autostart target directory {} failed: {error}",
                parent.display()
            )
        })?;
    }
    fs::copy(&paths.current_exe, &paths.target_exe).map_err(|error| {
        format!(
            "copy client binary from {} to {} failed: {error}",
            paths.current_exe.display(),
            paths.target_exe.display()
        )
    })?;
    Ok(())
}

fn enable_autostart(paths: &AutostartPaths) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        windows_enable_autostart(&paths.target_exe)
    } else if cfg!(target_os = "macos") {
        macos_enable_autostart(paths)
    } else {
        linux_enable_autostart(paths)
    }
}

fn disable_autostart(paths: &AutostartPaths) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        windows_disable_autostart()
    } else if cfg!(target_os = "macos") {
        rename_autostart_entry_disabled(paths)
    } else {
        linux_disable_autostart(paths)
    }
}

fn windows_enable_autostart(target_exe: &Path) -> Result<(), String> {
    let command = windows_run_command(target_exe);
    let script = r#"
$ErrorActionPreference = "Stop"
function Decode($value) {
  [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($value))
}
$key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$disabledKey = "__DISABLED_KEY__"
$name = "RustDeskLightClient"
$value = Decode "__COMMAND_B64__"
if (!(Test-Path $key)) {
  New-Item -Path $key -Force | Out-Null
}
New-ItemProperty -Path $key -Name $name -Value $value -PropertyType String -Force | Out-Null
if (Test-Path $disabledKey) {
  Remove-ItemProperty -Path $disabledKey -Name $name -ErrorAction SilentlyContinue
}
Write-Output "ok"
"#
    .replace("__COMMAND_B64__", &encode_text(&command))
    .replace("__DISABLED_KEY__", WINDOWS_RUN_DISABLED_KEY);
    powershell_result(
        run_powershell(&script, 20),
        "enable Windows login autostart",
    )
}

fn windows_disable_autostart() -> Result<(), String> {
    let script = r#"
$ErrorActionPreference = "Stop"
$key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$disabledKey = "__DISABLED_KEY__"
$name = "RustDeskLightClient"
if (Test-Path $key) {
  $property = (Get-ItemProperty -Path $key).PSObject.Properties | Where-Object { $_.Name -eq $name } | Select-Object -First 1
  if ($null -ne $property) {
    if (!(Test-Path $disabledKey)) {
      New-Item -Path $disabledKey -Force | Out-Null
    }
    New-ItemProperty -Path $disabledKey -Name $name -Value $property.Value -PropertyType String -Force | Out-Null
    Remove-ItemProperty -Path $key -Name $name -ErrorAction Stop
  }
}
Write-Output "ok"
"#
    .replace("__DISABLED_KEY__", WINDOWS_RUN_DISABLED_KEY);
    powershell_result(
        run_powershell(&script, 20),
        "disable Windows login autostart",
    )
}

fn linux_enable_autostart(paths: &AutostartPaths) -> Result<(), String> {
    if !linux_systemctl_available() {
        return Err(
            "Linux client autostart requires systemd/systemctl; desktop autostart is not used"
                .to_string(),
        );
    }

    if linux_is_root_user() {
        linux_enable_systemd_service(
            &linux_system_service_path(),
            &["daemon-reload"],
            &["enable", LINUX_SYSTEMD_SERVICE_NAME],
            linux_systemd_service_unit(
                &paths.target_exe,
                &paths.config_path,
                &paths.home_dir,
                true,
            ),
            "enable Linux systemd client service",
        )?;
    } else {
        linux_enable_systemd_service(
            &linux_user_service_path()?,
            &["--user", "daemon-reload"],
            &["--user", "enable", LINUX_SYSTEMD_SERVICE_NAME],
            linux_systemd_service_unit(
                &paths.target_exe,
                &paths.config_path,
                &paths.home_dir,
                false,
            ),
            "enable Linux user systemd client service",
        )?;
    }

    remove_legacy_linux_desktop_entries(paths)
}

fn linux_disable_autostart(paths: &AutostartPaths) -> Result<(), String> {
    let mut errors = Vec::new();
    let system_service_path = linux_system_service_path();
    if system_service_path.exists() {
        if let Err(error) = systemctl_result(
            run_command("systemctl", &["disable", LINUX_SYSTEMD_SERVICE_NAME], 40),
            "disable Linux systemd client service",
        ) {
            errors.push(error);
        }
    }

    let user_service_path = linux_user_service_path()?;
    if user_service_path.exists() {
        if let Err(error) = systemctl_result(
            run_command(
                "systemctl",
                &["--user", "disable", LINUX_SYSTEMD_SERVICE_NAME],
                40,
            ),
            "disable Linux user systemd client service",
        ) {
            errors.push(error);
        }
    }

    if let Err(error) = remove_legacy_linux_desktop_entries(paths) {
        errors.push(error);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn linux_enable_systemd_service(
    service_path: &Path,
    daemon_reload_args: &[&str],
    enable_args: &[&str],
    unit: String,
    context: &str,
) -> Result<(), String> {
    if let Some(parent) = service_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create systemd service directory {} failed: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(service_path, unit).map_err(|error| {
        format!(
            "write systemd service {} failed: {error}",
            service_path.display()
        )
    })?;
    systemctl_result(
        run_command("systemctl", daemon_reload_args, 40),
        "reload systemd units",
    )?;
    systemctl_result(run_command("systemctl", enable_args, 40), context)
}

fn linux_systemd_service_unit(
    target_exe: &Path,
    config_path: &Path,
    home_dir: &Path,
    system_service: bool,
) -> String {
    let install_target = if system_service {
        "multi-user.target"
    } else {
        "default.target"
    };
    let network_unit = if system_service {
        "Wants=network-online.target\nAfter=network-online.target\n"
    } else {
        ""
    };
    format!(
        "[Unit]\nDescription=rust-desk-light Client\n{network_unit}\n[Service]\nType=simple\nEnvironment=HOME={}\nExecStart={}\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy={install_target}\n",
        systemd_unit_value(home_dir),
        systemd_exec_command(target_exe, config_path)
    )
}

fn systemd_exec_command(target_exe: &Path, config_path: &Path) -> String {
    format!(
        "{} --service --config {}",
        systemd_unit_value(target_exe),
        systemd_unit_value(config_path)
    )
}

fn systemd_unit_value(path: &Path) -> String {
    quote_path(path)
}

fn linux_systemctl_available() -> bool {
    !command_output_failed(&run_command("systemctl", &["--version"], 4), "systemctl")
}

fn linux_is_root_user() -> bool {
    std::env::var("USER")
        .map(|value| value == "root")
        .unwrap_or(false)
        || run_command("id", &["-u"], 4)
            .lines()
            .next()
            .map(|line| line.trim() == "0")
            .unwrap_or(false)
}

fn linux_system_service_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system").join(LINUX_SYSTEMD_SERVICE_NAME)
}

fn linux_user_service_path() -> Result<PathBuf, String> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.to_string_lossy().is_empty())
        .map(PathBuf::from)
    {
        Some(path) => path,
        None => home_dir()?.join(".config"),
    };
    Ok(config_home
        .join("systemd")
        .join("user")
        .join(LINUX_SYSTEMD_SERVICE_NAME))
}

fn remove_legacy_linux_desktop_entries(paths: &AutostartPaths) -> Result<(), String> {
    let entry_path = Path::new(&paths.entry_path);
    let disabled_path = disabled_entry_path(entry_path);
    remove_file_if_exists(entry_path, "remove legacy desktop autostart entry")?;
    remove_file_if_exists(
        &disabled_path,
        "remove legacy disabled desktop autostart entry",
    )
}

fn macos_enable_autostart(paths: &AutostartPaths) -> Result<(), String> {
    let entry_path = Path::new(&paths.entry_path);
    let disabled_path = disabled_entry_path(entry_path);
    if let Some(parent) = entry_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create LaunchAgents directory {} failed: {error}",
                parent.display()
            )
        })?;
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
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
        xml_escape(MACOS_LAUNCH_AGENT_LABEL),
        xml_escape(&path_text(&paths.target_exe))
    );
    fs::write(entry_path, plist).map_err(|error| {
        format!(
            "write launch agent {} failed: {error}",
            entry_path.display()
        )
    })?;
    remove_file_if_exists(&disabled_path, "remove disabled launch agent")
}

fn rename_autostart_entry_disabled(paths: &AutostartPaths) -> Result<(), String> {
    let entry_path = Path::new(&paths.entry_path);
    let disabled_path = disabled_entry_path(entry_path);
    if !entry_path.exists() {
        return Ok(());
    }
    remove_file_if_exists(&disabled_path, "replace disabled autostart entry")?;
    fs::rename(entry_path, &disabled_path).map_err(|error| {
        format!(
            "disable autostart entry {} failed: {error}",
            entry_path.display()
        )
    })
}

fn disabled_entry_path(entry_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.disabled", entry_path.display()))
}

fn remove_file_if_exists(path: &Path, context: &str) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{context} {} failed: {error}", path.display())),
    }
}

fn powershell_result(output: String, context: &str) -> Result<(), String> {
    command_result(output, "powershell", context)
}

fn systemctl_result(output: String, context: &str) -> Result<(), String> {
    command_result(output, "systemctl", context)
}

fn command_result(output: String, program: &str, context: &str) -> Result<(), String> {
    let text = output.trim();
    if command_output_failed(text, program) {
        Err(format!("{context} failed: {text}"))
    } else {
        Ok(())
    }
}

fn command_output_failed(output: &str, program: &str) -> bool {
    let lower = output.trim().to_ascii_lowercase();
    lower.starts_with(&format!("{program} exited with error"))
        || lower.starts_with(&format!("{program} failed:"))
        || lower.contains(" timed out")
}

fn current_client_config_path() -> PathBuf {
    let path = rdl_config::parse_endpoint_args(std::env::args().skip(1))
        .ok()
        .and_then(|parsed| parsed.overrides.config_path)
        .unwrap_or_else(|| rdl_config::default_config_path(rdl_config::ConfigKind::Client));
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(&path))
            .unwrap_or(path)
    }
}

fn stable_client_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .or_else(|| {
                home_dir()
                    .ok()
                    .map(|home| home.join("AppData").join("Local"))
            })
            .map(|path| path.join(APP_DIR_NAME))
            .ok_or_else(|| "LOCALAPPDATA is unavailable".to_string())
    } else if cfg!(target_os = "macos") {
        Ok(home_dir()?
            .join("Library")
            .join("Application Support")
            .join(APP_DIR_NAME))
    } else {
        let data_home = match std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.to_string_lossy().is_empty())
        {
            Some(path) => PathBuf::from(path),
            None => home_dir()?.join(".local").join("share"),
        };
        Ok(data_home.join(APP_DIR_NAME))
    }
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "home directory is unavailable".to_string())
}

fn default_exe_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "rdl-client-gui.exe"
    } else {
        "rdl-client-cli"
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn windows_run_command(path: &Path) -> String {
    quote_path(path)
}

fn quote_path(path: &Path) -> String {
    let text = path_text(path).replace('"', "\\\"");
    format!("\"{text}\"")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn encode_text(value: &str) -> String {
    STANDARD.encode(value)
}

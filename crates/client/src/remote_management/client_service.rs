use crate::support::run_command;
use super::client_autostart::{
    install_config, install_current_binary, linux_enable_systemd_service, linux_is_root_user,
    linux_system_service_path, linux_systemd_service_unit, systemctl_result,
    AutostartPaths, LINUX_SYSTEMD_SERVICE_NAME,
};
use std::fs;
use std::path::PathBuf;

pub(super) fn apply_service_manager_action(action: &str) -> Result<(), String> {
    let paths = AutostartPaths::detect_system()?;
    match action {
        "enable" => {
            let current_paths = AutostartPaths::detect()?;
            install_current_binary(&paths)?;
            install_config(&current_paths.config_path, &paths.config_path)?;
            enable_service(&paths)
        }
        "disable" => disable_service(&paths),
        _ => Err(format!("unsupported client_service action: {action}")),
    }
}

fn enable_service(paths: &AutostartPaths) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        windows_enable_service(paths)
    } else if cfg!(target_os = "macos") {
        macos_enable_service(paths)
    } else {
        linux_enable_service(paths)
    }
}

fn disable_service(paths: &AutostartPaths) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        windows_disable_service()
    } else if cfg!(target_os = "macos") {
        macos_disable_service(paths)
    } else {
        linux_disable_service(paths)
    }
}

fn macos_enable_service(paths: &AutostartPaths) -> Result<(), String> {
    let label = "com.rust-desk-light.client";
    let daemon_dir = PathBuf::from("/Library/LaunchDaemons");
    let plist_path = daemon_dir.join(format!("{label}.plist"));

    fs::create_dir_all(&daemon_dir).map_err(|error| {
        format!("create LaunchDaemons directory failed: {error}")
    })?;

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>--service</string>
        <string>--config</string>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/rust-desk-light-client.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/rust-desk-light-client.log</string>
</dict>
</plist>
"#,
        super::client_autostart::path_text(&paths.target_exe),
        super::client_autostart::path_text(&paths.config_path),
    );

    fs::write(&plist_path, plist).map_err(|error| {
        format!("write launch daemon plist failed: {error}")
    })?;

    let load_output = crate::support::run_command(
        "launchctl",
        &["load", "-w", &plist_path.display().to_string()],
        30,
    );
    super::startup_command_result(load_output, "load macOS launch daemon")
}

fn macos_disable_service(_paths: &AutostartPaths) -> Result<(), String> {
    let label = "com.rust-desk-light.client";
    let daemon_dir = PathBuf::from("/Library/LaunchDaemons");
    let plist_path = daemon_dir.join(format!("{label}.plist"));

    if plist_path.exists() {
        let unload_output = crate::support::run_command(
            "launchctl",
            &["unload", "-w", &plist_path.display().to_string()],
            30,
        );
        super::startup_command_result(unload_output, "unload macOS launch daemon")?;
        fs::remove_file(&plist_path).map_err(|error| {
            format!("remove launch daemon plist failed: {error}")
        })?;
    }

    Ok(())
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

fn windows_enable_service(paths: &AutostartPaths) -> Result<(), String> {
    let exe = paths.target_exe.display().to_string();
    let config_path = paths.config_path.display().to_string();
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
    super::startup_command_result(
        crate::support::run_powershell(&script, 60),
        "enable Windows client service",
    )
}

fn windows_disable_service() -> Result<(), String> {
    let name = "RustDeskLightClientService";
    let script = format!(
        r#"
try {{
    if (Get-Service -Name '{name}' -ErrorAction SilentlyContinue) {{
        Stop-Service -Name '{name}' -Force -ErrorAction SilentlyContinue
        Set-Service -Name '{name}' -StartupType Disabled -ErrorAction Stop
        Write-Host "Service disabled successfully."
    }} else {{
        Write-Host "Service does not exist."
    }}
}} catch {{
    Write-Host $_.Exception.Message
    exit 1
}}
        "#
    );
    super::startup_command_result(
        crate::support::run_powershell(&script, 60),
        "disable Windows client service",
    )
}

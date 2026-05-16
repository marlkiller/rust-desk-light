use super::payload::{clean_result_value, ParsedInteractionPayload};

pub(crate) fn handle(payload: &str, gui_mode: bool) -> String {
    if !gui_mode {
        return super::disabled_detail(&rdl_protocol::CommandKind::BalloonTip);
    }

    let payload = ParsedInteractionPayload::parse(
        payload,
        "Rust Desk Light",
        "Notification from admin.",
        "message_b64",
    );
    match show_notification(&payload.title, &payload.body) {
        Ok(()) => format!(
            "balloon_tip\nstatus=shown\ntitle={}\nmessage={}",
            clean_result_value(&payload.title),
            clean_result_value(&payload.body)
        ),
        Err(error) => {
            println!("admin notification [{}]: {}", payload.title, payload.body);
            format!(
                "balloon_tip_error\nmessage={}\nfallback=printed_to_client_log",
                clean_result_value(&error)
            )
        }
    }
}

#[cfg(target_os = "windows")]
fn show_notification(title: &str, message: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$notify = New-Object System.Windows.Forms.NotifyIcon
$notify.Icon = [System.Drawing.SystemIcons]::Information
$notify.BalloonTipIcon = [System.Windows.Forms.ToolTipIcon]::Info
$notify.BalloonTipTitle = {}
$notify.BalloonTipText = {}
$notify.Visible = $true
$notify.ShowBalloonTip(5000)
Start-Sleep -Seconds 6
$notify.Dispose()
"#,
        super::platform::powershell_string(title),
        super::platform::powershell_string(message)
    );
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("powershell failed: {error}"))
}

#[cfg(target_os = "macos")]
fn show_notification(title: &str, message: &str) -> Result<(), String> {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        super::platform::applescript_string(message),
        super::platform::applescript_string(title)
    );
    super::platform::command_status("osascript", &["-e", &script])
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show_notification(title: &str, message: &str) -> Result<(), String> {
    super::platform::run_first_success(&[
        ("notify-send", vec![title, message]),
        (
            "zenity",
            vec!["--notification", "--text", &format!("{title}: {message}")],
        ),
    ])
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn show_notification(_title: &str, _message: &str) -> Result<(), String> {
    Err("system notifications are not supported on this platform".to_string())
}

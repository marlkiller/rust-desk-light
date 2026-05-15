use base64::{engine::general_purpose::STANDARD, Engine};
use rdl_protocol::CommandKind;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn handle(command: &CommandKind, payload: &str, gui_mode: bool) -> String {
    match command {
        CommandKind::TextChat => {
            if gui_mode {
                "chat_delivered".to_string()
            } else {
                "text_chat requires client GUI".to_string()
            }
        }
        CommandKind::MessageBox => message_box(payload, gui_mode),
        CommandKind::BalloonTip => balloon_tip(payload, gui_mode),
        CommandKind::OpenTextInNotepad => open_text_in_notepad(payload, gui_mode),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

fn message_box(payload: &str, gui_mode: bool) -> String {
    let payload = ParsedInteractionPayload::parse(
        payload,
        "Rust Desk Light",
        "Message from admin.",
        "message_b64",
    );
    if !gui_mode {
        println!("admin message [{}]: {}", payload.title, payload.body);
        return format!(
            "message_box\nstatus=printed_to_client_log\ntitle={}\nmessage={}",
            clean_result_value(&payload.title),
            clean_result_value(&payload.body)
        );
    }
    match show_message_box(&payload.title, &payload.body, payload.kind.as_deref()) {
        Ok(()) => format!(
            "message_box\nstatus=shown\ntitle={}\nmessage={}",
            clean_result_value(&payload.title),
            clean_result_value(&payload.body)
        ),
        Err(error) => {
            println!("admin message [{}]: {}", payload.title, payload.body);
            format!(
                "message_box_error\nmessage={}\nfallback=printed_to_client_log",
                clean_result_value(&error)
            )
        }
    }
}

fn balloon_tip(payload: &str, gui_mode: bool) -> String {
    let payload = ParsedInteractionPayload::parse(
        payload,
        "Rust Desk Light",
        "Notification from admin.",
        "message_b64",
    );
    if !gui_mode {
        println!("admin notification [{}]: {}", payload.title, payload.body);
        return format!(
            "balloon_tip\nstatus=printed_to_client_log\ntitle={}\nmessage={}",
            clean_result_value(&payload.title),
            clean_result_value(&payload.body)
        );
    }
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

fn open_text_in_notepad(payload: &str, gui_mode: bool) -> String {
    let payload =
        ParsedInteractionPayload::parse(payload, "rdl-note.txt", String::new(), "text_b64");
    let file_name = safe_text_file_name(&payload.title);
    if !gui_mode {
        return match write_text_file(&file_name, &payload.body) {
            Ok(path) => format!(
                "open_text_in_notepad\nstatus=written_terminal_mode\npath={}\nbytes={}",
                clean_result_value(&path.display().to_string()),
                payload.body.len()
            ),
            Err(error) => format!(
                "open_text_in_notepad_error\nmessage={}",
                clean_result_value(&error.to_string())
            ),
        };
    }
    match write_text_file(&file_name, &payload.body)
        .and_then(|path| open_text_file(&path).map(|open_status| (path, open_status)))
    {
        Ok((path, open_status)) => format!(
            "open_text_in_notepad\nstatus={open_status}\npath={}\nbytes={}",
            clean_result_value(&path.display().to_string()),
            payload.body.len()
        ),
        Err(error) => format!(
            "open_text_in_notepad_error\nmessage={}",
            clean_result_value(&error.to_string())
        ),
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ParsedInteractionPayload {
    title: String,
    body: String,
    kind: Option<String>,
}

impl ParsedInteractionPayload {
    fn parse(
        payload: &str,
        default_title: impl Into<String>,
        default_body: impl Into<String>,
        encoded_body_key: &str,
    ) -> Self {
        let default_title = default_title.into();
        let default_body = default_body.into();
        let title = payload_field(payload, "title")
            .or_else(|| payload_field(payload, "file_name"))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(default_title);
        let body = payload_field(payload, encoded_body_key)
            .and_then(|value| STANDARD.decode(value).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .or_else(|| payload_field(payload, "message"))
            .or_else(|| payload_field(payload, "text"))
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                let trimmed = payload.trim();
                if trimmed.is_empty() || trimmed.lines().all(|line| line.contains('=')) {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .unwrap_or(default_body);
        let kind = payload_field(payload, "kind").filter(|value| !value.trim().is_empty());

        Self {
            title: single_line(&title),
            body,
            kind: kind.map(|value| value.trim().to_ascii_lowercase()),
        }
    }
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .map(str::to_string)
}

fn single_line(value: &str) -> String {
    let value = value.replace(['\t', '\r', '\n'], " ");
    let value = value.trim();
    if value.is_empty() {
        "Rust Desk Light".to_string()
    } else {
        value.chars().take(120).collect()
    }
}

fn clean_result_value(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

#[cfg(target_os = "windows")]
fn show_message_box(title: &str, message: &str, kind: Option<&str>) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK,
    };

    let title = wide_null(title);
    let message = wide_null(message);
    let icon = match kind.unwrap_or("info") {
        "error" => MB_ICONERROR,
        "warning" | "warn" => MB_ICONWARNING,
        _ => MB_ICONINFORMATION,
    };
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | icon,
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_message_box(title: &str, message: &str, kind: Option<&str>) -> Result<(), String> {
    let icon = match kind.unwrap_or("info") {
        "error" => "stop",
        "warning" | "warn" => "caution",
        _ => "note",
    };
    let script = format!(
        "display dialog \"{}\" with title \"{}\" buttons {{\"OK\"}} default button \"OK\" with icon {icon}",
        applescript_string(message),
        applescript_string(title)
    );
    command_status("osascript", &["-e", &script])
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show_message_box(title: &str, message: &str, _kind: Option<&str>) -> Result<(), String> {
    run_first_success(&[
        (
            "zenity",
            vec!["--info", "--title", title, "--text", message],
        ),
        ("kdialog", vec!["--title", title, "--msgbox", message]),
        ("xmessage", vec!["-center", "-title", title, message]),
    ])
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn show_message_box(_title: &str, _message: &str, _kind: Option<&str>) -> Result<(), String> {
    Err("message box is not supported on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn show_notification(title: &str, message: &str) -> Result<(), String> {
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
        powershell_string(title),
        powershell_string(message)
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
        applescript_string(message),
        applescript_string(title)
    );
    command_status("osascript", &["-e", &script])
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show_notification(title: &str, message: &str) -> Result<(), String> {
    run_first_success(&[
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

fn write_text_file(file_name: &str, text: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("rust-desk-light");
    fs::create_dir_all(&dir)?;
    let path = dir.join(file_name);
    fs::write(&path, text)?;
    Ok(path)
}

fn open_text_file(path: &Path) -> io::Result<&'static str> {
    #[cfg(target_os = "windows")]
    {
        Command::new("notepad")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        return Ok("opened_in_notepad");
    }

    #[cfg(target_os = "macos")]
    {
        let textedit = Command::new("open")
            .args(["-a", "TextEdit"])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if textedit.is_ok() {
            return Ok("opened_in_textedit");
        }
        Command::new("open")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        return Ok("opened_with_default_app");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (program, args) in [
            ("xdg-open", Vec::<&str>::new()),
            ("gedit", Vec::<&str>::new()),
            ("kate", Vec::<&str>::new()),
            ("mousepad", Vec::<&str>::new()),
        ] {
            let result = Command::new(program)
                .args(args)
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if result.is_ok() {
                return Ok("opened_with_platform_editor");
            }
        }
        return Ok("written_no_editor_found");
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        let _ = path;
        Ok("written_no_editor_found")
    }
}

fn safe_text_file_name(value: &str) -> String {
    let mut name = value
        .trim()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>();
    if name.is_empty() || name == "." || name == ".." {
        name = format!("rdl-note-{}.txt", rdl_protocol::now_epoch_ms());
    }
    let has_txt_extension = name.to_ascii_lowercase().ends_with(".txt");
    let max_stem_len = if has_txt_extension { 120 } else { 116 };
    name = name.chars().take(max_stem_len).collect();
    if !has_txt_extension {
        name.push_str(".txt");
    }
    name
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn powershell_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n")
}

#[cfg(any(target_os = "macos", all(unix, not(target_os = "macos"))))]
fn command_status(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("{program} failed: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with error"))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run_first_success(commands: &[(&str, Vec<&str>)]) -> Result<(), String> {
    let mut errors = Vec::new();
    for (program, args) in commands {
        match command_status(program, args) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(error),
        }
    }
    Err(errors
        .last()
        .cloned()
        .unwrap_or_else(|| "no supported GUI command found".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{safe_text_file_name, ParsedInteractionPayload};
    use base64::{engine::general_purpose::STANDARD, Engine};

    #[test]
    fn parses_base64_message_payload() {
        let body = "hello\nworld";
        let payload = format!(
            "title=Notice\nkind=warning\nmessage_b64={}",
            STANDARD.encode(body)
        );

        let parsed = ParsedInteractionPayload::parse(&payload, "Default", "Body", "message_b64");

        assert_eq!(parsed.title, "Notice");
        assert_eq!(parsed.body, body);
        assert_eq!(parsed.kind.as_deref(), Some("warning"));
    }

    #[test]
    fn uses_raw_payload_as_body_for_terminal_commands() {
        let parsed = ParsedInteractionPayload::parse("plain text", "Title", "", "text_b64");

        assert_eq!(parsed.title, "Title");
        assert_eq!(parsed.body, "plain text");
    }

    #[test]
    fn sanitizes_text_file_names() {
        assert_eq!(safe_text_file_name("report:name"), "report_name.txt");
        assert_eq!(safe_text_file_name("already.txt"), "already.txt");
        assert!(safe_text_file_name(&"x".repeat(200)).ends_with(".txt"));
    }
}

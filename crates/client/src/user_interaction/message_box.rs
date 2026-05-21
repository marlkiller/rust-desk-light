use super::payload::{clean_result_value, ParsedInteractionPayload};

pub(crate) fn handle(payload: &str) -> String {
    let payload = ParsedInteractionPayload::parse(
        payload,
        "Rust Desk Light",
        "Message from admin.",
        "message_b64",
    );
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
        super::platform::applescript_string(message),
        super::platform::applescript_string(title)
    );
    super::platform::command_status("osascript", &["-e", &script])
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show_message_box(title: &str, message: &str, _kind: Option<&str>) -> Result<(), String> {
    super::platform::run_first_success(&[
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
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

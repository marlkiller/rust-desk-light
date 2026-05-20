use std::process::Command;
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
pub(crate) struct RemoteDesktopVideoFrame {
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct KeyModifiers {
    pub(crate) shift: bool,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) command: bool,
}

pub(crate) struct RemoteDesktopCapture {
    #[cfg(target_os = "windows")]
    inner: windows::capture::CaptureStream,
    #[cfg(target_os = "linux")]
    inner: linux::capture::CaptureStream,
    #[cfg(target_os = "macos")]
    inner: macos::capture::CaptureStream,
}

impl RemoteDesktopCapture {
    pub(crate) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        {
            return Ok(Self {
                inner: windows::capture::CaptureStream::new(screen_index, quality)?,
            });
        }
        #[cfg(target_os = "linux")]
        {
            return Ok(Self {
                inner: linux::capture::CaptureStream::new(screen_index, quality)?,
            });
        }
        #[cfg(target_os = "macos")]
        {
            return Ok(Self {
                inner: macos::capture::CaptureStream::new(screen_index, quality)?,
            });
        }
        #[allow(unreachable_code)]
        {
            let _ = (screen_index, quality);
            Err("screenshot is not implemented for this platform".to_string())
        }
    }

    pub(crate) fn capture_frame(&mut self) -> Result<RemoteDesktopVideoFrame, String> {
        #[cfg(target_os = "windows")]
        {
            return self.inner.capture_frame();
        }
        #[cfg(target_os = "linux")]
        {
            return self.inner.capture_frame();
        }
        #[cfg(target_os = "macos")]
        {
            return self.inner.capture_frame();
        }
        #[allow(unreachable_code)]
        {
            Err("screenshot is not implemented for this platform".to_string())
        }
    }
}

pub fn handle(payload: &str) -> String {
    let request = RemoteDesktopRequest::parse(payload);
    match request.action.as_str() {
        "screens" => screens(),
        "stop" => stop(),
        "move" => move_mouse(request.x, request.y),
        "click" => click(
            request.x,
            request.y,
            request.button.as_deref().unwrap_or("left"),
        ),
        "mouse_down" => mouse_button(
            request.x,
            request.y,
            request.button.as_deref().unwrap_or("left"),
            true,
        ),
        "mouse_up" => mouse_button(
            request.x,
            request.y,
            request.button.as_deref().unwrap_or("left"),
            false,
        ),
        "key" => send_key(
            request.key.as_deref(),
            request.pressed.unwrap_or(true),
            request.modifiers,
        ),
        "text" => send_text(&request),
        _ => format!(
            "remote_desktop_error\nmessage=unsupported action {}",
            request.action
        ),
    }
}

fn stop() -> String {
    "remote_desktop_stopped\nmessage=stopped".to_string()
}

fn screens() -> String {
    #[cfg(target_os = "windows")]
    {
        return windows::capture::screens();
    }
    #[cfg(target_os = "linux")]
    {
        return linux::capture::screens();
    }
    #[cfg(target_os = "macos")]
    {
        return macos::capture::screens();
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=screen listing is not implemented for this platform"
            .to_string()
    }
}

#[derive(Default)]
struct RemoteDesktopRequest {
    action: String,
    x: Option<i32>,
    y: Option<i32>,
    button: Option<String>,
    key: Option<String>,
    pressed: Option<bool>,
    modifiers: KeyModifiers,
    value: Option<String>,
    value_b64: Option<String>,
}

impl RemoteDesktopRequest {
    fn parse(payload: &str) -> Self {
        let mut request = Self {
            action: "screenshot".to_string(),
            ..Self::default()
        };
        for line in payload.lines() {
            if let Some(rest) = line.strip_prefix("action=") {
                request.action = rest.trim().to_ascii_lowercase();
            } else if let Some(rest) = line.strip_prefix("x=") {
                request.x = rest.trim().parse().ok();
            } else if let Some(rest) = line.strip_prefix("y=") {
                request.y = rest.trim().parse().ok();
            } else if let Some(rest) = line.strip_prefix("button=") {
                request.button = Some(rest.trim().to_ascii_lowercase());
            } else if let Some(rest) = line.strip_prefix("key=") {
                request.key = Some(rest.trim().to_ascii_lowercase());
            } else if let Some(rest) = line.strip_prefix("pressed=") {
                request.pressed = Some(parse_bool(rest));
            } else if let Some(rest) = line.strip_prefix("shift=") {
                request.modifiers.shift = parse_bool(rest);
            } else if let Some(rest) = line.strip_prefix("ctrl=") {
                request.modifiers.ctrl = parse_bool(rest);
            } else if let Some(rest) = line.strip_prefix("alt=") {
                request.modifiers.alt = parse_bool(rest);
            } else if let Some(rest) = line.strip_prefix("command=") {
                request.modifiers.command = parse_bool(rest);
            } else if let Some(rest) = line.strip_prefix("value_b64=") {
                request.value_b64 = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("value=") {
                request.value = Some(rest.to_string());
            }
        }
        request
    }

    fn text_value(&self) -> Result<String, String> {
        if let Some(value_b64) = &self.value_b64 {
            let bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, value_b64)
                    .map_err(|error| format!("invalid text payload: {error}"))?;
            return String::from_utf8(bytes).map_err(|error| format!("invalid text utf8: {error}"));
        }
        Ok(self.value.clone().unwrap_or_default())
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn click(x: Option<i32>, y: Option<i32>, button: &str) -> String {
    let Some(x) = x else {
        return "remote_desktop_error\nmessage=missing x".to_string();
    };
    let Some(y) = y else {
        return "remote_desktop_error\nmessage=missing y".to_string();
    };
    #[cfg(target_os = "windows")]
    {
        return windows::input::click(x, y, button);
    }
    #[cfg(target_os = "linux")]
    {
        return linux::input::click(x, y, button);
    }
    #[cfg(target_os = "macos")]
    {
        return macos::input::click(x, y, button);
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=click is not implemented for this platform".to_string()
    }
}

fn mouse_button(x: Option<i32>, y: Option<i32>, button: &str, down: bool) -> String {
    let Some(x) = x else {
        return "remote_desktop_error\nmessage=missing x".to_string();
    };
    let Some(y) = y else {
        return "remote_desktop_error\nmessage=missing y".to_string();
    };
    #[cfg(target_os = "windows")]
    {
        return windows::input::mouse_button(x, y, button, down);
    }
    #[cfg(target_os = "linux")]
    {
        return linux::input::mouse_button(x, y, button, down);
    }
    #[cfg(target_os = "macos")]
    {
        return macos::input::mouse_button(x, y, button, down);
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=mouse button is not implemented for this platform"
            .to_string()
    }
}

#[allow(dead_code)]
fn click_powershell(x: i32, y: i32, button: &str) -> String {
    let (down, up) = match button {
        "right" => (0x0008, 0x0010),
        _ => (0x0002, 0x0004),
    };
    let script = format!(
        r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class RdlInput {{
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);
}}
"@
[RdlInput]::SetCursorPos({x}, {y}) | Out-Null
[RdlInput]::mouse_event({down}, 0, 0, 0, [UIntPtr]::Zero)
[RdlInput]::mouse_event({up}, 0, 0, 0, [UIntPtr]::Zero)
Write-Output "remote_desktop_input"
Write-Output "message=click {button} {x} {y}"
"#
    );
    run_powershell(&script, Duration::from_secs(2))
}

fn move_mouse(x: Option<i32>, y: Option<i32>) -> String {
    let Some(x) = x else {
        return "remote_desktop_error\nmessage=missing x".to_string();
    };
    let Some(y) = y else {
        return "remote_desktop_error\nmessage=missing y".to_string();
    };
    #[cfg(target_os = "windows")]
    {
        return windows::input::move_mouse(x, y);
    }
    #[cfg(target_os = "linux")]
    {
        return linux::input::move_mouse(x, y);
    }
    #[cfg(target_os = "macos")]
    {
        return macos::input::move_mouse(x, y);
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=mouse move is not implemented for this platform".to_string()
    }
}

#[allow(dead_code)]
fn move_mouse_powershell(x: i32, y: i32) -> String {
    let script = format!(
        r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class RdlMouseMove {{
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
}}
"@
[RdlMouseMove]::SetCursorPos({x}, {y}) | Out-Null
Write-Output "remote_desktop_input"
Write-Output "message=mouse moved {x} {y}"
"#
    );
    run_powershell(&script, Duration::from_secs(2))
}

fn send_key(key: Option<&str>, pressed: bool, modifiers: KeyModifiers) -> String {
    let Some(key) = key.filter(|value| !value.trim().is_empty()) else {
        return "remote_desktop_error\nmessage=missing key".to_string();
    };
    if !pressed {
        return format!("remote_desktop_input\nmessage=key released {key}");
    }
    #[cfg(target_os = "windows")]
    {
        return windows::input::key(key, modifiers);
    }
    #[cfg(target_os = "linux")]
    {
        return linux::input::key(key, modifiers);
    }
    #[cfg(target_os = "macos")]
    {
        return macos::input::key(key, modifiers);
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=keyboard input is not implemented for this platform"
            .to_string()
    }
}

fn send_text(request: &RemoteDesktopRequest) -> String {
    let text = match request.text_value() {
        Ok(text) => text,
        Err(error) => return format!("remote_desktop_error\nmessage={error}"),
    };
    if text.is_empty() {
        return "remote_desktop_error\nmessage=text is empty".to_string();
    }
    #[cfg(target_os = "windows")]
    {
        return windows::input::text(&text);
    }
    #[cfg(target_os = "linux")]
    {
        return linux::input::text(&text);
    }
    #[cfg(target_os = "macos")]
    {
        return macos::input::text(&text);
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=text input is not implemented for this platform".to_string()
    }
}

#[allow(dead_code)]
fn send_text_powershell(text: &str) -> String {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, text);
    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms
$text = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String("{encoded}"))
[System.Windows.Forms.SendKeys]::SendWait($text)
Write-Output "remote_desktop_input"
Write-Output "message=text sent"
"#
    );
    run_powershell(&script, Duration::from_secs(2))
}

fn run_powershell(script: &str, timeout: Duration) -> String {
    let mut child = match Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return format!("remote_desktop_error\nmessage=powershell failed: {error}");
        }
    };

    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                return "remote_desktop_error\nmessage=powershell timeout".to_string();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                return format!("remote_desktop_error\nmessage=powershell wait failed: {error}")
            }
        }
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        Ok(output) => format!(
            "remote_desktop_error\nmessage={}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("remote_desktop_error\nmessage=powershell output failed: {error}"),
    }
}

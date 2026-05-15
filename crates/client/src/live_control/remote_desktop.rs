use std::process::Command;
use std::time::Duration;

pub fn handle(payload: &str) -> String {
    let request = RemoteDesktopRequest::parse(payload);
    match request.action.as_str() {
        "screens" => screens(),
        "screenshot" | "" => screenshot(request.screen.unwrap_or_default()),
        "stop" => stop(),
        "click" => click(
            request.x,
            request.y,
            request.button.as_deref().unwrap_or("left"),
        ),
        "text" => send_text(request.value.as_deref().unwrap_or("")),
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
    if !cfg!(target_os = "windows") {
        return "remote_desktop_error\nmessage=screen listing is currently implemented for windows only"
            .to_string();
    }
    #[cfg(target_os = "windows")]
    {
        return windows_capture::screens();
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=screen listing is currently implemented for windows only"
            .to_string()
    }
}

#[derive(Default)]
struct RemoteDesktopRequest {
    action: String,
    x: Option<i32>,
    y: Option<i32>,
    button: Option<String>,
    value: Option<String>,
    screen: Option<usize>,
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
            } else if let Some(rest) = line.strip_prefix("value=") {
                request.value = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("screen=") {
                request.screen = rest.trim().parse().ok();
            }
        }
        request
    }
}

fn screenshot(screen_index: usize) -> String {
    if !cfg!(target_os = "windows") {
        return "remote_desktop_error\nmessage=screenshot is currently implemented for windows only"
            .to_string();
    }
    #[cfg(target_os = "windows")]
    {
        return windows_capture::screenshot(screen_index);
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=screenshot is currently implemented for windows only"
            .to_string()
    }
}

#[cfg(target_os = "windows")]
mod windows_capture {
    use base64::Engine;
    use image::codecs::jpeg::JpegEncoder;
    use image::{imageops::FilterType, DynamicImage, RgbaImage};
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
        EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
        HMONITOR, MONITORINFOEXW, SRCCOPY,
    };

    const MAX_WIDTH: u32 = 960;
    const JPEG_QUALITY: u8 = 55;

    #[derive(Clone)]
    struct Screen {
        index: usize,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        primary: bool,
        name: String,
    }

    pub(super) fn screens() -> String {
        match enum_screens() {
            Ok(screens) => {
                let mut output = String::from("remote_desktop_screens");
                for screen in screens {
                    output.push_str(&format!(
                        "\nscreen\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        screen.index,
                        screen.x,
                        screen.y,
                        screen.width,
                        screen.height,
                        if screen.primary { "true" } else { "false" },
                        sanitize(&screen.name)
                    ));
                }
                output
            }
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(super) fn screenshot(screen_index: usize) -> String {
        match enum_screens()
            .and_then(|screens| {
                screens
                    .into_iter()
                    .find(|screen| screen.index == screen_index)
                    .ok_or_else(|| format!("screen index {screen_index} is not available"))
            })
            .and_then(capture_screen)
        {
            Ok(frame) => frame,
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    fn enum_screens() -> Result<Vec<Screen>, String> {
        let mut screens = Vec::<Screen>::new();
        let ok = unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_monitor),
                &mut screens as *mut Vec<Screen> as LPARAM,
            )
        };
        if ok == 0 {
            return Err("EnumDisplayMonitors failed".to_string());
        }
        if screens.is_empty() {
            return Err("no display monitors found".to_string());
        }
        Ok(screens)
    }

    unsafe extern "system" fn enum_monitor(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> i32 {
        let screens = &mut *(data as *mut Vec<Screen>);
        let mut info: MONITORINFOEXW = zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut _ as *mut _) == 0 {
            return 1;
        }
        let rect = info.monitorInfo.rcMonitor;
        let width = rect.right.saturating_sub(rect.left).max(0) as u32;
        let height = rect.bottom.saturating_sub(rect.top).max(0) as u32;
        let name = utf16_z_to_string(&info.szDevice);
        screens.push(Screen {
            index: screens.len(),
            x: rect.left,
            y: rect.top,
            width,
            height,
            primary: info.monitorInfo.dwFlags & 1 == 1,
            name,
        });
        1
    }

    fn capture_screen(screen: Screen) -> Result<String, String> {
        if screen.width == 0 || screen.height == 0 {
            return Err("selected screen has invalid size".to_string());
        }
        let rgba = capture_rgba(screen.x, screen.y, screen.width, screen.height)?;
        let image = RgbaImage::from_raw(screen.width, screen.height, rgba)
            .ok_or_else(|| "captured frame buffer has invalid size".to_string())?;
        let scale = (MAX_WIDTH as f32 / screen.width as f32).min(1.0);
        let (image_width, image_height, image) = if scale < 1.0 {
            let width = ((screen.width as f32 * scale).round() as u32).max(1);
            let height = ((screen.height as f32 * scale).round() as u32).max(1);
            let resized = image::imageops::resize(&image, width, height, FilterType::Triangle);
            (width, height, DynamicImage::ImageRgba8(resized))
        } else {
            (screen.width, screen.height, DynamicImage::ImageRgba8(image))
        };
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
            .encode_image(&image)
            .map_err(|error| format!("jpeg encode failed: {error}"))?;
        Ok(format!(
            "remote_desktop_frame\nscreen_index={}\nscreen_width={}\nscreen_height={}\nimage_width={}\nimage_height={}\nformat=jpeg\nbytes={}\npng_base64={}",
            screen.index,
            screen.width,
            screen.height,
            image_width,
            image_height,
            encoded.len(),
            base64::engine::general_purpose::STANDARD.encode(encoded)
        ))
    }

    fn capture_rgba(x: i32, y: i32, width: u32, height: u32) -> Result<Vec<u8>, String> {
        unsafe {
            let screen_dc = GetDC(null_mut());
            if screen_dc.is_null() {
                return Err("GetDC failed".to_string());
            }
            let memory_dc = CreateCompatibleDC(screen_dc);
            if memory_dc.is_null() {
                ReleaseDC(null_mut(), screen_dc);
                return Err("CreateCompatibleDC failed".to_string());
            }
            let bitmap = CreateCompatibleBitmap(screen_dc, width as i32, height as i32);
            if bitmap.is_null() {
                DeleteDC(memory_dc);
                ReleaseDC(null_mut(), screen_dc);
                return Err("CreateCompatibleBitmap failed".to_string());
            }
            let old_object = SelectObject(memory_dc, bitmap as HGDIOBJ);
            let blit_ok = BitBlt(
                memory_dc,
                0,
                0,
                width as i32,
                height as i32,
                screen_dc,
                x,
                y,
                SRCCOPY | CAPTUREBLT,
            );
            let mut buffer = vec![0u8; width as usize * height as usize * 4];
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [zeroed()],
            };
            let dib_lines = if blit_ok != 0 {
                GetDIBits(
                    memory_dc,
                    bitmap as HBITMAP,
                    0,
                    height,
                    buffer.as_mut_ptr() as *mut c_void,
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };
            if !old_object.is_null() {
                SelectObject(memory_dc, old_object);
            }
            DeleteObject(bitmap as HGDIOBJ);
            DeleteDC(memory_dc);
            ReleaseDC(null_mut(), screen_dc);
            if blit_ok == 0 {
                return Err("BitBlt failed".to_string());
            }
            if dib_lines == 0 {
                return Err("GetDIBits failed".to_string());
            }
            for pixel in buffer.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            Ok(buffer)
        }
    }

    fn utf16_z_to_string(value: &[u16]) -> String {
        let len = value
            .iter()
            .position(|item| *item == 0)
            .unwrap_or(value.len());
        String::from_utf16_lossy(&value[..len])
    }

    fn sanitize(value: &str) -> String {
        value.replace(['\t', '\r', '\n'], " ")
    }
}

fn click(x: Option<i32>, y: Option<i32>, button: &str) -> String {
    if !cfg!(target_os = "windows") {
        return "remote_desktop_error\nmessage=click is currently implemented for windows only"
            .to_string();
    }
    let Some(x) = x else {
        return "remote_desktop_error\nmessage=missing x".to_string();
    };
    let Some(y) = y else {
        return "remote_desktop_error\nmessage=missing y".to_string();
    };
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

fn send_text(text: &str) -> String {
    if !cfg!(target_os = "windows") {
        return "remote_desktop_error\nmessage=text input is currently implemented for windows only"
            .to_string();
    }
    if text.is_empty() {
        return "remote_desktop_error\nmessage=text is empty".to_string();
    }
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

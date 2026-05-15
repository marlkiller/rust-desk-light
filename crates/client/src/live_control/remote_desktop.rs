use std::process::Command;
use std::time::Duration;

use base64::Engine;

pub(crate) struct RemoteDesktopVideoFrame {
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

pub fn handle(payload: &str) -> String {
    let request = RemoteDesktopRequest::parse(payload);
    match request.action.as_str() {
        "screens" => screens(),
        "screenshot" | "" => screenshot(
            request.screen.unwrap_or_default(),
            request.quality.as_deref().unwrap_or("medium"),
        ),
        "stop" => stop(),
        "move" => move_mouse(request.x, request.y),
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
    #[cfg(target_os = "windows")]
    {
        return windows_capture::screens();
    }
    #[cfg(target_os = "linux")]
    {
        return linux_capture::screens();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_capture::screens();
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
    value: Option<String>,
    screen: Option<usize>,
    quality: Option<String>,
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
            } else if let Some(rest) = line.strip_prefix("quality=") {
                request.quality = Some(rest.trim().to_ascii_lowercase());
            }
        }
        request
    }
}

fn screenshot(screen_index: usize, quality: &str) -> String {
    match capture_video_frame(screen_index, quality) {
        Ok(frame) => format_frame_payload(screen_index, frame),
        Err(error) => format!("remote_desktop_error\nmessage={error}"),
    }
}

pub(crate) fn capture_video_frame(
    screen_index: usize,
    quality: &str,
) -> Result<RemoteDesktopVideoFrame, String> {
    #[cfg(target_os = "windows")]
    {
        return windows_capture::capture_video_frame(screen_index, quality);
    }
    #[cfg(target_os = "linux")]
    {
        return linux_capture::capture_video_frame(screen_index, quality);
    }
    #[cfg(target_os = "macos")]
    {
        return macos_capture::capture_video_frame(screen_index, quality);
    }
    #[allow(unreachable_code)]
    {
        let _ = (screen_index, quality);
        Err("screenshot is not implemented for this platform".to_string())
    }
}

fn format_frame_payload(screen_index: usize, frame: RemoteDesktopVideoFrame) -> String {
    format!(
        "remote_desktop_frame\nscreen_index={}\nscreen_width={}\nscreen_height={}\nimage_width={}\nimage_height={}\nformat={}\nbytes={}\npng_base64={}",
        screen_index,
        frame.source_width,
        frame.source_height,
        frame.image_width,
        frame.image_height,
        frame.format,
        frame.bytes.len(),
        base64::engine::general_purpose::STANDARD.encode(frame.bytes)
    )
}

#[cfg(target_os = "windows")]
mod windows_capture {
    use super::RemoteDesktopVideoFrame;
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

    pub(super) fn capture_video_frame(
        screen_index: usize,
        quality: &str,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        enum_screens()
            .and_then(|screens| {
                screens
                    .into_iter()
                    .find(|screen| screen.index == screen_index)
                    .ok_or_else(|| format!("screen index {screen_index} is not available"))
            })
            .and_then(|screen| capture_screen(screen, quality_profile(quality)))
    }

    #[derive(Clone, Copy)]
    struct QualityProfile {
        max_width: u32,
        jpeg_quality: u8,
    }

    fn quality_profile(value: &str) -> QualityProfile {
        match value {
            "low" => QualityProfile {
                max_width: 640,
                jpeg_quality: 42,
            },
            "high" => QualityProfile {
                max_width: 1920,
                jpeg_quality: 88,
            },
            _ => QualityProfile {
                max_width: 1280,
                jpeg_quality: 72,
            },
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

    fn capture_screen(
        screen: Screen,
        quality: QualityProfile,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        if screen.width == 0 || screen.height == 0 {
            return Err("selected screen has invalid size".to_string());
        }
        let rgba = capture_rgba(screen.x, screen.y, screen.width, screen.height)?;
        let image = RgbaImage::from_raw(screen.width, screen.height, rgba)
            .ok_or_else(|| "captured frame buffer has invalid size".to_string())?;
        let scale = (quality.max_width as f32 / screen.width as f32).min(1.0);
        let (image_width, image_height, image) = if scale < 1.0 {
            let width = ((screen.width as f32 * scale).round() as u32).max(1);
            let height = ((screen.height as f32 * scale).round() as u32).max(1);
            let resized = image::imageops::resize(&image, width, height, FilterType::Triangle);
            (width, height, DynamicImage::ImageRgba8(resized))
        } else {
            (screen.width, screen.height, DynamicImage::ImageRgba8(image))
        };
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, quality.jpeg_quality)
            .encode_image(&image)
            .map_err(|error| format!("jpeg encode failed: {error}"))?;
        Ok(RemoteDesktopVideoFrame {
            source_width: screen.width,
            source_height: screen.height,
            image_width,
            image_height,
            format: "jpeg".to_string(),
            bytes: encoded,
        })
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

#[cfg(target_os = "linux")]
mod linux_capture {
    use super::RemoteDesktopVideoFrame;
    use image::codecs::jpeg::JpegEncoder;
    use image::{imageops::FilterType, DynamicImage};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

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
            Ok(screens) => format_screens(&screens),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(super) fn capture_video_frame(
        screen_index: usize,
        quality: &str,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        enum_screens()
            .and_then(|screens| {
                screens
                    .into_iter()
                    .find(|screen| screen.index == screen_index)
                    .ok_or_else(|| format!("screen index {screen_index} is not available"))
            })
            .and_then(|screen| capture_screen(screen, quality_profile(quality)))
    }

    #[derive(Clone, Copy)]
    struct QualityProfile {
        max_width: u32,
        jpeg_quality: u8,
    }

    fn quality_profile(value: &str) -> QualityProfile {
        match value {
            "low" => QualityProfile {
                max_width: 640,
                jpeg_quality: 42,
            },
            "high" => QualityProfile {
                max_width: 1920,
                jpeg_quality: 88,
            },
            _ => QualityProfile {
                max_width: 1280,
                jpeg_quality: 72,
            },
        }
    }

    fn enum_screens() -> Result<Vec<Screen>, String> {
        if let Ok(output) = Command::new("xrandr").arg("--query").output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let screens = parse_xrandr(&text);
                if !screens.is_empty() {
                    return Ok(screens);
                }
            }
        }
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            return Err(
                "Wayland screen capture is not available in the lightweight backend; run under X11 or install a portal/scrap backend"
                    .to_string(),
            );
        }
        Err("xrandr was not found or no connected displays were reported".to_string())
    }

    fn parse_xrandr(text: &str) -> Vec<Screen> {
        let mut screens = Vec::new();
        for line in text.lines() {
            if !line.contains(" connected") {
                continue;
            }
            let parts = line.split_whitespace().collect::<Vec<_>>();
            let Some(name) = parts.first() else {
                continue;
            };
            let primary = parts.contains(&"primary");
            let Some(mode) = parts
                .iter()
                .find(|part| parse_geometry(part).is_some())
                .copied()
            else {
                continue;
            };
            let Some((width, height, x, y)) = parse_geometry(mode) else {
                continue;
            };
            screens.push(Screen {
                index: screens.len(),
                x,
                y,
                width,
                height,
                primary,
                name: (*name).to_string(),
            });
        }
        screens
    }

    fn parse_geometry(value: &str) -> Option<(u32, u32, i32, i32)> {
        let (size, rest) = value.split_once('+')?;
        let (width, height) = size.split_once('x')?;
        let (x, y) = rest.split_once('+')?;
        Some((
            width.parse().ok()?,
            height.parse().ok()?,
            x.parse().ok()?,
            y.parse().ok()?,
        ))
    }

    fn capture_screen(
        screen: Screen,
        quality: QualityProfile,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        let path = temp_path("rdl-linux-screen", "jpg");
        let geometry = format!(
            "{}x{}+{}+{}",
            screen.width, screen.height, screen.x, screen.y
        );
        let path_text = path.to_string_lossy().to_string();
        let captured = run_capture_command("maim", &["-g", &geometry, &path_text]).or_else(|_| {
            run_capture_command(
                "import",
                &["-window", "root", "-crop", &geometry, &path_text],
            )
        });
        if captured.is_err() {
            let _ = fs::remove_file(&path);
            return Err(
                "Linux capture requires maim or ImageMagick import on X11; Wayland needs a portal backend"
                    .to_string(),
            );
        }
        let bytes = fs::read(&path).map_err(|error| format!("read screenshot failed: {error}"))?;
        let _ = fs::remove_file(&path);
        encode_frame(screen, bytes, quality)
    }

    fn run_capture_command(program: &str, args: &[&str]) -> Result<(), String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    fn encode_frame(
        screen: Screen,
        bytes: Vec<u8>,
        quality: QualityProfile,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        let image = image::load_from_memory(&bytes)
            .map_err(|error| format!("load captured image failed: {error}"))?;
        let scale = (quality.max_width as f32 / image.width() as f32).min(1.0);
        let (image_width, image_height, image) = if scale < 1.0 {
            let width = ((image.width() as f32 * scale).round() as u32).max(1);
            let height = ((image.height() as f32 * scale).round() as u32).max(1);
            let resized = image::imageops::resize(&image, width, height, FilterType::Triangle);
            (width, height, DynamicImage::ImageRgba8(resized))
        } else {
            (image.width(), image.height(), image)
        };
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, quality.jpeg_quality)
            .encode_image(&image)
            .map_err(|error| format!("jpeg encode failed: {error}"))?;
        Ok(RemoteDesktopVideoFrame {
            source_width: screen.width,
            source_height: screen.height,
            image_width,
            image_height,
            format: "jpeg".to_string(),
            bytes: encoded,
        })
    }

    fn format_screens(screens: &[Screen]) -> String {
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

    fn temp_path(prefix: &str, ext: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            rdl_protocol::now_epoch_ms(),
            ext
        ))
    }

    fn sanitize(value: &str) -> String {
        value.replace(['\t', '\r', '\n'], " ")
    }
}

#[cfg(target_os = "macos")]
mod macos_capture {
    use super::RemoteDesktopVideoFrame;
    use image::codecs::jpeg::JpegEncoder;
    use image::{imageops::FilterType, DynamicImage};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

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
            Ok(screens) => format_screens(&screens),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(super) fn capture_video_frame(
        screen_index: usize,
        quality: &str,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        enum_screens()
            .and_then(|screens| {
                screens
                    .into_iter()
                    .find(|screen| screen.index == screen_index)
                    .ok_or_else(|| format!("screen index {screen_index} is not available"))
            })
            .and_then(|screen| capture_screen(screen, quality_profile(quality)))
    }

    #[derive(Clone, Copy)]
    struct QualityProfile {
        max_width: u32,
        jpeg_quality: u8,
    }

    fn quality_profile(value: &str) -> QualityProfile {
        match value {
            "low" => QualityProfile {
                max_width: 640,
                jpeg_quality: 42,
            },
            "high" => QualityProfile {
                max_width: 1920,
                jpeg_quality: 88,
            },
            _ => QualityProfile {
                max_width: 1280,
                jpeg_quality: 72,
            },
        }
    }

    fn enum_screens() -> Result<Vec<Screen>, String> {
        let script = r#"tell application "Finder" to get bounds of window of desktop"#;
        let output = Command::new("osascript")
            .args(["-e", script])
            .output()
            .map_err(|error| format!("osascript failed: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let values = text
            .split(',')
            .filter_map(|item| item.trim().parse::<i32>().ok())
            .collect::<Vec<_>>();
        if values.len() != 4 {
            return Err("could not read macOS desktop bounds".to_string());
        }
        let width = values[2].saturating_sub(values[0]).max(1) as u32;
        let height = values[3].saturating_sub(values[1]).max(1) as u32;
        Ok(vec![Screen {
            index: 0,
            x: values[0],
            y: values[1],
            width,
            height,
            primary: true,
            name: "Main Display".to_string(),
        }])
    }

    fn capture_screen(
        screen: Screen,
        quality: QualityProfile,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        let path = temp_path("rdl-macos-screen", "jpg");
        let rect = format!(
            "{},{},{},{}",
            screen.x, screen.y, screen.width, screen.height
        );
        let path_text = path.to_string_lossy().to_string();
        let output = Command::new("screencapture")
            .args(["-x", "-t", "jpg", "-R", &rect, &path_text])
            .output()
            .map_err(|error| format!("screencapture failed: {error}"))?;
        if !output.status.success() {
            let _ = fs::remove_file(&path);
            return Err(format!(
                "screencapture failed; grant Screen Recording permission: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let bytes = fs::read(&path).map_err(|error| format!("read screenshot failed: {error}"))?;
        let _ = fs::remove_file(&path);
        let image = image::load_from_memory(&bytes)
            .map_err(|error| format!("load captured image failed: {error}"))?;
        let scale = (quality.max_width as f32 / image.width() as f32).min(1.0);
        let (image_width, image_height, image) = if scale < 1.0 {
            let width = ((image.width() as f32 * scale).round() as u32).max(1);
            let height = ((image.height() as f32 * scale).round() as u32).max(1);
            let resized = image::imageops::resize(&image, width, height, FilterType::Triangle);
            (width, height, DynamicImage::ImageRgba8(resized))
        } else {
            (image.width(), image.height(), image)
        };
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, quality.jpeg_quality)
            .encode_image(&image)
            .map_err(|error| format!("jpeg encode failed: {error}"))?;
        Ok(RemoteDesktopVideoFrame {
            source_width: screen.width,
            source_height: screen.height,
            image_width,
            image_height,
            format: "jpeg".to_string(),
            bytes: encoded,
        })
    }

    fn format_screens(screens: &[Screen]) -> String {
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

    fn temp_path(prefix: &str, ext: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            rdl_protocol::now_epoch_ms(),
            ext
        ))
    }

    fn sanitize(value: &str) -> String {
        value.replace(['\t', '\r', '\n'], " ")
    }
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
        return windows_input::click(x, y, button);
    }
    #[cfg(target_os = "linux")]
    {
        return linux_input::click(x, y, button);
    }
    #[cfg(target_os = "macos")]
    {
        return macos_input::click(x, y, button);
    }
    #[allow(unreachable_code)]
    {
        "remote_desktop_error\nmessage=click is not implemented for this platform".to_string()
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
        return windows_input::move_mouse(x, y);
    }
    #[cfg(target_os = "linux")]
    {
        return linux_input::move_mouse(x, y);
    }
    #[cfg(target_os = "macos")]
    {
        return macos_input::move_mouse(x, y);
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

#[cfg(target_os = "windows")]
mod windows_input {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        mouse_event, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

    pub(super) fn move_mouse(x: i32, y: i32) -> String {
        let ok = unsafe { SetCursorPos(x, y) };
        if ok == 0 {
            return "remote_desktop_error\nmessage=SetCursorPos failed".to_string();
        }
        format!("remote_desktop_input\nmessage=mouse moved {x} {y}")
    }

    pub(super) fn click(x: i32, y: i32, button: &str) -> String {
        let ok = unsafe { SetCursorPos(x, y) };
        if ok == 0 {
            return "remote_desktop_error\nmessage=SetCursorPos failed".to_string();
        }
        let (down, up) = match button {
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        };
        unsafe {
            mouse_event(down, 0, 0, 0, 0);
            mouse_event(up, 0, 0, 0, 0);
        }
        format!("remote_desktop_input\nmessage=click {button} {x} {y}")
    }
}

#[cfg(target_os = "linux")]
mod linux_input {
    use std::process::Command;

    pub(super) fn move_mouse(x: i32, y: i32) -> String {
        let x = x.to_string();
        let y = y.to_string();
        match run_xdotool(&["mousemove", &x, &y]) {
            Ok(()) => format!("remote_desktop_input\nmessage=mouse moved {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(super) fn click(x: i32, y: i32, button: &str) -> String {
        let button_id = if button == "right" { "3" } else { "1" };
        let x = x.to_string();
        let y = y.to_string();
        match run_xdotool(&["mousemove", &x, &y, "click", button_id]) {
            Ok(()) => format!("remote_desktop_input\nmessage=click {button} {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    fn run_xdotool(args: &[&str]) -> Result<(), String> {
        if std::env::var("WAYLAND_DISPLAY").is_ok() && std::env::var("DISPLAY").is_err() {
            return Err(
                "Linux input currently requires X11 xdotool; Wayland needs ydotool/portal backend"
                    .to_string(),
            );
        }
        let output = Command::new("xdotool")
            .args(args)
            .output()
            .map_err(|error| format!("xdotool failed: {error}; install xdotool for X11 input"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "xdotool failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_input {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;
    use std::thread;
    use std::time::Duration;

    pub(super) fn move_mouse(x: i32, y: i32) -> String {
        match post_move(x, y) {
            Ok(()) => format!("remote_desktop_input\nmessage=mouse moved {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(super) fn click(x: i32, y: i32, button: &str) -> String {
        match post_click(x, y, button) {
            Ok(()) => format!("remote_desktop_input\nmessage=click {button} {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    fn post_move(x: i32, y: i32) -> Result<(), String> {
        ensure_accessibility_permission()?;
        let source = event_source()?;
        post_mouse_event(&source, CGEventType::MouseMoved, CGMouseButton::Left, x, y)
    }

    fn post_click(x: i32, y: i32, button: &str) -> Result<(), String> {
        ensure_accessibility_permission()?;
        let source = event_source()?;
        let (down, up, mouse_button) = match button {
            "right" => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
            _ => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
        };
        post_mouse_event(&source, CGEventType::MouseMoved, mouse_button, x, y)?;
        post_mouse_event(&source, down, mouse_button, x, y)?;
        thread::sleep(Duration::from_millis(20));
        post_mouse_event(&source, up, mouse_button, x, y)
    }

    fn event_source() -> Result<CGEventSource, String> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "CGEventSourceCreate failed".to_string())
    }

    fn post_mouse_event(
        source: &CGEventSource,
        event_type: CGEventType,
        button: CGMouseButton,
        x: i32,
        y: i32,
    ) -> Result<(), String> {
        let point = CGPoint::new(x as f64, y as f64);
        let event = CGEvent::new_mouse_event(source.clone(), event_type, point, button)
            .map_err(|_| "CGEventCreateMouseEvent failed".to_string())?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn ensure_accessibility_permission() -> Result<(), String> {
        if accessibility_trusted(false) || accessibility_trusted(true) {
            Ok(())
        } else {
            Err(format!(
                "macOS input requires Accessibility permission for the running client process. Enable this exact executable in System Settings > Privacy & Security > Accessibility, then restart/reconnect the client. executable={}",
                current_executable_label()
            ))
        }
    }

    fn current_executable_label() -> String {
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|error| format!("unknown ({error})"))
    }

    fn accessibility_trusted(prompt: bool) -> bool {
        if !prompt {
            return unsafe { AXIsProcessTrusted() != 0 };
        }

        unsafe {
            let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
            let value = CFBoolean::true_value();
            let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0
        }
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    }
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

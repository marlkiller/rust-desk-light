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

pub(crate) struct RemoteDesktopCapture {
    #[cfg(target_os = "windows")]
    inner: windows_capture::CaptureStream,
    #[cfg(target_os = "linux")]
    inner: linux_capture::CaptureStream,
    #[cfg(target_os = "macos")]
    inner: macos_capture::CaptureStream,
}

impl RemoteDesktopCapture {
    pub(crate) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        {
            return Ok(Self {
                inner: windows_capture::CaptureStream::new(screen_index, quality)?,
            });
        }
        #[cfg(target_os = "linux")]
        {
            return Ok(Self {
                inner: linux_capture::CaptureStream::new(screen_index, quality)?,
            });
        }
        #[cfg(target_os = "macos")]
        {
            return Ok(Self {
                inner: macos_capture::CaptureStream::new(screen_index, quality)?,
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
        CaptureStream::new(screen_index, quality).and_then(|mut capture| capture.capture_frame())
    }

    pub(super) struct CaptureStream {
        screen: Screen,
        quality: QualityProfile,
        screen_dc: HDC,
        memory_dc: HDC,
        bitmap: HBITMAP,
        old_object: HGDIOBJ,
        buffer: Vec<u8>,
        info: BITMAPINFO,
    }

    impl CaptureStream {
        pub(super) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
            let screen = enum_screens().and_then(|screens| {
                screens
                    .into_iter()
                    .find(|screen| screen.index == screen_index)
                    .ok_or_else(|| format!("screen index {screen_index} is not available"))
            })?;
            if screen.width == 0 || screen.height == 0 {
                return Err("selected screen has invalid size".to_string());
            }
            let width = screen.width;
            let height = screen.height;
            let buffer_len = width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| "selected screen is too large".to_string())?
                as usize;
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
                if old_object.is_null() {
                    DeleteObject(bitmap as HGDIOBJ);
                    DeleteDC(memory_dc);
                    ReleaseDC(null_mut(), screen_dc);
                    return Err("SelectObject failed".to_string());
                }
                Ok(Self {
                    screen,
                    quality: quality_profile(quality),
                    screen_dc,
                    memory_dc,
                    bitmap,
                    old_object,
                    buffer: vec![0u8; buffer_len],
                    info: BITMAPINFO {
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
                    },
                })
            }
        }

        pub(super) fn capture_frame(&mut self) -> Result<RemoteDesktopVideoFrame, String> {
            let blit_ok = unsafe {
                BitBlt(
                    self.memory_dc,
                    0,
                    0,
                    self.screen.width as i32,
                    self.screen.height as i32,
                    self.screen_dc,
                    self.screen.x,
                    self.screen.y,
                    SRCCOPY | CAPTUREBLT,
                )
            };
            if blit_ok == 0 {
                return Err("BitBlt failed".to_string());
            }
            let dib_lines = unsafe {
                GetDIBits(
                    self.memory_dc,
                    self.bitmap,
                    0,
                    self.screen.height,
                    self.buffer.as_mut_ptr() as *mut c_void,
                    &mut self.info,
                    DIB_RGB_COLORS,
                )
            };
            if dib_lines == 0 {
                return Err("GetDIBits failed".to_string());
            }
            let mut rgba = self.buffer.clone();
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }

            let image = RgbaImage::from_raw(self.screen.width, self.screen.height, rgba)
                .ok_or_else(|| "captured frame buffer has invalid size".to_string())?;
            let scale = (self.quality.max_width as f32 / self.screen.width as f32).min(1.0);
            let (image_width, image_height, output_image) = if scale < 1.0 {
                let width = ((self.screen.width as f32 * scale).round() as u32).max(1);
                let height = ((self.screen.height as f32 * scale).round() as u32).max(1);
                let resized = image::imageops::resize(&image, width, height, FilterType::Triangle);
                (width, height, DynamicImage::ImageRgba8(resized))
            } else {
                (
                    self.screen.width,
                    self.screen.height,
                    DynamicImage::ImageRgba8(image),
                )
            };
            let mut encoded = Vec::new();
            JpegEncoder::new_with_quality(&mut encoded, self.quality.jpeg_quality)
                .encode_image(&output_image)
                .map_err(|error| format!("jpeg encode failed: {error}"))?;
            Ok(RemoteDesktopVideoFrame {
                source_width: self.screen.width,
                source_height: self.screen.height,
                image_width,
                image_height,
                format: "jpeg".to_string(),
                bytes: encoded,
            })
        }
    }

    impl Drop for CaptureStream {
        fn drop(&mut self) {
            unsafe {
                if !self.old_object.is_null() {
                    SelectObject(self.memory_dc, self.old_object);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap as HGDIOBJ);
                }
                if !self.memory_dc.is_null() {
                    DeleteDC(self.memory_dc);
                }
                if !self.screen_dc.is_null() {
                    ReleaseDC(null_mut(), self.screen_dc);
                }
            }
        }
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
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
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
        CaptureStream::new(screen_index, quality).and_then(|mut capture| capture.capture_frame())
    }

    pub(super) struct CaptureStream {
        screen: Screen,
        quality: QualityProfile,
        geometry: String,
        backends: Vec<CaptureBackend>,
        active_backend: usize,
    }

    impl CaptureStream {
        pub(super) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
            let screen = enum_screens().and_then(|screens| {
                screens
                    .into_iter()
                    .find(|screen| screen.index == screen_index)
                    .ok_or_else(|| format!("screen index {screen_index} is not available"))
            })?;
            let geometry = screen_geometry(&screen);
            Ok(Self {
                screen,
                quality: quality_profile(quality),
                geometry,
                backends: capture_backends()?,
                active_backend: 0,
            })
        }

        pub(super) fn capture_frame(&mut self) -> Result<RemoteDesktopVideoFrame, String> {
            let mut last_error = String::new();
            for offset in 0..self.backends.len() {
                let index = (self.active_backend + offset) % self.backends.len();
                match self.backends[index]
                    .capture(&self.geometry)
                    .and_then(|bytes| encode_frame(self.screen.clone(), bytes, self.quality))
                {
                    Ok(frame) => {
                        self.active_backend = index;
                        return Ok(frame);
                    }
                    Err(error) => {
                        last_error = error;
                    }
                }
            }
            Err(if last_error.trim().is_empty() {
                "Linux capture requires maim or ImageMagick import on X11; Wayland needs a portal backend".to_string()
            } else {
                last_error
            })
        }
    }

    #[derive(Clone, Copy)]
    enum CaptureBackend {
        MaimStdout,
        MaimFile,
        ImportStdout,
    }

    impl CaptureBackend {
        fn capture(self, geometry: &str) -> Result<Vec<u8>, String> {
            match self {
                Self::MaimStdout => run_capture_stdout("maim", &["-f", "jpg", "-g", geometry]),
                Self::MaimFile => {
                    let path = temp_path("rdl-linux-screen", "jpg");
                    let path_text = path.to_string_lossy().to_string();
                    run_capture_file("maim", &["-g", geometry, &path_text], &path)
                }
                Self::ImportStdout => {
                    run_capture_stdout("import", &["-window", "root", "-crop", geometry, "jpg:-"])
                }
            }
        }
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

    fn capture_backends() -> Result<Vec<CaptureBackend>, String> {
        let mut backends = Vec::new();
        if command_in_path("maim") {
            backends.push(CaptureBackend::MaimStdout);
            backends.push(CaptureBackend::MaimFile);
        }
        if command_in_path("import") {
            backends.push(CaptureBackend::ImportStdout);
        }
        if !backends.is_empty() {
            return Ok(backends);
        }
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            return Err(
                "Wayland screen capture is not available in the lightweight backend; run under X11 or install a portal/scrap backend"
                    .to_string(),
            );
        }
        Err("Linux capture requires maim or ImageMagick import on X11".to_string())
    }

    fn command_in_path(program: &str) -> bool {
        let Some(paths) = env::var_os("PATH") else {
            return false;
        };
        env::split_paths(&paths).any(|dir| dir.join(program).is_file())
    }

    fn run_capture_stdout(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        if output.stdout.is_empty() {
            return Err(format!("{program} produced an empty screenshot"));
        }
        Ok(output.stdout)
    }

    fn run_capture_file(program: &str, args: &[&str], path: &Path) -> Result<Vec<u8>, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            let _ = fs::remove_file(path);
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        let bytes = fs::read(path).map_err(|error| format!("read screenshot failed: {error}"))?;
        let _ = fs::remove_file(path);
        if bytes.is_empty() {
            return Err(format!("{program} produced an empty screenshot"));
        }
        Ok(bytes)
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

    fn screen_geometry(screen: &Screen) -> String {
        format!(
            "{}x{}+{}+{}",
            screen.width, screen.height, screen.x, screen.y
        )
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
    use core_graphics::display::{CGDirectDisplayID, CGDisplay};
    use core_graphics::image::CGImage;
    use image::codecs::jpeg::JpegEncoder;
    use image::{imageops::FilterType, DynamicImage, RgbaImage};

    #[derive(Clone)]
    struct Screen {
        index: usize,
        display_id: CGDirectDisplayID,
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
        CaptureStream::new(screen_index, quality).and_then(|mut capture| capture.capture_frame())
    }

    pub(super) struct CaptureStream {
        screen: Screen,
        quality: QualityProfile,
        display: CGDisplay,
        rgba: Vec<u8>,
    }

    impl CaptureStream {
        pub(super) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
            let screen = enum_screens().and_then(|screens| {
                screens
                    .into_iter()
                    .find(|screen| screen.index == screen_index)
                    .ok_or_else(|| format!("screen index {screen_index} is not available"))
            })?;
            let display = CGDisplay::new(screen.display_id);
            Ok(Self {
                screen,
                quality: quality_profile(quality),
                display,
                rgba: Vec::new(),
            })
        }

        pub(super) fn capture_frame(&mut self) -> Result<RemoteDesktopVideoFrame, String> {
            let capture = self.display.image().ok_or_else(|| {
                "CoreGraphics capture failed; grant Screen Recording permission to the client"
                    .to_string()
            })?;
            encode_capture(&self.screen, &capture, self.quality, &mut self.rgba)
        }
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
        let displays = CGDisplay::active_displays()
            .map_err(|error| format!("CGGetActiveDisplayList failed: {error}"))?;
        let mut screens = Vec::new();
        for display_id in displays {
            let display = CGDisplay::new(display_id);
            if !display.is_active() || display.is_asleep() {
                continue;
            }
            let bounds = display.bounds();
            let width = bounds.size.width.round().max(1.0) as u32;
            let height = bounds.size.height.round().max(1.0) as u32;
            screens.push(Screen {
                index: screens.len(),
                display_id,
                x: bounds.origin.x.round() as i32,
                y: bounds.origin.y.round() as i32,
                width,
                height,
                primary: display.is_main(),
                name: format!(
                    "Display {} ({}x{})",
                    display.unit_number(),
                    display.pixels_wide(),
                    display.pixels_high()
                ),
            });
        }
        if screens.is_empty() {
            Err("no active macOS displays found".to_string())
        } else {
            Ok(screens)
        }
    }

    fn encode_capture(
        screen: &Screen,
        capture: &CGImage,
        quality: QualityProfile,
        rgba: &mut Vec<u8>,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        let (width, height) = cg_image_to_rgba_buffer(capture, rgba)?;
        let rgba_buffer = std::mem::take(rgba);
        let image = RgbaImage::from_raw(width, height, rgba_buffer)
            .ok_or_else(|| "captured display buffer has invalid size".to_string())?;
        let scale = (quality.max_width as f32 / image.width() as f32).min(1.0);
        let recycle_output = scale >= 1.0;
        let (image_width, image_height, image) = if scale < 1.0 {
            let width = ((image.width() as f32 * scale).round() as u32).max(1);
            let height = ((image.height() as f32 * scale).round() as u32).max(1);
            let resized = image::imageops::resize(&image, width, height, FilterType::Triangle);
            *rgba = image.into_raw();
            (width, height, DynamicImage::ImageRgba8(resized))
        } else {
            (
                image.width(),
                image.height(),
                DynamicImage::ImageRgba8(image),
            )
        };
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, quality.jpeg_quality)
            .encode_image(&image)
            .map_err(|error| format!("jpeg encode failed: {error}"))?;
        if recycle_output {
            if let DynamicImage::ImageRgba8(image) = image {
                *rgba = image.into_raw();
            }
        }
        Ok(RemoteDesktopVideoFrame {
            source_width: screen.width,
            source_height: screen.height,
            image_width,
            image_height,
            format: "jpeg".to_string(),
            bytes: encoded,
        })
    }

    fn cg_image_to_rgba_buffer(image: &CGImage, rgba: &mut Vec<u8>) -> Result<(u32, u32), String> {
        let width = image.width() as u32;
        let height = image.height() as u32;
        if width == 0 || height == 0 {
            return Err("captured display image is empty".to_string());
        }
        if image.bits_per_component() != 8 || image.bits_per_pixel() != 32 {
            return Err(format!(
                "unsupported macOS screen pixel format: {} bpc, {} bpp",
                image.bits_per_component(),
                image.bits_per_pixel()
            ));
        }

        let bytes_per_row = image.bytes_per_row();
        let row_len = width as usize * 4;
        let required = bytes_per_row
            .checked_mul(height as usize)
            .ok_or_else(|| "captured display buffer is too large".to_string())?;
        let data = image.data();
        let bytes = data.bytes();
        if bytes_per_row < row_len || bytes.len() < required {
            return Err("captured display buffer has invalid stride".to_string());
        }

        rgba.clear();
        rgba.resize(row_len * height as usize, 0);
        let mut dst = 0;
        for y in 0..height as usize {
            let offset = y * bytes_per_row;
            let row = &bytes[offset..offset + row_len];
            for pixel in row.chunks_exact(4) {
                rgba[dst] = pixel[2];
                rgba[dst + 1] = pixel[1];
                rgba[dst + 2] = pixel[0];
                rgba[dst + 3] = pixel[3];
                dst += 4;
            }
        }
        Ok((width, height))
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

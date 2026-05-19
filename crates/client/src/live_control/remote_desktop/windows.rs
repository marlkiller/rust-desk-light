pub(crate) mod capture {
    use super::super::RemoteDesktopVideoFrame;
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

    pub(crate) fn screens() -> String {
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

    pub(crate) fn capture_video_frame(
        screen_index: usize,
        quality: &str,
    ) -> Result<RemoteDesktopVideoFrame, String> {
        CaptureStream::new(screen_index, quality).and_then(|mut capture| capture.capture_frame())
    }

    pub(crate) struct CaptureStream {
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
        pub(crate) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
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

        pub(crate) fn capture_frame(&mut self) -> Result<RemoteDesktopVideoFrame, String> {
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

pub(crate) mod input {
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    pub(crate) fn move_mouse(x: i32, y: i32) -> String {
        if let Err(error) = send_mouse_inputs(&[move_input(x, y)]) {
            return format!("remote_desktop_error\nmessage={error}");
        }
        format!("remote_desktop_input\nmessage=mouse moved {x} {y}")
    }

    pub(crate) fn click(x: i32, y: i32, button: &str) -> String {
        let (down, up) = match button {
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        };
        if let Err(error) =
            send_mouse_inputs(&[move_input(x, y), button_input(down), button_input(up)])
        {
            return format!("remote_desktop_error\nmessage={error}");
        }
        format!("remote_desktop_input\nmessage=click {button} {x} {y}")
    }

    fn move_input(x: i32, y: i32) -> INPUT {
        let (dx, dy) = absolute_virtual_desktop_point(x, y);
        mouse_input(
            dx,
            dy,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )
    }

    fn button_input(flags: u32) -> INPUT {
        mouse_input(0, 0, flags)
    }

    fn mouse_input(dx: i32, dy: i32, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn absolute_virtual_desktop_point(x: i32, y: i32) -> (i32, i32) {
        let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
        let right = left.saturating_add(width.saturating_sub(1));
        let bottom = top.saturating_add(height.saturating_sub(1));
        let x = x.clamp(left, right);
        let y = y.clamp(top, bottom);
        let dx = ((x.saturating_sub(left)) as i64 * 65_535 / width.saturating_sub(1).max(1) as i64)
            as i32;
        let dy = ((y.saturating_sub(top)) as i64 * 65_535 / height.saturating_sub(1).max(1) as i64)
            as i32;
        (dx, dy)
    }

    fn send_mouse_inputs(inputs: &[INPUT]) -> Result<(), String> {
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            )
        };
        if sent == inputs.len() as u32 {
            return Ok(());
        }
        let code = unsafe { GetLastError() };
        Err(format!(
            "SendInput failed: sent {sent}/{} events, error={code}",
            inputs.len()
        ))
    }
}

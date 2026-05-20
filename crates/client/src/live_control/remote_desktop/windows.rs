pub(crate) mod capture {
    use super::super::{FrameChangeDetector, RemoteDesktopVideoFrame};
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{GetLastError, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
        EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject,
        SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, HALFTONE, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFOEXW, SRCCOPY,
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

    pub(crate) struct CaptureStream {
        screen: Screen,
        quality: QualityProfile,
        image_width: u32,
        image_height: u32,
        bgra_buffer: Vec<u8>,
        rgb_buffer: Vec<u8>,
        resources: CaptureResources,
        change_detector: FrameChangeDetector,
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
            let quality = quality_profile(quality);
            let (image_width, image_height) =
                scaled_size(screen.width, screen.height, quality.max_width);
            let resources = CaptureResources::new(image_width, image_height)?;
            Ok(Self {
                screen,
                quality,
                image_width,
                image_height,
                bgra_buffer: Vec::new(),
                rgb_buffer: Vec::new(),
                resources,
                change_detector: FrameChangeDetector::default(),
            })
        }

        pub(crate) fn capture_frame(&mut self) -> Result<Option<RemoteDesktopVideoFrame>, String> {
            self.resources
                .capture_bgra(
                    self.screen.x,
                    self.screen.y,
                    self.screen.width,
                    self.screen.height,
                    &mut self.bgra_buffer,
                )
                .map_err(|error| {
                    format!(
                        "{error} screen={} origin={},{} size={}x{}",
                        self.screen.index,
                        self.screen.x,
                        self.screen.y,
                        self.screen.width,
                        self.screen.height
                    )
                })?;
            if !self.change_detector.should_send(&self.bgra_buffer) {
                return Ok(None);
            }
            write_rgb_from_bgra(&self.bgra_buffer, &mut self.rgb_buffer)?;
            let encoded = encode_rgb_jpeg(
                &self.rgb_buffer,
                self.image_width,
                self.image_height,
                self.quality.jpeg_quality,
            )?;
            Ok(Some(RemoteDesktopVideoFrame {
                source_width: self.screen.width,
                source_height: self.screen.height,
                image_width: self.image_width,
                image_height: self.image_height,
                format: "jpeg".to_string(),
                bytes: encoded,
            }))
        }
    }

    struct CaptureResources {
        screen_dc: HDC,
        memory_dc: HDC,
        bitmap: HBITMAP,
        old_object: HGDIOBJ,
        info: BITMAPINFO,
        image_width: u32,
        image_height: u32,
    }

    impl CaptureResources {
        fn new(image_width: u32, image_height: u32) -> Result<Self, String> {
            unsafe {
                let screen_dc = GetDC(null_mut());
                if screen_dc.is_null() {
                    return Err(format!("GetDC failed: error={}", last_error_code()));
                }
                let memory_dc = CreateCompatibleDC(screen_dc);
                if memory_dc.is_null() {
                    ReleaseDC(null_mut(), screen_dc);
                    return Err(format!(
                        "CreateCompatibleDC failed: error={}",
                        last_error_code()
                    ));
                }
                let bitmap =
                    CreateCompatibleBitmap(screen_dc, image_width as i32, image_height as i32);
                if bitmap.is_null() {
                    DeleteDC(memory_dc);
                    ReleaseDC(null_mut(), screen_dc);
                    return Err(format!(
                        "CreateCompatibleBitmap failed: error={}",
                        last_error_code()
                    ));
                }
                let old_object = SelectObject(memory_dc, bitmap as HGDIOBJ);
                if old_object.is_null() {
                    DeleteObject(bitmap as HGDIOBJ);
                    DeleteDC(memory_dc);
                    ReleaseDC(null_mut(), screen_dc);
                    return Err(format!("SelectObject failed: error={}", last_error_code()));
                }
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: image_width as i32,
                        biHeight: -(image_height as i32),
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
                Ok(Self {
                    screen_dc,
                    memory_dc,
                    bitmap: bitmap as HBITMAP,
                    old_object,
                    info,
                    image_width,
                    image_height,
                })
            }
        }

        fn capture_bgra(
            &mut self,
            x: i32,
            y: i32,
            source_width: u32,
            source_height: u32,
            buffer: &mut Vec<u8>,
        ) -> Result<(), String> {
            let buffer_len = self
                .image_width
                .checked_mul(self.image_height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| "selected screen is too large".to_string())?
                as usize;
            let blit_result = blit_to_bitmap(
                self.memory_dc,
                self.screen_dc,
                x,
                y,
                source_width,
                source_height,
                self.image_width,
                self.image_height,
            );
            buffer.resize(buffer_len, 0);
            let dib_lines = if blit_result.is_ok() {
                unsafe {
                    GetDIBits(
                        self.memory_dc,
                        self.bitmap,
                        0,
                        self.image_height,
                        buffer.as_mut_ptr() as *mut c_void,
                        &mut self.info,
                        DIB_RGB_COLORS,
                    )
                }
            } else {
                0
            };
            let dib_error = last_error_code();

            blit_result?;
            if dib_lines == 0 {
                return Err(format!("GetDIBits failed: error={dib_error}"));
            }
            Ok(())
        }
    }

    impl Drop for CaptureResources {
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

    fn scaled_size(source_width: u32, source_height: u32, max_width: u32) -> (u32, u32) {
        let scale = (max_width as f32 / source_width as f32).min(1.0);
        if scale >= 1.0 {
            return (source_width, source_height);
        }
        let width = ((source_width as f32 * scale).round() as u32).max(1);
        let height = ((source_height as f32 * scale).round() as u32).max(1);
        (width, height)
    }

    fn blit_to_bitmap(
        memory_dc: HDC,
        screen_dc: HDC,
        x: i32,
        y: i32,
        source_width: u32,
        source_height: u32,
        image_width: u32,
        image_height: u32,
    ) -> Result<(), String> {
        let scaled = source_width != image_width || source_height != image_height;
        let operation = if scaled { "StretchBlt" } else { "BitBlt" };
        if scaled {
            unsafe {
                SetStretchBltMode(memory_dc, HALFTONE);
            }
        }
        let capture_result = blit_with_op(
            memory_dc,
            screen_dc,
            x,
            y,
            source_width,
            source_height,
            image_width,
            image_height,
            SRCCOPY | CAPTUREBLT,
        );
        if capture_result.is_ok() {
            return Ok(());
        }
        let capture_error = capture_result.err().unwrap_or_default();
        let srccopy_result = blit_with_op(
            memory_dc,
            screen_dc,
            x,
            y,
            source_width,
            source_height,
            image_width,
            image_height,
            SRCCOPY,
        );
        if srccopy_result.is_ok() {
            return Ok(());
        }
        let srccopy_error = srccopy_result.err().unwrap_or_default();
        Err(format!(
            "{operation} CAPTUREBLT failed: error={capture_error}; {operation} SRCCOPY failed: error={srccopy_error}"
        ))
    }

    fn blit_with_op(
        memory_dc: HDC,
        screen_dc: HDC,
        x: i32,
        y: i32,
        source_width: u32,
        source_height: u32,
        image_width: u32,
        image_height: u32,
        raster_op: u32,
    ) -> Result<(), u32> {
        let ok = unsafe {
            if source_width == image_width && source_height == image_height {
                BitBlt(
                    memory_dc,
                    0,
                    0,
                    image_width as i32,
                    image_height as i32,
                    screen_dc,
                    x,
                    y,
                    raster_op,
                )
            } else {
                StretchBlt(
                    memory_dc,
                    0,
                    0,
                    image_width as i32,
                    image_height as i32,
                    screen_dc,
                    x,
                    y,
                    source_width as i32,
                    source_height as i32,
                    raster_op,
                )
            }
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(last_error_code())
        }
    }

    fn write_rgb_from_bgra(bgra: &[u8], rgb: &mut Vec<u8>) -> Result<(), String> {
        if bgra.len() % 4 != 0 {
            return Err("captured frame buffer has invalid size".to_string());
        }
        rgb.resize(bgra.len() / 4 * 3, 0);
        for (source, target) in bgra.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) {
            target[0] = source[2];
            target[1] = source[1];
            target[2] = source[0];
        }
        Ok(())
    }

    fn encode_rgb_jpeg(
        rgb: &[u8],
        width: u32,
        height: u32,
        quality: u8,
    ) -> Result<Vec<u8>, String> {
        let encode = || -> Result<Vec<u8>, String> {
            let mut compressor = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
            compressor.set_fastest_defaults();
            compressor.set_size(width as usize, height as usize);
            compressor.set_quality(quality as f32);
            let mut compressor = compressor
                .start_compress(Vec::new())
                .map_err(|error| format!("jpeg encode start failed: {error}"))?;
            compressor
                .write_scanlines(rgb)
                .map_err(|error| format!("jpeg encode failed: {error}"))?;
            compressor
                .finish()
                .map_err(|error| format!("jpeg encode finish failed: {error}"))
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(encode))
            .map_err(|_| "jpeg encode panicked".to_string())?
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

    fn last_error_code() -> u32 {
        unsafe { GetLastError() }
    }
}

pub(crate) mod input {
    use std::{mem::size_of, thread, time::Duration};

    use super::super::KeyModifiers;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
        MOUSEINPUT, VIRTUAL_KEY, VK_0, VK_A, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END,
        VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT, VK_LEFT, VK_MENU, VK_NEXT, VK_OEM_1, VK_OEM_2,
        VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS,
        VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB,
        VK_UP,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    const CLICK_MOVE_SETTLE: Duration = Duration::from_millis(8);
    const CLICK_HOLD: Duration = Duration::from_millis(24);

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
        if let Err(error) = send_mouse_inputs(&[move_input(x, y)]) {
            return format!("remote_desktop_error\nmessage={error}");
        }
        thread::sleep(CLICK_MOVE_SETTLE);
        if let Err(error) = send_mouse_inputs(&[button_input(down)]) {
            return format!("remote_desktop_error\nmessage={error}");
        }
        thread::sleep(CLICK_HOLD);
        if let Err(error) = send_mouse_inputs(&[button_input(up)]) {
            return format!("remote_desktop_error\nmessage={error}");
        }
        format!("remote_desktop_input\nmessage=click {button} {x} {y}")
    }

    pub(crate) fn mouse_button(x: i32, y: i32, button: &str, down: bool) -> String {
        let flag = match (button, down) {
            ("right", true) => MOUSEEVENTF_RIGHTDOWN,
            ("right", false) => MOUSEEVENTF_RIGHTUP,
            (_, true) => MOUSEEVENTF_LEFTDOWN,
            (_, false) => MOUSEEVENTF_LEFTUP,
        };
        if let Err(error) = send_mouse_inputs(&[move_input(x, y), button_input(flag)]) {
            return format!("remote_desktop_error\nmessage={error}");
        }
        let state = if down { "down" } else { "up" };
        format!("remote_desktop_input\nmessage=mouse {button} {state} {x} {y}")
    }

    pub(crate) fn key(name: &str, modifiers: KeyModifiers) -> String {
        let Some(vk) = key_vk(name) else {
            return format!("remote_desktop_error\nmessage=unsupported key {name}");
        };
        let mut inputs = Vec::new();
        let modifiers = modifier_vks(modifiers);
        for modifier in &modifiers {
            inputs.push(key_input(*modifier, 0));
        }
        inputs.push(key_input(vk, 0));
        inputs.push(key_input(vk, KEYEVENTF_KEYUP));
        for modifier in modifiers.iter().rev() {
            inputs.push(key_input(*modifier, KEYEVENTF_KEYUP));
        }
        if let Err(error) = send_inputs(&inputs) {
            let _ = send_inputs(&key_release_inputs(vk, &modifiers));
            return format!("remote_desktop_error\nmessage={error}");
        }
        format!("remote_desktop_input\nmessage=key {name}")
    }

    pub(crate) fn text(text: &str) -> String {
        let mut inputs = Vec::new();
        for unit in text.encode_utf16() {
            inputs.push(unicode_input(unit, 0));
            inputs.push(unicode_input(unit, KEYEVENTF_KEYUP));
        }
        if let Err(error) = send_inputs(&inputs) {
            return format!("remote_desktop_error\nmessage={error}");
        }
        "remote_desktop_input\nmessage=text sent".to_string()
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

    fn key_input(vk: VIRTUAL_KEY, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn key_release_inputs(vk: VIRTUAL_KEY, modifiers: &[VIRTUAL_KEY]) -> Vec<INPUT> {
        let mut inputs = vec![key_input(vk, KEYEVENTF_KEYUP)];
        for modifier in modifiers.iter().rev() {
            inputs.push(key_input(*modifier, KEYEVENTF_KEYUP));
        }
        inputs
    }

    fn unicode_input(unit: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE | flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
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

    fn key_vk(name: &str) -> Option<VIRTUAL_KEY> {
        match name {
            "arrow_down" => Some(VK_DOWN),
            "arrow_left" => Some(VK_LEFT),
            "arrow_right" => Some(VK_RIGHT),
            "arrow_up" => Some(VK_UP),
            "escape" => Some(VK_ESCAPE),
            "tab" => Some(VK_TAB),
            "backspace" => Some(VK_BACK),
            "enter" => Some(VK_RETURN),
            "space" => Some(VK_SPACE),
            "insert" => Some(VK_INSERT),
            "delete" => Some(VK_DELETE),
            "home" => Some(VK_HOME),
            "end" => Some(VK_END),
            "page_up" => Some(VK_PRIOR),
            "page_down" => Some(VK_NEXT),
            "colon" | "semicolon" => Some(VK_OEM_1),
            "comma" => Some(VK_OEM_COMMA),
            "backslash" | "pipe" => Some(VK_OEM_5),
            "slash" | "questionmark" => Some(VK_OEM_2),
            "open_bracket" | "open_curly_bracket" => Some(VK_OEM_4),
            "close_bracket" | "close_curly_bracket" => Some(VK_OEM_6),
            "backtick" => Some(VK_OEM_3),
            "minus" => Some(VK_OEM_MINUS),
            "period" => Some(VK_OEM_PERIOD),
            "plus" | "equals" => Some(VK_OEM_PLUS),
            "quote" => Some(VK_OEM_7),
            "exclamationmark" => Some(VK_0 + 1),
            "browser_back" => {
                Some(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_BROWSER_BACK)
            }
            key if key.len() == 1 => {
                let byte = key.as_bytes()[0];
                if byte.is_ascii_digit() {
                    Some(VK_0 + (byte - b'0') as u16)
                } else if byte.is_ascii_lowercase() {
                    Some(VK_A + (byte - b'a') as u16)
                } else {
                    None
                }
            }
            key if key.starts_with('f') => key
                .strip_prefix('f')
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|value| (1..=24).contains(value))
                .map(|value| VK_F1 + value - 1),
            _ => None,
        }
    }

    fn modifier_vks(modifiers: KeyModifiers) -> Vec<VIRTUAL_KEY> {
        let mut keys = Vec::new();
        if modifiers.shift {
            keys.push(VK_SHIFT);
        }
        if modifiers.ctrl || modifiers.command {
            keys.push(VK_CONTROL);
        }
        if modifiers.alt {
            keys.push(VK_MENU);
        }
        keys
    }

    fn send_mouse_inputs(inputs: &[INPUT]) -> Result<(), String> {
        send_inputs(inputs)
    }

    fn send_inputs(inputs: &[INPUT]) -> Result<(), String> {
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

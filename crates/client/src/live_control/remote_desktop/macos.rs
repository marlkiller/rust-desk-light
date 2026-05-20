pub(crate) mod capture {
    use super::super::RemoteDesktopVideoFrame;
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

    pub(crate) fn screens() -> String {
        match enum_screens() {
            Ok(screens) => format_screens(&screens),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) struct CaptureStream {
        screen: Screen,
        quality: QualityProfile,
        display: CGDisplay,
        rgba: Vec<u8>,
    }

    impl CaptureStream {
        pub(crate) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
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

        pub(crate) fn capture_frame(&mut self) -> Result<RemoteDesktopVideoFrame, String> {
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

pub(crate) mod input {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::super::KeyModifiers;

    static LEFT_BUTTON_DOWN: AtomicBool = AtomicBool::new(false);
    static RIGHT_BUTTON_DOWN: AtomicBool = AtomicBool::new(false);

    pub(crate) fn move_mouse(x: i32, y: i32) -> String {
        match post_move(x, y) {
            Ok(()) => format!("remote_desktop_input\nmessage=mouse moved {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn click(x: i32, y: i32, button: &str) -> String {
        match post_click(x, y, button) {
            Ok(()) => format!("remote_desktop_input\nmessage=click {button} {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn mouse_button(x: i32, y: i32, button: &str, down: bool) -> String {
        match post_mouse_button(x, y, button, down) {
            Ok(()) => {
                let state = if down { "down" } else { "up" };
                format!("remote_desktop_input\nmessage=mouse {button} {state} {x} {y}")
            }
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn key(name: &str, modifiers: KeyModifiers) -> String {
        match post_key(name, modifiers) {
            Ok(()) => format!("remote_desktop_input\nmessage=key {name}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn text(text: &str) -> String {
        match post_text(text) {
            Ok(()) => "remote_desktop_input\nmessage=text sent".to_string(),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    fn post_move(x: i32, y: i32) -> Result<(), String> {
        ensure_accessibility_permission()?;
        let source = event_source()?;
        let (event_type, button) = if LEFT_BUTTON_DOWN.load(Ordering::Relaxed) {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        } else if RIGHT_BUTTON_DOWN.load(Ordering::Relaxed) {
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        } else {
            (CGEventType::MouseMoved, CGMouseButton::Left)
        };
        post_mouse_event(&source, event_type, button, x, y)
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

    fn post_mouse_button(x: i32, y: i32, button: &str, down: bool) -> Result<(), String> {
        ensure_accessibility_permission()?;
        let source = event_source()?;
        let (event_type, mouse_button, state) = match (button, down) {
            ("right", true) => (
                CGEventType::RightMouseDown,
                CGMouseButton::Right,
                &RIGHT_BUTTON_DOWN,
            ),
            ("right", false) => (
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
                &RIGHT_BUTTON_DOWN,
            ),
            (_, true) => (
                CGEventType::LeftMouseDown,
                CGMouseButton::Left,
                &LEFT_BUTTON_DOWN,
            ),
            (_, false) => (
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
                &LEFT_BUTTON_DOWN,
            ),
        };
        post_mouse_event(&source, CGEventType::MouseMoved, mouse_button, x, y)?;
        post_mouse_event(&source, event_type, mouse_button, x, y)?;
        state.store(down, Ordering::Relaxed);
        Ok(())
    }

    fn post_key(name: &str, modifiers: KeyModifiers) -> Result<(), String> {
        ensure_accessibility_permission()?;
        let source = event_source()?;
        let key_code = key_code(name).ok_or_else(|| format!("unsupported key {name}"))?;
        let modifiers = modifier_key_codes(modifiers);
        for modifier in &modifiers {
            post_keyboard_event(&source, *modifier, true)?;
        }
        post_keyboard_event(&source, key_code, true)?;
        post_keyboard_event(&source, key_code, false)?;
        for modifier in modifiers.iter().rev() {
            post_keyboard_event(&source, *modifier, false)?;
        }
        Ok(())
    }

    fn post_text(text: &str) -> Result<(), String> {
        ensure_accessibility_permission()?;
        let source = event_source()?;
        for ch in text.chars() {
            let (key_code, shift) = text_key_code(ch)
                .ok_or_else(|| format!("unsupported macOS text character U+{:04X}", ch as u32))?;
            if shift {
                post_keyboard_event(&source, 56, true)?;
            }
            post_keyboard_event(&source, key_code, true)?;
            post_keyboard_event(&source, key_code, false)?;
            if shift {
                post_keyboard_event(&source, 56, false)?;
            }
        }
        Ok(())
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

    fn post_keyboard_event(
        source: &CGEventSource,
        key_code: u16,
        pressed: bool,
    ) -> Result<(), String> {
        let event = CGEvent::new_keyboard_event(source.clone(), key_code, pressed)
            .map_err(|_| "CGEventCreateKeyboardEvent failed".to_string())?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn modifier_key_codes(modifiers: KeyModifiers) -> Vec<u16> {
        let mut keys = Vec::new();
        if modifiers.shift {
            keys.push(56);
        }
        if modifiers.ctrl {
            keys.push(59);
        }
        if modifiers.alt {
            keys.push(58);
        }
        if modifiers.command {
            keys.push(55);
        }
        keys
    }

    fn text_key_code(ch: char) -> Option<(u16, bool)> {
        if ch.is_ascii_alphabetic() {
            return key_code(&ch.to_ascii_lowercase().to_string())
                .map(|code| (code, ch.is_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return key_code(&ch.to_string()).map(|code| (code, false));
        }
        Some(match ch {
            ' ' => (49, false),
            '\t' => (48, false),
            '\n' | '\r' => (36, false),
            '!' => (18, true),
            '@' => (19, true),
            '#' => (20, true),
            '$' => (21, true),
            '%' => (23, true),
            '^' => (22, true),
            '&' => (26, true),
            '*' => (28, true),
            '(' => (25, true),
            ')' => (29, true),
            '-' => (27, false),
            '_' => (27, true),
            '=' => (24, false),
            '+' => (24, true),
            '[' => (33, false),
            '{' => (33, true),
            ']' => (30, false),
            '}' => (30, true),
            '\\' => (42, false),
            '|' => (42, true),
            ';' => (41, false),
            ':' => (41, true),
            '\'' => (39, false),
            '"' => (39, true),
            ',' => (43, false),
            '<' => (43, true),
            '.' => (47, false),
            '>' => (47, true),
            '/' => (44, false),
            '?' => (44, true),
            '`' => (50, false),
            '~' => (50, true),
            _ => return None,
        })
    }

    fn key_code(name: &str) -> Option<u16> {
        Some(match name {
            "a" => 0,
            "s" => 1,
            "d" => 2,
            "f" => 3,
            "h" => 4,
            "g" => 5,
            "z" => 6,
            "x" => 7,
            "c" => 8,
            "v" => 9,
            "b" => 11,
            "q" => 12,
            "w" => 13,
            "e" => 14,
            "r" => 15,
            "y" => 16,
            "t" => 17,
            "1" => 18,
            "2" => 19,
            "3" => 20,
            "4" => 21,
            "6" => 22,
            "5" => 23,
            "equals" | "plus" => 24,
            "9" => 25,
            "7" => 26,
            "minus" => 27,
            "8" => 28,
            "0" => 29,
            "close_bracket" | "close_curly_bracket" => 30,
            "o" => 31,
            "u" => 32,
            "open_bracket" | "open_curly_bracket" => 33,
            "i" => 34,
            "p" => 35,
            "enter" => 36,
            "l" => 37,
            "j" => 38,
            "quote" => 39,
            "k" => 40,
            "semicolon" | "colon" => 41,
            "backslash" | "pipe" => 42,
            "comma" => 43,
            "slash" | "questionmark" => 44,
            "n" => 45,
            "m" => 46,
            "period" => 47,
            "tab" => 48,
            "space" => 49,
            "backtick" => 50,
            "backspace" => 51,
            "escape" => 53,
            "f1" => 122,
            "f2" => 120,
            "f3" => 99,
            "f4" => 118,
            "f5" => 96,
            "f6" => 97,
            "f7" => 98,
            "f8" => 100,
            "f9" => 101,
            "f10" => 109,
            "f11" => 103,
            "f12" => 111,
            "f13" => 105,
            "f14" => 107,
            "f15" => 113,
            "f16" => 106,
            "f17" => 64,
            "f18" => 79,
            "f19" => 80,
            "f20" => 90,
            "home" => 115,
            "page_up" => 116,
            "delete" => 117,
            "end" => 119,
            "page_down" => 121,
            "arrow_left" => 123,
            "arrow_right" => 124,
            "arrow_down" => 125,
            "arrow_up" => 126,
            _ => return None,
        })
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

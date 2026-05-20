pub(crate) mod capture {
    use super::super::RemoteDesktopVideoFrame;
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

    pub(crate) fn screens() -> String {
        match enum_screens() {
            Ok(screens) => format_screens(&screens),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) struct CaptureStream {
        screen: Screen,
        quality: QualityProfile,
        geometry: String,
        backends: Vec<CaptureBackend>,
        active_backend: usize,
    }

    impl CaptureStream {
        pub(crate) fn new(screen_index: usize, quality: &str) -> Result<Self, String> {
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

        pub(crate) fn capture_frame(&mut self) -> Result<RemoteDesktopVideoFrame, String> {
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

pub(crate) mod input {
    use std::process::Command;

    use super::super::KeyModifiers;

    pub(crate) fn move_mouse(x: i32, y: i32) -> String {
        let x = x.to_string();
        let y = y.to_string();
        match run_xdotool(&["mousemove", &x, &y]) {
            Ok(()) => format!("remote_desktop_input\nmessage=mouse moved {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn click(x: i32, y: i32, button: &str) -> String {
        let button_id = if button == "right" { "3" } else { "1" };
        let x = x.to_string();
        let y = y.to_string();
        match run_xdotool(&["mousemove", &x, &y, "click", button_id]) {
            Ok(()) => format!("remote_desktop_input\nmessage=click {button} {x} {y}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn mouse_button(x: i32, y: i32, button: &str, down: bool) -> String {
        let button_id = if button == "right" { "3" } else { "1" };
        let action = if down { "mousedown" } else { "mouseup" };
        let x = x.to_string();
        let y = y.to_string();
        match run_xdotool(&["mousemove", &x, &y, action, button_id]) {
            Ok(()) => {
                let state = if down { "down" } else { "up" };
                format!("remote_desktop_input\nmessage=mouse {button} {state} {x} {y}")
            }
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn key(name: &str, modifiers: KeyModifiers) -> String {
        let Some(key_name) = xdotool_key(name) else {
            return format!("remote_desktop_error\nmessage=unsupported key {name}");
        };
        let modifiers = modifier_key_names(modifiers);
        let mut args = Vec::new();
        for modifier in &modifiers {
            args.push("keydown".to_string());
            args.push((*modifier).to_string());
        }
        args.push("key".to_string());
        args.push(key_name);
        for modifier in modifiers.iter().rev() {
            args.push("keyup".to_string());
            args.push((*modifier).to_string());
        }
        match run_xdotool_owned(&args) {
            Ok(()) => format!("remote_desktop_input\nmessage=key {name}"),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    pub(crate) fn text(text: &str) -> String {
        match run_xdotool(&["type", "--clearmodifiers", "--delay", "0", text]) {
            Ok(()) => "remote_desktop_input\nmessage=text sent".to_string(),
            Err(error) => format!("remote_desktop_error\nmessage={error}"),
        }
    }

    fn xdotool_key(name: &str) -> Option<String> {
        let value = match name {
            "arrow_down" => "Down",
            "arrow_left" => "Left",
            "arrow_right" => "Right",
            "arrow_up" => "Up",
            "escape" => "Escape",
            "tab" => "Tab",
            "backspace" => "BackSpace",
            "enter" => "Return",
            "space" => "space",
            "insert" => "Insert",
            "delete" => "Delete",
            "home" => "Home",
            "end" => "End",
            "page_up" => "Page_Up",
            "page_down" => "Page_Down",
            "colon" => "colon",
            "comma" => "comma",
            "backslash" => "backslash",
            "slash" => "slash",
            "pipe" => "bar",
            "questionmark" => "question",
            "exclamationmark" => "exclam",
            "open_bracket" => "bracketleft",
            "close_bracket" => "bracketright",
            "open_curly_bracket" => "braceleft",
            "close_curly_bracket" => "braceright",
            "backtick" => "grave",
            "minus" => "minus",
            "period" => "period",
            "plus" => "plus",
            "equals" => "equal",
            "semicolon" => "semicolon",
            "quote" => "apostrophe",
            "browser_back" => "XF86Back",
            key if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
                return Some(key.to_string());
            }
            key if key.starts_with('f') => {
                let number = key.strip_prefix('f')?.parse::<u8>().ok()?;
                if (1..=35).contains(&number) {
                    return Some(format!("F{number}"));
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        Some(value.to_string())
    }

    fn modifier_key_names(modifiers: KeyModifiers) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if modifiers.shift {
            keys.push("Shift_L");
        }
        if modifiers.ctrl || modifiers.command {
            keys.push("Control_L");
        }
        if modifiers.alt {
            keys.push("Alt_L");
        }
        keys
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

    fn run_xdotool_owned(args: &[String]) -> Result<(), String> {
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        run_xdotool(&refs)
    }
}

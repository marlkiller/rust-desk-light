use crate::support::{
    current_dir_label, hostname, join_sections, run_command, run_command_with_stdin,
    run_first_available, run_first_available_with_stdin, run_powershell, run_powershell_with_stdin,
    truncate_chars, username,
};
use rdl_protocol::CommandKind;

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::ComputerInfo => computer_info(),
        CommandKind::Clipboard => clipboard_command(payload),
        CommandKind::Proxy => format!("TODO: {} accepted as planned stub", command.as_str()),
        _ => unreachable!("system_info received non-system command"),
    }
}

fn computer_info() -> String {
    let mut sections = vec![
        format!("hostname={}", hostname()),
        format!("user={}", username()),
        format!("os={}", os_label()),
        format!("kernel={}", kernel_label()),
        format!("arch={}", std::env::consts::ARCH),
        format!("current_dir={}", current_dir_label()),
        format!("process_id={}", std::process::id()),
        format!("gui_session={}", gui_session_label()),
    ];
    sections.extend(platform_computer_info());
    join_sections("computer_info", sections)
}

fn os_label() -> String {
    if cfg!(target_os = "linux") {
        if let Some(value) = os_release_value("PRETTY_NAME") {
            return value;
        }
    }
    if cfg!(target_os = "windows") {
        let output = run_command("cmd", &["/C", "ver"], 10);
        let trimmed = output.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if cfg!(target_os = "macos") {
        let output = run_command("sw_vers", &["-productVersion"], 10);
        let trimmed = output.trim();
        if !trimmed.is_empty() {
            return format!("macOS {trimmed}");
        }
    }
    std::env::consts::OS.to_string()
}

fn kernel_label() -> String {
    if cfg!(target_os = "windows") {
        return run_command("cmd", &["/C", "ver"], 10).trim().to_string();
    }
    let output = run_command("uname", &["-r"], 10);
    let trimmed = output.trim();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn gui_session_label() -> String {
    std::env::var("XDG_SESSION_TYPE")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .or_else(|_| std::env::var("WAYLAND_DISPLAY").map(|_| "wayland".to_string()))
        .or_else(|_| std::env::var("DISPLAY").map(|_| "x11".to_string()))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn platform_computer_info() -> Vec<String> {
    if cfg!(target_os = "windows") {
        return vec![
            format!("windows_system={}", trim_command(run_command(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    "(Get-CimInstance Win32_ComputerSystem | Select-Object Manufacturer,Model,TotalPhysicalMemory | ConvertTo-Json -Compress)",
                ],
                20,
            ))),
            format!("windows_os={}", trim_command(run_command(
                "powershell",
                &[
                    "-NoProfile",
                    "-Command",
                    "(Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber,LastBootUpTime | ConvertTo-Json -Compress)",
                ],
                20,
            ))),
        ];
    }
    if cfg!(target_os = "linux") {
        return vec![
            format!("distro_id={}", os_release_value("ID").unwrap_or_default()),
            format!(
                "distro_version={}",
                os_release_value("VERSION_ID").unwrap_or_default()
            ),
            format!(
                "desktop={}",
                std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
            ),
            format!("display={}", std::env::var("DISPLAY").unwrap_or_default()),
            format!(
                "wayland_display={}",
                std::env::var("WAYLAND_DISPLAY").unwrap_or_default()
            ),
            format!("cpu={}", first_cpu_model()),
            format!("memory={}", memory_summary()),
            format!(
                "uptime={}",
                trim_command(run_command("uptime", &["-p"], 10))
            ),
            format!(
                "ip_addresses={}",
                trim_command(run_command("hostname", &["-I"], 10))
            ),
        ];
    }
    if cfg!(target_os = "macos") {
        return vec![
            format!(
                "product_name={}",
                trim_command(run_command("sw_vers", &["-productName"], 10))
            ),
            format!(
                "build_version={}",
                trim_command(run_command("sw_vers", &["-buildVersion"], 10))
            ),
            format!(
                "hardware={}",
                trim_command(run_command("sysctl", &["-n", "hw.model"], 10))
            ),
            format!(
                "cpu={}",
                trim_command(run_command(
                    "sysctl",
                    &["-n", "machdep.cpu.brand_string"],
                    10
                ))
            ),
            format!(
                "memory_bytes={}",
                trim_command(run_command("sysctl", &["-n", "hw.memsize"], 10))
            ),
        ];
    }
    Vec::new()
}

fn os_release_value(key: &str) -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let prefix = format!("{key}=");
    text.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim_matches('"').to_string())
}

fn first_cpu_model() -> String {
    let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") else {
        return String::new();
    };
    text.lines()
        .find_map(|line| line.strip_prefix("model name\t: "))
        .unwrap_or_default()
        .to_string()
}

fn memory_summary() -> String {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return String::new();
    };
    let total = text
        .lines()
        .find(|line| line.starts_with("MemTotal:"))
        .unwrap_or_default();
    let available = text
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))
        .unwrap_or_default();
    format!("{total}; {available}")
}

fn trim_command(output: String) -> String {
    output.trim().replace(['\r', '\n'], " ")
}

fn clipboard_command(payload: &str) -> String {
    let trimmed = payload.trim();
    if let Some(value) = trimmed
        .strip_prefix("write:")
        .or_else(|| trimmed.strip_prefix("set:"))
    {
        return write_clipboard(value.trim_start());
    }
    if trimmed.eq_ignore_ascii_case("write") || trimmed.eq_ignore_ascii_case("set") {
        return "clipboard write requires payload: write:<text>".to_string();
    }
    read_clipboard()
}

fn read_clipboard() -> String {
    let result = if cfg!(target_os = "windows") {
        run_powershell("Get-Clipboard", 40)
    } else if cfg!(target_os = "macos") {
        run_command("pbpaste", &[], 40)
    } else {
        run_first_available(
            &[
                ("wl-paste", &[][..]),
                ("xclip", &["-selection", "clipboard", "-o"][..]),
                ("xsel", &["--clipboard", "--output"][..]),
            ],
            40,
        )
    };
    format!("clipboard read:\n{}", truncate_chars(&result, 4_000))
}

fn write_clipboard(value: &str) -> String {
    if cfg!(target_os = "windows") {
        return run_powershell_with_stdin("$input | Set-Clipboard", value, 20);
    }
    if cfg!(target_os = "macos") {
        return run_command_with_stdin("pbcopy", &[], value, 20);
    }
    run_first_available_with_stdin(
        &[
            ("wl-copy", &[][..]),
            ("xclip", &["-selection", "clipboard"][..]),
            ("xsel", &["--clipboard", "--input"][..]),
        ],
        value,
        20,
    )
}

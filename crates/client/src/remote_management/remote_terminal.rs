use crate::support::truncate_chars;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

static CURRENT_DIR: OnceLock<Mutex<PathBuf>> = OnceLock::new();

pub(crate) fn execute(payload: &str) -> String {
    let command = payload.trim();
    if command.is_empty() {
        return terminal_response(
            current_dir_label(),
            "remote_terminal requires a command payload",
        );
    }

    if let Some(target_dir) = parse_cd_target(command) {
        return change_dir(target_dir);
    }

    let cwd = current_dir();
    let output = if cfg!(target_os = "windows") {
        run_powershell_in_dir(command, &cwd, 2_000)
    } else {
        run_command_in_dir("sh", &["-lc", command], &cwd, 2_000)
    };
    terminal_response(cwd.display().to_string(), &output)
}

fn parse_cd_target(command: &str) -> Option<&str> {
    let trimmed = command.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == "cd" || lower == "chdir" {
        return Some("");
    }
    for prefix in ["cd ", "chdir "] {
        if lower.starts_with(prefix) {
            let mut target = trimmed[prefix.len()..].trim();
            if cfg!(target_os = "windows") && target.to_ascii_lowercase().starts_with("/d ") {
                target = target[3..].trim();
            }
            return Some(target);
        }
    }
    None
}

fn change_dir(target: &str) -> String {
    let current = current_dir();
    if target.trim().is_empty() {
        return terminal_response(current.display().to_string(), "");
    }

    let target = unquote(target.trim());
    let next = expand_dir(&current, target);
    if !next.is_dir() {
        return terminal_response(
            current.display().to_string(),
            &format!("cd failed: directory not found: {}", next.display()),
        );
    }

    let next = next.canonicalize().unwrap_or(next);
    if let Ok(mut value) = current_dir_lock().lock() {
        *value = next.clone();
    }
    terminal_response(next.display().to_string(), "")
}

fn expand_dir(current: &PathBuf, target: &str) -> PathBuf {
    if target == "~" {
        return home_dir().unwrap_or_else(|| current.clone());
    }
    if let Some(rest) = target.strip_prefix("~/") {
        return home_dir().unwrap_or_else(|| current.clone()).join(rest);
    }
    let path = PathBuf::from(target);
    if path.is_absolute() {
        path
    } else {
        current.join(path)
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn current_dir() -> PathBuf {
    current_dir_lock()
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn current_dir_label() -> String {
    current_dir().display().to_string()
}

fn current_dir_lock() -> &'static Mutex<PathBuf> {
    CURRENT_DIR
        .get_or_init(|| Mutex::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn run_powershell_in_dir(script: &str, current_dir: &PathBuf, max_lines: usize) -> String {
    run_command_in_dir(
        "powershell",
        &[
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!(
                "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[System.Text.Encoding]::UTF8; {script}"
            ),
        ],
        current_dir,
        max_lines,
    )
}

fn run_command_in_dir(
    program: &str,
    args: &[&str],
    current_dir: &PathBuf,
    max_lines: usize,
) -> String {
    match Command::new(program)
        .args(args)
        .current_dir(current_dir)
        .output()
    {
        Ok(output) => command_output_text(
            program,
            output.status.success(),
            output.stdout,
            output.stderr,
            max_lines,
        ),
        Err(error) => format!("{program} failed: {error}"),
    }
}

fn command_output_text(
    program: &str,
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    max_lines: usize,
) -> String {
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    let mut text = String::new();
    if !success {
        text.push_str(program);
        text.push_str(" exited with error\n");
    }
    if !stdout.trim().is_empty() {
        text.push_str(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr.trim());
    }
    if text.is_empty() {
        text.push_str("ok");
    }
    truncate_lines(&text, max_lines)
}

fn truncate_lines(value: &str, max_lines: usize) -> String {
    let mut lines = value.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if value.lines().count() > max_lines {
        lines.push_str("\n...");
    }
    truncate_chars(&lines, 256_000)
}

fn terminal_response(current_dir: String, output: &str) -> String {
    format!("__rdl_terminal_cwd\t{current_dir}\n{output}")
}

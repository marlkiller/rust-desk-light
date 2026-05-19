use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);
const EXECUTE_MAX_LINES: usize = 2_000;

pub(super) fn run_shell(script: &str) -> String {
    if cfg!(target_os = "windows") {
        run_process(
            "powershell",
            &[
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                format!(
                    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[System.Text.Encoding]::UTF8; {script}"
                ),
            ],
            None,
        )
    } else {
        run_process("sh", &["-lc".to_string(), script.to_string()], None)
    }
}

pub(super) fn run_process(program: &str, args: &[String], working_dir: Option<&str>) -> String {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(working_dir) = working_dir {
        command.current_dir(working_dir);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return format!("status=failed\nmessage={}", clean_value(&error.to_string())),
    };
    let stdout_reader = child.stdout.take().map(|mut stdout| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr_reader = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        })
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < EXECUTE_TIMEOUT => {
                thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return format!(
                    "status=failed\nmessage=timed out after {}s",
                    EXECUTE_TIMEOUT.as_secs()
                );
            }
            Err(error) => {
                return format!(
                    "status=failed\nmessage=wait failed: {}",
                    clean_value(&error.to_string())
                );
            }
        }
    };
    let stdout = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    command_output_text(status.success(), stdout, stderr)
}

fn command_output_text(success: bool, stdout: Vec<u8>, stderr: Vec<u8>) -> String {
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    let mut lines = vec![format!(
        "status={}",
        if success { "success" } else { "failed" }
    )];
    if !stdout.trim().is_empty() {
        lines.push("stdout:".to_string());
        lines.extend(stdout.trim().lines().map(str::to_string));
    }
    if !stderr.trim().is_empty() {
        lines.push("stderr:".to_string());
        lines.extend(stderr.trim().lines().map(str::to_string));
    }
    if lines.len() == 1 {
        lines.push("stdout:".to_string());
        lines.push("ok".to_string());
    }
    truncate_lines(&lines.join("\n"), EXECUTE_MAX_LINES)
}

pub(super) fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg(version_arg(program))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success() || status.code().is_some())
        .unwrap_or(false)
}

fn version_arg(program: &str) -> &'static str {
    match program {
        "powershell" => "-Version",
        _ => "--version",
    }
}

pub(super) fn split_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(quote_char) = quote {
            if ch == quote_char {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            ch if ch.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

pub(super) fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .map(str::to_string)
}

pub(super) fn clean_value(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

pub(super) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn truncate_lines(value: &str, max_lines: usize) -> String {
    let mut lines = value.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if value.lines().count() > max_lines {
        lines.push_str("\n...");
    }
    if lines.chars().count() > 256_000 {
        lines = lines.chars().take(256_000).collect::<String>();
        lines.push_str("\n...");
    }
    lines
}

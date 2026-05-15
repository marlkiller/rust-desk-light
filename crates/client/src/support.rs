use std::io::{Read, Write};
use std::os::raw::{c_char, c_int};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

static HOSTNAME_CACHE: OnceLock<String> = OnceLock::new();

pub fn run_command(program: &str, args: &[&str], max_lines: usize) -> String {
    run_command_timeout(program, args, max_lines, Duration::from_secs(12))
}

pub fn run_command_timeout(
    program: &str,
    args: &[&str],
    max_lines: usize,
    timeout: Duration,
) -> String {
    let mut child = match Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return format!("{program} failed: {error}"),
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

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return format!("{program} timed out after {}s", timeout.as_secs());
            }
            Err(error) => return format!("{program} wait failed: {error}"),
        }
    };

    let stdout = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();

    command_output_text(program, status.success(), stdout, stderr, max_lines)
}

pub fn run_powershell(script: &str, max_lines: usize) -> String {
    run_command(
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
        max_lines,
    )
}

pub fn run_command_with_stdin(
    program: &str,
    args: &[&str],
    input: &str,
    max_lines: usize,
) -> String {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return format!("{program} failed: {error}"),
    };

    if let Some(stdin) = child.stdin.as_mut() {
        if let Err(error) = stdin.write_all(input.as_bytes()) {
            return format!("{program} stdin failed: {error}");
        }
    }

    match child.wait_with_output() {
        Ok(output) => {
            if output.status.success() {
                format!("clipboard write accepted, bytes={}", input.len())
            } else {
                command_output_text(program, false, output.stdout, output.stderr, max_lines)
            }
        }
        Err(error) => format!("{program} failed: {error}"),
    }
}

pub fn run_powershell_with_stdin(script: &str, input: &str, max_lines: usize) -> String {
    run_command_with_stdin(
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
        input,
        max_lines,
    )
}

pub fn run_first_available(commands: &[(&str, &[&str])], max_lines: usize) -> String {
    for (program, args) in commands {
        let output = run_command(program, args, max_lines);
        if !output.starts_with(&format!("{program} failed:")) {
            return output;
        }
    }
    "no supported command available on this host".to_string()
}

pub fn run_first_available_with_stdin(
    commands: &[(&str, &[&str])],
    input: &str,
    max_lines: usize,
) -> String {
    for (program, args) in commands {
        let output = run_command_with_stdin(program, args, input, max_lines);
        if !output.starts_with(&format!("{program} failed:")) {
            return output;
        }
    }
    "no supported clipboard writer available on this host".to_string()
}

pub fn join_sections(title: &str, sections: Vec<String>) -> String {
    let body = sections
        .into_iter()
        .filter(|section| !section.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{title}:\n{body}")
}

pub fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n...");
    truncated
}

pub fn hostname() -> String {
    HOSTNAME_CACHE.get_or_init(resolve_hostname).clone()
}

fn resolve_hostname() -> String {
    std::env::var("HOSTNAME")
        .map_err(|error| error.to_string())
        .or_else(|_| std::env::var("COMPUTERNAME").map_err(|error| error.to_string()))
        .or_else(|_| platform_hostname())
        .or_else(|_| command_first_line("scutil", &["--get", "ComputerName"]))
        .or_else(|_| command_first_line("scutil", &["--get", "LocalHostName"]))
        .or_else(|_| command_first_line("hostname", &[]))
        .or_else(|_| {
            std::fs::read_to_string("/etc/hostname")
                .map(|value| value.trim().to_string())
                .map_err(|error| error.to_string())
        })
        .map(|value| value.trim().to_string())
        .map_err(|error| error.to_string())
        .and_then(|value| {
            if value.is_empty() {
                Err("empty hostname".to_string())
            } else {
                Ok(value)
            }
        })
        .unwrap_or_else(|_| "unknown-host".to_string())
}

#[cfg(target_family = "unix")]
fn platform_hostname() -> Result<String, String> {
    let mut buffer = [0_u8; 256];
    let result = unsafe { gethostname(buffer.as_mut_ptr().cast::<c_char>(), buffer.len()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let len = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8(buffer[..len].to_vec()).map_err(|error| error.to_string())
}

#[cfg(not(target_family = "unix"))]
fn platform_hostname() -> Result<String, String> {
    Err("platform hostname unavailable".to_string())
}

#[cfg(target_family = "unix")]
extern "C" {
    fn gethostname(name: *mut c_char, len: usize) -> c_int;
}

pub fn username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string())
}

pub fn current_dir_label() -> String {
    std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
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

fn command_first_line(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("{program} exited with error"));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| error.to_string())?
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "empty output".to_string())
}

fn truncate_lines(value: &str, max_lines: usize) -> String {
    let mut lines = value.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if value.lines().count() > max_lines {
        lines.push_str("\n...");
    }
    truncate_chars(&lines, 256_000)
}

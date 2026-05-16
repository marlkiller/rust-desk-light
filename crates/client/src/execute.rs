use base64::{engine::general_purpose::STANDARD, Engine};
use rdl_protocol::CommandKind;
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);
const EXECUTE_MAX_LINES: usize = 2_000;

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::ExecuteFile => execute_file(payload),
        CommandKind::ExecuteCode => execute_code(payload),
        CommandKind::ExecuteStaticCommand => execute_static_command(payload),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

fn execute_file(payload: &str) -> String {
    let path = payload_field(payload, "path")
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            let trimmed = payload.trim();
            (!trimmed.is_empty() && !trimmed.lines().all(|line| line.contains('=')))
                .then(|| trimmed.to_string())
        });
    let Some(path) = path else {
        return "execute_file\nstatus=failed\nmessage=path is required".to_string();
    };
    let args = payload_field(payload, "args")
        .map(|value| split_args(&value))
        .unwrap_or_default();
    let working_dir =
        payload_field(payload, "working_dir").filter(|value| !value.trim().is_empty());
    let output = run_process(&path, &args, working_dir.as_deref());
    format!(
        "execute_file\npath={}\nargs={}\n{}",
        clean_value(&path),
        clean_value(&args.join(" ")),
        output
    )
}

fn execute_code(payload: &str) -> String {
    match payload_field(payload, "action").as_deref() {
        Some("languages") => execute_code_languages(),
        _ => run_code(payload),
    }
}

fn execute_code_languages() -> String {
    let mut rows = vec!["Language\tCommand\tStatus".to_string()];
    for runtime in language_runtimes() {
        if command_available(runtime.command) {
            rows.push(format!("{}\t{}\tavailable", runtime.id, runtime.command));
        }
    }
    if rows.len() == 1 {
        rows.push("none\t-\tNo supported language found".to_string());
    }
    format!("execute_code_languages:\n{}", rows.join("\n"))
}

fn run_code(payload: &str) -> String {
    let language = payload_field(payload, "language").unwrap_or_default();
    let Some(runtime) = language_runtimes()
        .into_iter()
        .find(|runtime| runtime.id == language)
    else {
        return format!(
            "execute_code\nstatus=failed\nlanguage={}\nmessage=unsupported language",
            clean_value(&language)
        );
    };
    if !command_available(runtime.command) {
        return format!(
            "execute_code\nstatus=failed\nlanguage={}\nmessage=language runtime is not available",
            clean_value(runtime.id)
        );
    }
    let Some(code) = payload_field(payload, "code_b64")
        .and_then(|value| STANDARD.decode(value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|value| !value.trim().is_empty())
    else {
        return format!(
            "execute_code\nstatus=failed\nlanguage={}\nmessage=code is required",
            clean_value(runtime.id)
        );
    };

    let path = std::env::temp_dir().join(format!(
        "rdl-execute-{}-{}.{}",
        std::process::id(),
        now_millis(),
        runtime.extension
    ));
    if let Err(error) = fs::write(&path, code) {
        return format!(
            "execute_code\nstatus=failed\nlanguage={}\nmessage=write temp file failed: {}",
            clean_value(runtime.id),
            clean_value(&error.to_string())
        );
    }
    let path = path.display().to_string();
    let args = runtime_args(&runtime, &path);
    let output = run_process(runtime.command, &args, None);
    let _ = fs::remove_file(&path);
    format!(
        "execute_code\nlanguage={}\ncommand={}\n{}",
        clean_value(runtime.id),
        clean_value(runtime.command),
        output
    )
}

fn execute_static_command(payload: &str) -> String {
    let preset_id = payload_field(payload, "preset")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "whoami".to_string());
    let Some(preset) = static_commands()
        .into_iter()
        .find(|preset| preset.id == preset_id)
    else {
        return format!(
            "execute_static_command\nstatus=failed\npreset={}\nmessage=unknown preset",
            clean_value(&preset_id)
        );
    };
    let script = if cfg!(target_os = "windows") {
        preset.windows
    } else {
        preset.unix
    };
    let output = run_shell(script);
    format!(
        "execute_static_command\npreset={}\nlabel={}\n{}",
        clean_value(preset.id),
        clean_value(preset.label),
        output
    )
}

#[derive(Clone, Copy)]
struct LanguageRuntime {
    id: &'static str,
    command: &'static str,
    extension: &'static str,
}

fn language_runtimes() -> Vec<LanguageRuntime> {
    let mut runtimes = vec![
        LanguageRuntime {
            id: "python3",
            command: "python3",
            extension: "py",
        },
        LanguageRuntime {
            id: "python",
            command: "python",
            extension: "py",
        },
        LanguageRuntime {
            id: "node",
            command: "node",
            extension: "js",
        },
    ];
    if cfg!(target_os = "windows") {
        runtimes.push(LanguageRuntime {
            id: "powershell",
            command: "powershell",
            extension: "ps1",
        });
    } else {
        runtimes.push(LanguageRuntime {
            id: "bash",
            command: "bash",
            extension: "sh",
        });
        runtimes.push(LanguageRuntime {
            id: "sh",
            command: "sh",
            extension: "sh",
        });
    }
    runtimes
}

fn runtime_args(runtime: &LanguageRuntime, path: &str) -> Vec<String> {
    if cfg!(target_os = "windows") && runtime.id == "powershell" {
        return vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-File".to_string(),
            path.to_string(),
        ];
    }
    vec![path.to_string()]
}

#[derive(Clone, Copy)]
struct StaticCommand {
    id: &'static str,
    label: &'static str,
    windows: &'static str,
    unix: &'static str,
}

fn static_commands() -> Vec<StaticCommand> {
    vec![
        StaticCommand {
            id: "whoami",
            label: "Who Am I",
            windows: "whoami",
            unix: "whoami",
        },
        StaticCommand {
            id: "hostname",
            label: "Hostname",
            windows: "hostname",
            unix: "hostname",
        },
        StaticCommand {
            id: "uptime",
            label: "Uptime",
            windows: "Get-CimInstance Win32_OperatingSystem | Select-Object LastBootUpTime,LocalDateTime | Format-List",
            unix: "uptime",
        },
        StaticCommand {
            id: "disk_usage",
            label: "Disk Usage",
            windows: "Get-PSDrive -PSProvider FileSystem | Select-Object Name,Used,Free,Root | Format-Table -AutoSize",
            unix: "df -h",
        },
        StaticCommand {
            id: "network_config",
            label: "Network Config",
            windows: "ipconfig",
            unix: "ifconfig 2>/dev/null || ip addr",
        },
        StaticCommand {
            id: "environment",
            label: "Environment",
            windows: "Get-ChildItem Env: | Sort-Object Name | Format-Table -AutoSize",
            unix: "env | sort",
        },
    ]
}

fn run_shell(script: &str) -> String {
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

fn run_process(program: &str, args: &[String], working_dir: Option<&str>) -> String {
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

fn command_available(program: &str) -> bool {
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

fn split_args(value: &str) -> Vec<String> {
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

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .map(str::to_string)
}

fn clean_value(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

fn now_millis() -> u128 {
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

#[cfg(test)]
mod tests {
    use super::{split_args, static_commands};

    #[test]
    fn split_args_handles_quotes() {
        assert_eq!(
            split_args(r#"--name "Ada Lovelace" 'quoted value' plain"#),
            vec!["--name", "Ada Lovelace", "quoted value", "plain"]
        );
    }

    #[test]
    fn static_commands_include_requested_basics() {
        let ids = static_commands()
            .into_iter()
            .map(|command| command.id)
            .collect::<Vec<_>>();

        assert!(ids.contains(&"whoami"));
        assert!(ids.contains(&"hostname"));
        assert!(ids.contains(&"disk_usage"));
    }
}

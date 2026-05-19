use rdl_protocol::CommandKind;
#[cfg(not(target_os = "windows"))]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const DEFAULT_EXIT_DELAY_MS: u64 = 900;
const DEFAULT_POWER_DELAY_SECONDS: u64 = 30;
const DEFAULT_RESTART_DELAY_MS: u64 = 900;

pub(crate) fn handle(command: &CommandKind, payload: &str) -> String {
    let request = SessionRequest::parse(payload);
    if !request.confirm {
        return result(
            command,
            "refused",
            vec!["message=confirm=true is required".to_string()],
        );
    }

    match command {
        CommandKind::UpdateClient => update_client(&request),
        CommandKind::UninstallClient => uninstall_client(&request),
        CommandKind::KillClientProcess => kill_client_process(&request),
        CommandKind::Shutdown => power_command(command, &request, PowerAction::Shutdown),
        CommandKind::Reboot => power_command(command, &request, PowerAction::Reboot),
        CommandKind::DeleteClient => delete_client(&request),
        _ => unreachable!("session received non-session command"),
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct SessionRequest {
    confirm: bool,
    dry_run: bool,
    delay_seconds: Option<u64>,
    update_path: Option<PathBuf>,
    remove_binary: bool,
}

impl SessionRequest {
    fn parse(payload: &str) -> Self {
        Self {
            confirm: bool_field(payload, "confirm"),
            dry_run: bool_field(payload, "dry_run"),
            delay_seconds: payload_field(payload, "delay_seconds")
                .and_then(|value| value.parse::<u64>().ok()),
            update_path: payload_field(payload, "update_path")
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            remove_binary: bool_field(payload, "remove_binary"),
        }
    }
}

pub(crate) fn schedule_config_file_restart(config_path: &Path) -> io::Result<PathBuf> {
    let current_exe = std::env::current_exe()?;
    let args = config_file_restart_args(config_path);
    schedule_restart(&current_exe, &args)?;
    schedule_exit(DEFAULT_RESTART_DELAY_MS, 0);
    Ok(current_exe)
}

fn update_client(request: &SessionRequest) -> String {
    if request.dry_run {
        return result(
            &CommandKind::UpdateClient,
            "dry_run",
            vec!["message=client restart/update would be scheduled".to_string()],
        );
    }

    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return result(
                &CommandKind::UpdateClient,
                "error",
                vec![format!("message=current_exe failed: {error}")],
            )
        }
    };
    let args = current_args();

    let schedule = if let Some(update_path) = request.update_path.as_deref() {
        if !update_path.is_file() {
            return result(
                &CommandKind::UpdateClient,
                "error",
                vec![format!(
                    "message=update_path is not a file: {}",
                    clean_value(&update_path.display().to_string())
                )],
            );
        }
        schedule_replace_and_restart(&current_exe, update_path, &args)
    } else {
        schedule_restart(&current_exe, &args)
    };

    match schedule {
        Ok(()) => {
            schedule_exit(DEFAULT_RESTART_DELAY_MS, 0);
            let mut lines = vec![
                "message=client restart scheduled".to_string(),
                format!("path={}", clean_value(&current_exe.display().to_string())),
            ];
            if let Some(update_path) = request.update_path.as_deref() {
                lines.push(format!(
                    "update_path={}",
                    clean_value(&update_path.display().to_string())
                ));
            }
            result(&CommandKind::UpdateClient, "scheduled", lines)
        }
        Err(error) => result(
            &CommandKind::UpdateClient,
            "error",
            vec![format!("message={}", clean_value(&error.to_string()))],
        ),
    }
}

fn uninstall_client(request: &SessionRequest) -> String {
    if request.dry_run {
        return result(
            &CommandKind::UninstallClient,
            "dry_run",
            vec![format!(
                "identity_path={}",
                clean_value(
                    &crate::runtime::client_identity_file_path()
                        .display()
                        .to_string()
                )
            )],
        );
    }

    let identity_result = remove_client_identity();
    let binary_result = if request.remove_binary {
        std::env::current_exe()
            .and_then(|path| schedule_remove_binary_after_exit(&path).map(|_| path))
    } else {
        Ok(PathBuf::new())
    };

    match (identity_result, binary_result) {
        (Ok(identity_path), Ok(binary_path)) => {
            schedule_exit(DEFAULT_EXIT_DELAY_MS, 0);
            let mut lines = vec![
                "message=client uninstall scheduled".to_string(),
                format!(
                    "identity_path={}",
                    clean_value(&identity_path.display().to_string())
                ),
                format!("remove_binary={}", request.remove_binary),
            ];
            if request.remove_binary {
                lines.push(format!(
                    "binary_path={}",
                    clean_value(&binary_path.display().to_string())
                ));
            }
            result(&CommandKind::UninstallClient, "scheduled", lines)
        }
        (Err(error), _) | (_, Err(error)) => result(
            &CommandKind::UninstallClient,
            "error",
            vec![format!("message={}", clean_value(&error.to_string()))],
        ),
    }
}

fn kill_client_process(request: &SessionRequest) -> String {
    if request.dry_run {
        return result(
            &CommandKind::KillClientProcess,
            "dry_run",
            vec!["message=client process exit would be scheduled".to_string()],
        );
    }

    schedule_exit(DEFAULT_EXIT_DELAY_MS, 0);
    result(
        &CommandKind::KillClientProcess,
        "scheduled",
        vec![
            "message=client process exit scheduled".to_string(),
            format!("process_id={}", std::process::id()),
        ],
    )
}

#[derive(Clone, Copy)]
enum PowerAction {
    Shutdown,
    Reboot,
}

fn power_command(command: &CommandKind, request: &SessionRequest, action: PowerAction) -> String {
    let delay = request.delay_seconds.unwrap_or(DEFAULT_POWER_DELAY_SECONDS);
    if request.dry_run {
        return result(
            command,
            "dry_run",
            vec![format!(
                "message={} would be scheduled in {delay}s",
                power_action_name(action)
            )],
        );
    }

    match schedule_power_action(action, delay) {
        Ok(()) => result(
            command,
            "scheduled",
            vec![
                format!(
                    "message={} scheduled in {delay}s",
                    power_action_name(action)
                ),
                format!("delay_seconds={delay}"),
            ],
        ),
        Err(error) => result(
            command,
            "error",
            vec![format!("message={}", clean_value(&error.to_string()))],
        ),
    }
}

fn delete_client(request: &SessionRequest) -> String {
    if request.dry_run {
        return result(
            &CommandKind::DeleteClient,
            "dry_run",
            vec!["message=client identity would be removed and process would exit".to_string()],
        );
    }

    match remove_client_identity() {
        Ok(path) => {
            schedule_exit(DEFAULT_EXIT_DELAY_MS, 0);
            result(
                &CommandKind::DeleteClient,
                "scheduled",
                vec![
                    "message=client identity removed; process exit scheduled".to_string(),
                    format!("identity_path={}", clean_value(&path.display().to_string())),
                ],
            )
        }
        Err(error) => result(
            &CommandKind::DeleteClient,
            "error",
            vec![format!("message={}", clean_value(&error.to_string()))],
        ),
    }
}

fn remove_client_identity() -> io::Result<PathBuf> {
    let path = crate::runtime::client_identity_file_path();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(path)
}

fn schedule_exit(delay_ms: u64, code: i32) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(delay_ms));
        std::process::exit(code);
    });
}

fn current_args() -> Vec<OsString> {
    std::env::args_os().skip(1).collect()
}

fn config_file_restart_args(config_path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--config"),
        config_path.as_os_str().to_os_string(),
    ]
}

#[cfg(target_os = "windows")]
fn schedule_restart(current_exe: &Path, args: &[OsString]) -> io::Result<()> {
    let start_process = powershell_start_process(current_exe, args);
    let script = format!(
        "$pidToWait={}; Wait-Process -Id $pidToWait -ErrorAction SilentlyContinue; {}",
        std::process::id(),
        start_process
    );
    spawn_powershell(&script)
}

#[cfg(not(target_os = "windows"))]
fn schedule_restart(current_exe: &Path, args: &[OsString]) -> io::Result<()> {
    let script = format!(
        "while kill -0 {} 2>/dev/null; do sleep 0.2; done; exec {} {}",
        std::process::id(),
        sh_quote(current_exe.as_os_str()),
        sh_args(args)
    );
    spawn_shell(&script)
}

#[cfg(target_os = "windows")]
fn schedule_replace_and_restart(
    current_exe: &Path,
    update_path: &Path,
    args: &[OsString],
) -> io::Result<()> {
    let start_process = powershell_start_process(current_exe, args);
    let script = format!(
        "$pidToWait={}; Wait-Process -Id $pidToWait -ErrorAction SilentlyContinue; Copy-Item -LiteralPath {} -Destination {} -Force; {}",
        std::process::id(),
        powershell_string(&update_path.display().to_string()),
        powershell_string(&current_exe.display().to_string()),
        start_process
    );
    spawn_powershell(&script)
}

#[cfg(not(target_os = "windows"))]
fn schedule_replace_and_restart(
    current_exe: &Path,
    update_path: &Path,
    args: &[OsString],
) -> io::Result<()> {
    let script = format!(
        "while kill -0 {} 2>/dev/null; do sleep 0.2; done; cp {} {}; chmod +x {}; exec {} {}",
        std::process::id(),
        sh_quote(update_path.as_os_str()),
        sh_quote(current_exe.as_os_str()),
        sh_quote(current_exe.as_os_str()),
        sh_quote(current_exe.as_os_str()),
        sh_args(args)
    );
    spawn_shell(&script)
}

#[cfg(target_os = "windows")]
fn schedule_remove_binary_after_exit(path: &Path) -> io::Result<()> {
    let script = format!(
        "$pidToWait={}; Wait-Process -Id $pidToWait -ErrorAction SilentlyContinue; Remove-Item -LiteralPath {} -Force -ErrorAction SilentlyContinue",
        std::process::id(),
        powershell_string(&path.display().to_string())
    );
    spawn_powershell(&script)
}

#[cfg(not(target_os = "windows"))]
fn schedule_remove_binary_after_exit(path: &Path) -> io::Result<()> {
    let script = format!(
        "while kill -0 {} 2>/dev/null; do sleep 0.2; done; rm -f {}",
        std::process::id(),
        sh_quote(path.as_os_str())
    );
    spawn_shell(&script)
}

#[cfg(target_os = "windows")]
fn schedule_power_action(action: PowerAction, delay_seconds: u64) -> io::Result<()> {
    let flag = match action {
        PowerAction::Shutdown => "/s",
        PowerAction::Reboot => "/r",
    };
    Command::new("shutdown")
        .args([
            flag,
            "/t",
            &delay_seconds.to_string(),
            "/c",
            "Rust Desk Light remote session command",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn schedule_power_action(action: PowerAction, delay_seconds: u64) -> io::Result<()> {
    let verb = match action {
        PowerAction::Shutdown => "shut down",
        PowerAction::Reboot => "restart",
    };
    let script = format!(
        "sleep {}; osascript -e {}",
        delay_seconds,
        sh_quote(OsStr::new(&format!(
            "tell application \"System Events\" to {verb}"
        )))
    );
    spawn_shell(&script)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn schedule_power_action(action: PowerAction, delay_seconds: u64) -> io::Result<()> {
    let command = match action {
        PowerAction::Shutdown => "systemctl poweroff || loginctl poweroff || shutdown -h now",
        PowerAction::Reboot => "systemctl reboot || loginctl reboot || shutdown -r now",
    };
    spawn_shell(&format!("sleep {delay_seconds}; {command}"))
}

#[cfg(not(any(target_os = "windows", unix)))]
fn schedule_power_action(_action: PowerAction, _delay_seconds: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "power actions are not supported on this platform",
    ))
}

fn power_action_name(action: PowerAction) -> &'static str {
    match action {
        PowerAction::Shutdown => "shutdown",
        PowerAction::Reboot => "reboot",
    }
}

#[cfg(target_os = "windows")]
fn spawn_powershell(script: &str) -> io::Result<()> {
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn spawn_shell(script: &str) -> io::Result<()> {
    Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "windows")]
fn powershell_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
fn powershell_arg_list(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| powershell_string(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(target_os = "windows")]
fn powershell_start_process(current_exe: &Path, args: &[OsString]) -> String {
    let command = format!(
        "Start-Process -FilePath {}",
        powershell_string(&current_exe.display().to_string())
    );
    if args.is_empty() {
        command
    } else {
        format!("{command} -ArgumentList @({})", powershell_arg_list(args))
    }
}

#[cfg(not(target_os = "windows"))]
fn sh_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| sh_quote(arg.as_os_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(not(target_os = "windows"))]
fn sh_quote(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn bool_field(payload: &str, key: &str) -> bool {
    payload_field(payload, key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| value.trim().to_string())
}

fn result(command: &CommandKind, status: &str, lines: Vec<String>) -> String {
    let mut output = vec![command.as_str().to_string(), format!("status={status}")];
    output.extend(lines);
    output.join("\n")
}

fn clean_value(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

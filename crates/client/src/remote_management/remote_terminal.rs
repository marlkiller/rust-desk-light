use crate::support::truncate_chars;
use rdl_protocol::{CommandOutputStream, REMOTE_TERMINAL_CANCEL};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

static CURRENT_DIR: OnceLock<Mutex<PathBuf>> = OnceLock::new();
static RUNNING_COMMAND: OnceLock<Mutex<Option<RunningCommand>>> = OnceLock::new();
static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct TerminalOutput {
    pub(crate) stream_id: u64,
    pub(crate) sequence: u64,
    pub(crate) stream: CommandOutputStream,
    pub(crate) chunk: String,
    pub(crate) current_dir: String,
    pub(crate) finished: bool,
    pub(crate) success: bool,
}

struct RunningCommand {
    child: Arc<Mutex<Child>>,
}

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

pub(crate) fn execute_streaming<F>(payload: &str, mut send: F) -> io::Result<()>
where
    F: FnMut(TerminalOutput) -> io::Result<()>,
{
    let command = payload.trim();
    if command == REMOTE_TERMINAL_CANCEL {
        return cancel_running_command(send);
    }

    let stream_id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
    let mut sequence = 1u64;
    if command.is_empty() {
        return send_final(
            &mut send,
            stream_id,
            &mut sequence,
            current_dir_label(),
            "remote_terminal requires a command payload",
            false,
        );
    }

    if let Some(target_dir) = parse_cd_target(command) {
        let detail = change_dir(target_dir);
        let (current_dir, output) = parse_terminal_response(&detail);
        let success = !terminal_output_failed(&output);
        return send_final(
            &mut send,
            stream_id,
            &mut sequence,
            current_dir.unwrap_or_else(current_dir_label),
            output.trim(),
            success,
        );
    }

    let cwd = current_dir();
    let mut child = match spawn_command(command, &cwd) {
        Ok(child) => child,
        Err(error) => {
            return send_final(
                &mut send,
                stream_id,
                &mut sequence,
                cwd.display().to_string(),
                &format!("failed to start command: {error}"),
                false,
            );
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));
    if !set_running_command(child.clone()) {
        let _ = child.lock().map(|mut child| terminate_child(&mut child));
        return send_final(
            &mut send,
            stream_id,
            &mut sequence,
            cwd.display().to_string(),
            "remote_terminal is already running a command",
            false,
        );
    }

    let (chunk_tx, chunk_rx) = mpsc::channel::<(CommandOutputStream, String)>();
    let mut reader_threads = Vec::new();
    if let Some(stdout) = stdout {
        reader_threads.push(spawn_output_reader(
            stdout,
            CommandOutputStream::Stdout,
            chunk_tx.clone(),
        ));
    }
    if let Some(stderr) = stderr {
        reader_threads.push(spawn_output_reader(
            stderr,
            CommandOutputStream::Stderr,
            chunk_tx.clone(),
        ));
    }
    drop(chunk_tx);

    let status = loop {
        match chunk_rx.recv_timeout(Duration::from_millis(25)) {
            Ok((stream, chunk)) => {
                send_chunk(
                    &mut send,
                    stream_id,
                    &mut sequence,
                    stream,
                    chunk,
                    cwd.display().to_string(),
                )?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(25)),
        }
        while let Ok((stream, chunk)) = chunk_rx.try_recv() {
            send_chunk(
                &mut send,
                stream_id,
                &mut sequence,
                stream,
                chunk,
                cwd.display().to_string(),
            )?;
        }

        let status = child
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "terminal process lock poisoned"))?
            .try_wait()?;
        if let Some(status) = status {
            break status;
        }
    };

    for handle in reader_threads {
        let _ = handle.join();
    }
    while let Ok((stream, chunk)) = chunk_rx.try_recv() {
        send_chunk(
            &mut send,
            stream_id,
            &mut sequence,
            stream,
            chunk,
            cwd.display().to_string(),
        )?;
    }
    clear_running_command(&child);

    let success = status.success();
    let final_chunk = if success {
        ""
    } else {
        "process exited with error"
    };
    send_final(
        &mut send,
        stream_id,
        &mut sequence,
        cwd.display().to_string(),
        final_chunk,
        success,
    )
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

fn spawn_command(command: &str, current_dir: &PathBuf) -> io::Result<Child> {
    if cfg!(target_os = "windows") {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &format!(
                    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[System.Text.Encoding]::UTF8; {command}"
                ),
            ])
            .current_dir(current_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    } else {
        let mut shell = Command::new("sh");
        shell
            .args(["-lc", command])
            .current_dir(current_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            shell.process_group(0);
        }
        shell.spawn()
    }
}

fn spawn_output_reader<R>(
    mut reader: R,
    stream: CommandOutputStream,
    tx: mpsc::Sender<(CommandOutputStream, String)>,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let chunk = String::from_utf8_lossy(&buffer[..count]).to_string();
                    if tx.send((stream, chunk)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send((
                        CommandOutputStream::Stderr,
                        format!("failed to read {}: {error}\n", stream.as_str()),
                    ));
                    break;
                }
            }
        }
    })
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
    let text = compact_terminal_output(&text);
    truncate_lines(&text, max_lines)
}

fn compact_terminal_output(value: &str) -> String {
    let mut lines = Vec::new();
    let mut previous_blank = false;
    for line in value.lines().map(str::trim_end) {
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        lines.push(line);
        previous_blank = blank;
    }
    lines.join("\n").trim().to_string()
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

fn parse_terminal_response(detail: &str) -> (Option<String>, String) {
    let Some(rest) = detail.strip_prefix("__rdl_terminal_cwd\t") else {
        return (None, detail.to_string());
    };
    rest.split_once('\n')
        .map(|(current_dir, output)| (Some(current_dir.to_string()), output.to_string()))
        .unwrap_or_else(|| (Some(rest.to_string()), String::new()))
}

fn terminal_output_failed(output: &str) -> bool {
    let output = output.trim().to_ascii_lowercase();
    output.starts_with("cd failed:") || output.contains(" exited with error")
}

fn send_chunk<F>(
    send: &mut F,
    stream_id: u64,
    sequence: &mut u64,
    stream: CommandOutputStream,
    chunk: String,
    current_dir: String,
) -> io::Result<()>
where
    F: FnMut(TerminalOutput) -> io::Result<()>,
{
    if chunk.is_empty() {
        return Ok(());
    }
    let output = TerminalOutput {
        stream_id,
        sequence: *sequence,
        stream,
        chunk,
        current_dir,
        finished: false,
        success: true,
    };
    *sequence = sequence.saturating_add(1);
    send(output)
}

fn send_final<F>(
    send: &mut F,
    stream_id: u64,
    sequence: &mut u64,
    current_dir: String,
    chunk: &str,
    success: bool,
) -> io::Result<()>
where
    F: FnMut(TerminalOutput) -> io::Result<()>,
{
    let output = TerminalOutput {
        stream_id,
        sequence: *sequence,
        stream: CommandOutputStream::Status,
        chunk: chunk.to_string(),
        current_dir,
        finished: true,
        success,
    };
    *sequence = sequence.saturating_add(1);
    send(output)
}

fn cancel_running_command<F>(mut send: F) -> io::Result<()>
where
    F: FnMut(TerminalOutput) -> io::Result<()>,
{
    let child = {
        let running = running_command_lock()
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "terminal process lock poisoned"))?;
        let Some(running) = running.as_ref() else {
            let mut sequence = 1;
            return send_final(
                &mut send,
                0,
                &mut sequence,
                current_dir_label(),
                "no running terminal command",
                true,
            );
        };
        running.child.clone()
    };
    if let Ok(mut child) = child.lock() {
        terminate_child(&mut child);
    }
    let mut sequence = 1;
    send_chunk(
        &mut send,
        0,
        &mut sequence,
        CommandOutputStream::Status,
        "cancel requested\n".to_string(),
        current_dir_label(),
    )
}

fn set_running_command(child: Arc<Mutex<Child>>) -> bool {
    let Ok(mut running) = running_command_lock().lock() else {
        return false;
    };
    if running.is_some() {
        return false;
    }
    *running = Some(RunningCommand { child });
    true
}

fn clear_running_command(child: &Arc<Mutex<Child>>) {
    let Ok(mut running) = running_command_lock().lock() else {
        return;
    };
    if running
        .as_ref()
        .map(|running| Arc::ptr_eq(&running.child, child))
        .unwrap_or(false)
    {
        *running = None;
    }
}

fn running_command_lock() -> &'static Mutex<Option<RunningCommand>> {
    RUNNING_COMMAND.get_or_init(|| Mutex::new(None))
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    terminate_unix_process_group(child);
    #[cfg(windows)]
    terminate_windows_process_tree(child);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.kill();
    }
}

#[cfg(unix)]
fn terminate_unix_process_group(child: &mut Child) {
    let pid = child.id();
    let group = format!("-{pid}");
    let _ = Command::new("kill").args(["-TERM", &group]).status();
    thread::sleep(Duration::from_millis(150));
    match child.try_wait() {
        Ok(Some(_)) => {}
        _ => {
            let _ = Command::new("kill").args(["-KILL", &group]).status();
            let _ = child.kill();
        }
    }
}

#[cfg(windows)]
fn terminate_windows_process_tree(child: &mut Child) {
    let pid = child.id().to_string();
    let _ = Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .status();
    let _ = child.kill();
}

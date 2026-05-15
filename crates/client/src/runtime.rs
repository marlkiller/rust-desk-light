use rdl_protocol::{DEFAULT_SERVER_IP, DEFAULT_SERVER_PORT};
use std::fs;
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

static HOSTNAME_CACHE: OnceLock<String> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) ip: String,
    pub(crate) port: u16,
}

impl Config {
    pub(crate) fn from_env() -> Self {
        let mut ip = DEFAULT_SERVER_IP.to_string();
        let mut port = DEFAULT_SERVER_PORT;
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--ip" => {
                    if let Some(value) = args.next() {
                        ip = value;
                    }
                }
                "--port" => {
                    if let Some(value) = args.next() {
                        if let Ok(value) = value.parse() {
                            port = value;
                        }
                    }
                }
                "--help" | "-h" => {
                    println!("Usage: rdl-client [--ip 127.0.0.1] [--port 21115]");
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        Self { ip, port }
    }
}

#[derive(Clone)]
pub(crate) struct LocalIdentity {
    pub(crate) id: String,
    pub(crate) fingerprint: String,
}

pub(crate) fn gui_available() -> bool {
    if std::env::var_os("RDL_FORCE_TERMINAL").is_some() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

pub(crate) fn hostname() -> String {
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
        .and_then(|value| {
            let value = value.trim().to_string();
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

pub(crate) fn os_label() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
            if let Some(value) = text
                .lines()
                .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            {
                return value.trim_matches('"').to_string();
            }
        }
    }
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

pub(crate) fn username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string())
}

pub(crate) fn load_client_identity() -> LocalIdentity {
    let path = identity_file_path("client.identity");
    if let Ok(text) = fs::read_to_string(&path) {
        let mut id = String::new();
        let mut fingerprint = String::new();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("id=") {
                id = value.trim().to_string();
            }
            if let Some(value) = line.strip_prefix("fingerprint=") {
                fingerprint = value.trim().to_string();
            }
        }
        if !id.is_empty() && !fingerprint.is_empty() {
            return LocalIdentity { id, fingerprint };
        }
    }

    let seed = format!(
        "{}|{}|{}|{}|{}",
        hostname(),
        username(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        rdl_protocol::now_epoch_ms()
    );
    let id = format!("client-{:016x}", simple_hash(&seed));
    let fingerprint = fingerprint_for(&id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, format!("id={id}\nfingerprint={fingerprint}\n"));
    LocalIdentity { id, fingerprint }
}

fn fingerprint_for(id: &str) -> String {
    format!(
        "fp-{:016x}",
        simple_hash(&format!(
            "{}|{}|{}|{}|{}",
            id,
            hostname(),
            username(),
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    )
}

fn identity_file_path(file_name: &str) -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata)
            .join("rust-desk-light")
            .join(file_name);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("rust-desk-light")
            .join(file_name);
    }
    PathBuf::from(file_name)
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

fn simple_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

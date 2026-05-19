use fs2::FileExt;
use rdl_config::{ConfigKind, EmbeddedEndpointConfig, EndpointConfig, EndpointOverrides};
use std::fs;
use std::fs::File;
use std::io;
#[cfg(target_family = "unix")]
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

#[used]
#[cfg_attr(
    all(unix, not(target_os = "macos")),
    link_section = ".rodata.rdl_client_config"
)]
#[cfg_attr(target_os = "macos", link_section = "__TEXT,__rdlcfg")]
#[cfg_attr(target_os = "windows", link_section = ".rdata$RDL")]
static RDL_CLIENT_EMBEDDED_CONFIG_SLOT: [u8; rdl_config::CLIENT_EMBEDDED_CONFIG_SLOT_BYTES] =
    rdl_config::empty_client_embedded_config_slot();

static HOSTNAME_CACHE: OnceLock<String> = OnceLock::new();

pub(crate) struct ClientProcessLock {
    _file: File,
    path: PathBuf,
}

impl ClientProcessLock {
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

pub(crate) fn acquire_client_process_lock() -> io::Result<ClientProcessLock> {
    let path = rdl_config::default_config_dir().join("client.lock");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            file.set_len(0)?;
            use std::io::Write;
            writeln!(&file, "pid={}", std::process::id())?;
            Ok(ClientProcessLock { _file: file, path })
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "another rdl-client-gui/rdl-client-cli process is already running (lock: {})",
                path.display()
            ),
        )),
        Err(error) => Err(error),
    }
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) ip: String,
    pub(crate) port: u16,
    pub(crate) auth_token: String,
    pub(crate) config_path: PathBuf,
    cli_ip: Option<String>,
    cli_port: Option<u16>,
    embedded_config: Option<EmbeddedEndpointConfig>,
    overrides: EndpointOverrides,
}

impl Config {
    pub(crate) fn from_env() -> Result<Self, rdl_config::ConfigError> {
        keep_embedded_config_slot_linked();
        let parsed = rdl_config::parse_endpoint_args(std::env::args().skip(1))?;
        if parsed.version {
            println!("{}", rdl_version::app_version(client_binary_name()));
            std::process::exit(0);
        }
        if parsed.help {
            println!(
                "{}",
                rdl_config::help_text(client_binary_name(), ConfigKind::Client)
            );
            std::process::exit(0);
        }

        Self::load(parsed.overrides)
    }

    fn load(overrides: EndpointOverrides) -> Result<Self, rdl_config::ConfigError> {
        let loaded = rdl_config::load_endpoint_config(ConfigKind::Client, &overrides)?;
        Ok(Self {
            ip: loaded.endpoint.ip,
            port: loaded.endpoint.port,
            auth_token: loaded.auth_token.unwrap_or_default(),
            config_path: loaded.config_path,
            cli_ip: loaded.cli_ip,
            cli_port: loaded.cli_port,
            embedded_config: loaded.embedded_config,
            overrides,
        })
    }

    pub(crate) fn reload(&self) -> Result<Self, rdl_config::ConfigError> {
        Self::load(self.overrides.clone())
    }

    pub(crate) fn endpoint(&self) -> EndpointConfig {
        EndpointConfig::new(self.ip.clone(), self.port)
    }

    pub(crate) fn cli_ip_overridden(&self) -> bool {
        self.cli_ip.is_some()
    }

    pub(crate) fn cli_port_overridden(&self) -> bool {
        self.cli_port.is_some()
    }

    pub(crate) fn embedded_config_enabled(&self) -> bool {
        self.embedded_config.is_some()
    }

    pub(crate) fn config_mode_label(&self) -> &'static str {
        if self.embedded_config_enabled() {
            "Embedded read-only"
        } else {
            "User config file"
        }
    }

    #[cfg(feature = "gui")]
    pub(crate) fn config_mode_detail(&self) -> String {
        if self.embedded_config_enabled() {
            "embedded read-only config; client.toml is not loaded or saved".to_string()
        } else {
            format!("client.toml: {}", self.config_path.display())
        }
    }

    pub(crate) fn startup_config_notice(&self) -> String {
        if self.embedded_config_enabled() {
            format!(
                "config mode=embedded-read-only server={}:{} client_toml=not_loaded_or_saved",
                self.ip, self.port
            )
        } else {
            format!(
                "config mode=file server={}:{} path={}",
                self.ip,
                self.port,
                self.config_path.display()
            )
        }
    }
}

#[inline(never)]
fn keep_embedded_config_slot_linked() {
    std::hint::black_box(&RDL_CLIENT_EMBEDDED_CONFIG_SLOT);
}

fn client_binary_name() -> &'static str {
    if cfg!(feature = "gui") {
        "rdl-client-gui"
    } else {
        "rdl-client-cli"
    }
}

pub(crate) struct ClientConfigUpdate {
    pub(crate) accepted: bool,
    pub(crate) detail: String,
    pub(crate) restart: bool,
}

pub(crate) fn update_client_config(config: &Config, payload: &str) -> ClientConfigUpdate {
    let request = ClientConfigUpdateRequest::parse(payload);
    if request.show {
        return ClientConfigUpdate {
            accepted: true,
            detail: client_config_detail("current", config, config, None, false, "current config"),
            restart: false,
        };
    }
    if !request.confirm {
        return ClientConfigUpdate {
            accepted: false,
            detail: "client_config\nstatus=refused\nmessage=confirm=true is required".to_string(),
            restart: false,
        };
    }
    if let Some(port) = request.invalid_port.as_ref() {
        return ClientConfigUpdate {
            accepted: false,
            detail: format!(
                "client_config\nstatus=error\nmessage=invalid port: {}",
                clean_value(port)
            ),
            restart: false,
        };
    }
    if config.embedded_config_enabled() {
        return ClientConfigUpdate {
            accepted: false,
            detail: client_config_detail(
                "refused",
                config,
                config,
                None,
                false,
                "embedded read-only config is active; client.toml is not loaded or saved",
            ),
            restart: false,
        };
    }

    let mut endpoint = config.endpoint();
    if let Some(ip) = request.ip.as_ref() {
        endpoint.ip = ip.clone();
    }
    if let Some(port) = request.port {
        endpoint.port = port;
    }
    if endpoint.ip.trim().is_empty() {
        return ClientConfigUpdate {
            accepted: false,
            detail: "client_config\nstatus=error\nmessage=ip cannot be empty".to_string(),
            restart: false,
        };
    }
    if request.ip.is_none() && request.port.is_none() {
        return ClientConfigUpdate {
            accepted: false,
            detail: "client_config\nstatus=error\nmessage=ip or port is required".to_string(),
            restart: false,
        };
    }

    if request.dry_run {
        let mut next = config.clone();
        if !config.cli_ip_overridden() {
            next.ip = endpoint.ip.clone();
        }
        if !config.cli_port_overridden() {
            next.port = endpoint.port;
        }
        return ClientConfigUpdate {
            accepted: true,
            detail: client_config_detail(
                "dry_run",
                config,
                &next,
                Some(&endpoint),
                false,
                "config would be updated",
            ),
            restart: false,
        };
    }

    if let Err(error) =
        rdl_config::write_endpoint_config(ConfigKind::Client, &config.config_path, &endpoint)
    {
        return ClientConfigUpdate {
            accepted: false,
            detail: format!(
                "client_config\nstatus=error\nmessage={}",
                clean_value(&error.to_string())
            ),
            restart: false,
        };
    }
    if let Some(auth_token) = request
        .auth_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if let Err(error) =
            rdl_config::write_auth_token_config(ConfigKind::Client, &config.config_path, auth_token)
        {
            return ClientConfigUpdate {
                accepted: false,
                detail: format!(
                    "client_config\nstatus=error\nmessage=config was written but auth token save failed: {}",
                    clean_value(&error.to_string())
                ),
                restart: false,
            };
        }
    }

    let reloaded = match config.reload() {
        Ok(config) => config,
        Err(error) => {
            return ClientConfigUpdate {
                accepted: false,
                detail: format!(
                    "client_config\nstatus=error\nmessage=config was written but reload failed: {}",
                    clean_value(&error.to_string())
                ),
                restart: false,
            }
        }
    };
    let effective_changed = reloaded.ip != config.ip || reloaded.port != config.port;
    let restart = request.restart;
    let message = if restart {
        "config updated; client restart requested from config file"
    } else if effective_changed {
        "config updated; restart disabled"
    } else if config.cli_ip_overridden() || config.cli_port_overridden() {
        "config updated; startup arguments still override effective endpoint"
    } else {
        "config updated"
    };

    ClientConfigUpdate {
        accepted: true,
        detail: client_config_detail(
            "updated",
            config,
            &reloaded,
            Some(&endpoint),
            restart,
            message,
        ),
        restart,
    }
}

#[derive(Debug, Default)]
struct ClientConfigUpdateRequest {
    confirm: bool,
    dry_run: bool,
    show: bool,
    restart: bool,
    auth_token: Option<String>,
    ip: Option<String>,
    port: Option<u16>,
    invalid_port: Option<String>,
}

impl ClientConfigUpdateRequest {
    fn parse(payload: &str) -> Self {
        let action = payload_field(payload, "action");
        Self {
            confirm: bool_field(payload, "confirm"),
            dry_run: bool_field(payload, "dry_run"),
            show: action.as_deref() == Some("show"),
            restart: payload_field(payload, "restart")
                .or_else(|| payload_field(payload, "reconnect"))
                .map(|value| bool_value(&value))
                .unwrap_or(true),
            auth_token: payload_field(payload, "auth_token")
                .filter(|value| !value.trim().is_empty()),
            ip: payload_field(payload, "ip").filter(|value| !value.trim().is_empty()),
            port: payload_field(payload, "port").and_then(|value| value.parse::<u16>().ok()),
            invalid_port: payload_field(payload, "port")
                .filter(|value| value.parse::<u16>().is_err()),
        }
    }
}

fn client_config_detail(
    status: &str,
    current: &Config,
    effective: &Config,
    file_endpoint: Option<&EndpointConfig>,
    restart: bool,
    message: &str,
) -> String {
    let file_endpoint = file_endpoint
        .cloned()
        .unwrap_or_else(|| effective.endpoint());
    [
        "client_config".to_string(),
        format!("status={status}"),
        format!("message={}", clean_value(message)),
        format!(
            "path={}",
            clean_value(&current.config_path.display().to_string())
        ),
        format!("config_ip={}", clean_value(&file_endpoint.ip)),
        format!("config_port={}", file_endpoint.port),
        format!("effective_ip={}", clean_value(&effective.ip)),
        format!("effective_port={}", effective.port),
        format!("cli_ip_override={}", current.cli_ip_overridden()),
        format!("cli_port_override={}", current.cli_port_overridden()),
        format!("embedded_config={}", current.embedded_config_enabled()),
        format!("config_mode={}", clean_value(current.config_mode_label())),
        format!(
            "auth_token_configured={}",
            !effective.auth_token.trim().is_empty()
        ),
        format!("restart={restart}"),
        format!(
            "restart_mode={}",
            if restart { "config_file" } else { "none" }
        ),
    ]
    .join("\n")
}

#[derive(Clone)]
pub(crate) struct LocalIdentity {
    pub(crate) id: String,
    pub(crate) fingerprint: String,
}

#[cfg(feature = "gui")]
pub(crate) fn gui_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(not(feature = "gui"))]
pub(crate) fn gui_available() -> bool {
    false
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
    let path = client_identity_file_path();
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

pub(crate) fn client_identity_file_path() -> PathBuf {
    identity_file_path("client.identity")
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
    rdl_config::default_config_dir().join(file_name)
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

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    if let Some(value) = payload
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix).map(str::trim))
        .map(|value| value.trim_matches('"').to_string())
    {
        return Some(value);
    }
    payload.split_whitespace().find_map(|part| {
        part.strip_prefix(&prefix)
            .map(|value| value.trim_matches('"').to_string())
    })
}

fn bool_field(payload: &str, key: &str) -> bool {
    payload_field(payload, key)
        .map(|value| bool_value(&value))
        .unwrap_or(false)
}

fn bool_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y" | "on"
    )
}

fn clean_value(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

fn simple_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

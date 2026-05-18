use rdl_config::{ConfigKind, EndpointConfig, EndpointOverrides};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) ip: String,
    pub(crate) port: u16,
    pub(crate) auth_token: String,
    pub(crate) config_path: PathBuf,
    startup_notice: String,
    overrides: EndpointOverrides,
}

impl Config {
    pub(crate) fn from_env() -> Result<Self, rdl_config::ConfigError> {
        let parsed = rdl_config::parse_endpoint_args(std::env::args().skip(1))?;
        if parsed.version {
            println!("{}", rdl_version::app_version("rdl-admin-gui"));
            std::process::exit(0);
        }
        if parsed.help {
            println!(
                "{}",
                rdl_config::help_text("rdl-admin-gui", ConfigKind::Admin)
            );
            std::process::exit(0);
        }

        Self::load(parsed.overrides)
    }

    fn load(overrides: EndpointOverrides) -> Result<Self, rdl_config::ConfigError> {
        let loaded = rdl_config::load_endpoint_config(ConfigKind::Admin, &overrides)?;
        let startup_notice = admin_startup_config_notice(&loaded);
        Ok(Self {
            ip: loaded.endpoint.ip,
            port: loaded.endpoint.port,
            auth_token: loaded.auth_token.unwrap_or_default(),
            config_path: loaded.config_path,
            startup_notice,
            overrides,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn reload(&self) -> Result<Self, rdl_config::ConfigError> {
        Self::load(self.overrides.clone())
    }

    pub(crate) fn startup_config_notice(&self) -> &str {
        &self.startup_notice
    }

    pub(crate) fn save_server_connection(
        &mut self,
        ip: &str,
        port: u16,
        token: &str,
    ) -> Result<(), rdl_config::ConfigError> {
        let endpoint = EndpointConfig::new(ip, port);
        rdl_config::write_endpoint_config(ConfigKind::Admin, &self.config_path, &endpoint)?;
        rdl_config::write_auth_token_config(ConfigKind::Admin, &self.config_path, token)?;
        self.ip = ip.to_string();
        self.port = port;
        self.auth_token = token.to_string();
        self.startup_notice = format!(
            "config file: {}\nserver: {}:{} (ui)\nauth token: ui",
            self.config_path.display(),
            self.ip,
            self.port
        );
        Ok(())
    }

    pub(crate) fn save_ui_preferences(
        &mut self,
        theme: &str,
        language: &str,
    ) -> Result<(), rdl_config::ConfigError> {
        rdl_config::write_ui_config(ConfigKind::Admin, &self.config_path, theme, language)?;
        Ok(())
    }
}

fn admin_startup_config_notice(loaded: &rdl_config::LoadedEndpointConfig) -> String {
    format!(
        "config file: {}\nserver: {}:{} ({})\nauth token: {}",
        loaded.config_path.display(),
        loaded.endpoint.ip,
        loaded.endpoint.port,
        endpoint_source_label(loaded),
        auth_source_label(loaded, false)
    )
}

fn endpoint_source_label(loaded: &rdl_config::LoadedEndpointConfig) -> &'static str {
    if loaded.cli_ip.is_some() || loaded.cli_port.is_some() {
        "args"
    } else if loaded.file_ip.is_some() || loaded.file_port.is_some() {
        "file"
    } else {
        "default"
    }
}

fn auth_source_label(loaded: &rdl_config::LoadedEndpointConfig, generated: bool) -> &'static str {
    if loaded.cli_auth_token.is_some() {
        "args"
    } else if std::env::var("RDL_AUTH_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .is_some()
    {
        "env"
    } else if loaded.file_auth_token.is_some() {
        "file"
    } else if generated {
        "generated"
    } else {
        "none"
    }
}

#[derive(Clone)]
pub(crate) struct LocalIdentity {
    pub(crate) id: String,
    pub(crate) fingerprint: String,
}

pub(crate) fn hostname() -> String {
    std::env::var("HOSTNAME")
        .map_err(|error| error.to_string())
        .or_else(|_| std::env::var("COMPUTERNAME").map_err(|error| error.to_string()))
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

pub(crate) fn load_admin_identity() -> LocalIdentity {
    let path = identity_file_path("admin.identity");
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
        "{}|{}|{}|{}",
        username(),
        hostname(),
        std::env::consts::OS,
        rdl_protocol::now_epoch_ms()
    );
    let id = format!(
        "admin-{}-{:08x}",
        sanitize(&username()),
        simple_hash(&seed) as u32
    );
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
            "{}|{}|{}|{}",
            id,
            hostname(),
            username(),
            std::env::consts::OS
        ))
    )
}

fn identity_file_path(file_name: &str) -> PathBuf {
    rdl_config::default_config_dir().join(file_name)
}

fn sanitize(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    if sanitized.is_empty() {
        "admin".to_string()
    } else {
        sanitized
    }
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

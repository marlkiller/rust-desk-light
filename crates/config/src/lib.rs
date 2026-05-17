use rdl_protocol::{DEFAULT_SERVER_IP, DEFAULT_SERVER_PORT};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigKind {
    Admin,
    Client,
    Server,
}

impl ConfigKind {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Admin => "admin.toml",
            Self::Client => "client.toml",
            Self::Server => "server.toml",
        }
    }

    pub fn endpoint_section(self) -> &'static str {
        match self {
            Self::Admin | Self::Client => "server",
            Self::Server => "listen",
        }
    }

    pub fn default_ip(self) -> &'static str {
        match self {
            Self::Admin | Self::Client => DEFAULT_SERVER_IP,
            Self::Server => "0.0.0.0",
        }
    }

    pub fn default_port(self) -> u16 {
        DEFAULT_SERVER_PORT
    }

    fn endpoint_sections(self) -> &'static [&'static str] {
        match self {
            Self::Admin | Self::Client => &["server", "network"],
            Self::Server => &["listen", "server", "network"],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointConfig {
    pub ip: String,
    pub port: u16,
}

impl EndpointConfig {
    pub fn new(ip: impl Into<String>, port: u16) -> Self {
        Self {
            ip: ip.into(),
            port,
        }
    }

    pub fn addr(&self) -> String {
        format!("{}:{}", self.ip, self.port)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EndpointOverrides {
    pub config_path: Option<PathBuf>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub auth_token: Option<String>,
    pub require_client_auth: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedEndpointConfig {
    pub endpoint: EndpointConfig,
    pub config_path: PathBuf,
    pub config_exists: bool,
    pub auth_token: Option<String>,
    pub require_client_auth: bool,
    pub file_ip: Option<String>,
    pub file_port: Option<u16>,
    pub file_auth_token: Option<String>,
    pub file_require_client_auth: Option<bool>,
    pub cli_ip: Option<String>,
    pub cli_port: Option<u16>,
    pub cli_auth_token: Option<String>,
    pub cli_require_client_auth: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedEndpointArgs {
    pub overrides: EndpointOverrides,
    pub help: bool,
    pub version: bool,
}

#[derive(Debug)]
pub enum ConfigError {
    MissingValue(&'static str),
    InvalidPort { value: String },
    InvalidBool { key: String, value: String },
    Io { path: PathBuf, error: io::Error },
    Parse { path: PathBuf, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue(flag) => write!(f, "missing value for {flag}"),
            Self::InvalidPort { value } => write!(f, "invalid port: {value}"),
            Self::InvalidBool { key, value } => write!(f, "invalid boolean {key}: {value}"),
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Parse { path, message } => write!(f, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {}

pub fn parse_endpoint_args<I>(args: I) -> Result<ParsedEndpointArgs, ConfigError>
where
    I: IntoIterator<Item = String>,
{
    let mut parsed = ParsedEndpointArgs::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                let value = args.next().ok_or(ConfigError::MissingValue("--config"))?;
                parsed.overrides.config_path = Some(PathBuf::from(value));
            }
            "--ip" => {
                let value = args.next().ok_or(ConfigError::MissingValue("--ip"))?;
                parsed.overrides.ip = Some(value);
            }
            "--port" => {
                let value = args.next().ok_or(ConfigError::MissingValue("--port"))?;
                parsed.overrides.port = Some(parse_port(&value)?);
            }
            "--auth-token" => {
                parsed.overrides.auth_token = Some(
                    args.next()
                        .ok_or(ConfigError::MissingValue("--auth-token"))?,
                );
            }
            "--require-client-auth" => parsed.overrides.require_client_auth = Some(true),
            "--no-require-client-auth" => parsed.overrides.require_client_auth = Some(false),
            "--version" | "-V" => parsed.version = true,
            "--help" | "-h" => parsed.help = true,
            _ if arg.starts_with("--config=") => {
                parsed.overrides.config_path = Some(PathBuf::from(&arg["--config=".len()..]));
            }
            _ if arg.starts_with("--ip=") => {
                parsed.overrides.ip = Some(arg["--ip=".len()..].to_string());
            }
            _ if arg.starts_with("--port=") => {
                let value = &arg["--port=".len()..];
                parsed.overrides.port = Some(parse_port(value)?);
            }
            _ if arg.starts_with("--auth-token=") => {
                parsed.overrides.auth_token = Some(arg["--auth-token=".len()..].to_string());
            }
            _ => {}
        }
    }

    Ok(parsed)
}

pub fn default_config_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("rust-desk-light");
    }
    if let Some(xdg_config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg_config_home).join("rust-desk-light");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("rust-desk-light");
    }
    PathBuf::from(".")
}

pub fn default_config_path(kind: ConfigKind) -> PathBuf {
    default_config_dir().join(kind.file_name())
}

pub fn load_endpoint_config(
    kind: ConfigKind,
    overrides: &EndpointOverrides,
) -> Result<LoadedEndpointConfig, ConfigError> {
    let config_path = overrides
        .config_path
        .clone()
        .unwrap_or_else(|| default_config_path(kind));
    let mut endpoint = EndpointConfig::new(kind.default_ip(), kind.default_port());
    let mut config_exists = false;
    let mut file_ip = None;
    let mut file_port = None;
    let mut auth_token = None;
    let mut require_client_auth = false;
    let mut file_auth_token = None;
    let mut file_require_client_auth = None;

    match fs::read_to_string(&config_path) {
        Ok(text) => {
            config_exists = true;
            let document = ConfigDocument::parse(&text, &config_path)?;
            if let Some(value) = document.endpoint_string(kind, "ip") {
                endpoint.ip = value.clone();
                file_ip = Some(value);
            }
            if let Some(value) = document.endpoint_port(kind, "port")? {
                endpoint.port = value;
                file_port = Some(value);
            }
            if let Some(value) = document.auth_token() {
                auth_token = Some(value.clone());
                file_auth_token = Some(value);
            }
            if let Some(value) = document.auth_bool("require_client_auth")? {
                require_client_auth = value;
                file_require_client_auth = Some(value);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ConfigError::Io {
                path: config_path,
                error,
            });
        }
    }

    if let Some(ip) = overrides.ip.as_ref() {
        endpoint.ip = ip.clone();
    }
    if let Some(port) = overrides.port {
        endpoint.port = port;
    }
    if let Some(token) = std::env::var("RDL_AUTH_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
    {
        auth_token = Some(token);
    }
    if let Some(value) = overrides.auth_token.as_ref() {
        auth_token = Some(value.clone());
    }
    if let Some(value) = overrides.require_client_auth {
        require_client_auth = value;
    }

    if !config_exists {
        write_endpoint_config(kind, &config_path, &endpoint)?;
    }

    Ok(LoadedEndpointConfig {
        endpoint,
        config_path,
        config_exists,
        auth_token,
        require_client_auth,
        file_ip,
        file_port,
        file_auth_token,
        file_require_client_auth,
        cli_ip: overrides.ip.clone(),
        cli_port: overrides.port,
        cli_auth_token: overrides.auth_token.clone(),
        cli_require_client_auth: overrides.require_client_auth,
    })
}

pub fn write_endpoint_config(
    kind: ConfigKind,
    path: &Path,
    endpoint: &EndpointConfig,
) -> Result<(), ConfigError> {
    let document = match fs::read_to_string(path) {
        Ok(text) => ConfigDocument::parse(&text, path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => ConfigDocument::default(),
        Err(error) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                error,
            })
        }
    };
    let text = document.with_endpoint(kind, endpoint).to_toml_string(kind);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ConfigError::Io {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    fs::write(path, text).map_err(|error| ConfigError::Io {
        path: path.to_path_buf(),
        error,
    })
}

pub fn write_auth_token_config(
    kind: ConfigKind,
    path: &Path,
    token: &str,
) -> Result<(), ConfigError> {
    let document = match fs::read_to_string(path) {
        Ok(text) => ConfigDocument::parse(&text, path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => ConfigDocument::default(),
        Err(error) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                error,
            })
        }
    };
    let text = document.with_auth_token(token).to_toml_string(kind);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ConfigError::Io {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    fs::write(path, text).map_err(|error| ConfigError::Io {
        path: path.to_path_buf(),
        error,
    })
}

pub fn write_require_client_auth_config(
    kind: ConfigKind,
    path: &Path,
    required: bool,
) -> Result<(), ConfigError> {
    let document = match fs::read_to_string(path) {
        Ok(text) => ConfigDocument::parse(&text, path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => ConfigDocument::default(),
        Err(error) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                error,
            })
        }
    };
    let text = document
        .with_require_client_auth(required)
        .to_toml_string(kind);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ConfigError::Io {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    fs::write(path, text).map_err(|error| ConfigError::Io {
        path: path.to_path_buf(),
        error,
    })
}

pub fn help_text(binary: &str, kind: ConfigKind) -> String {
    format!(
        "Usage: {binary} [--config PATH] [--ip {}] [--port {}] [--auth-token TOKEN] [--version]\n\nServer only: [--require-client-auth] [--no-require-client-auth]\nConfig file: {}\nPriority: built-in defaults < config file < environment < startup arguments.",
        kind.default_ip(),
        kind.default_port(),
        default_config_path(kind).display()
    )
}

fn parse_port(value: &str) -> Result<u16, ConfigError> {
    value.parse::<u16>().map_err(|_| ConfigError::InvalidPort {
        value: value.to_string(),
    })
}

fn parse_bool(key: &str, value: &str) -> Result<bool, ConfigError> {
    match parse_toml_string(value).to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => Ok(true),
        "false" | "no" | "0" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidBool {
            key: key.to_string(),
            value: value.to_string(),
        }),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ConfigDocument {
    top_level: BTreeMap<String, String>,
    sections: BTreeMap<String, BTreeMap<String, String>>,
}

impl ConfigDocument {
    fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let mut document = Self::default();
        let mut section: Option<String> = None;

        for (index, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                let Some(name) = line
                    .strip_prefix('[')
                    .and_then(|line| line.strip_suffix(']'))
                else {
                    return Err(ConfigError::Parse {
                        path: path.to_path_buf(),
                        message: format!("line {} has an invalid section header", index + 1),
                    });
                };
                let name = name.trim();
                if name.is_empty() {
                    return Err(ConfigError::Parse {
                        path: path.to_path_buf(),
                        message: format!("line {} has an empty section name", index + 1),
                    });
                }
                section = Some(name.to_string());
                document.sections.entry(name.to_string()).or_default();
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(ConfigError::Parse {
                    path: path.to_path_buf(),
                    message: format!("line {} expected key = value", index + 1),
                });
            };
            let key = key.trim();
            if key.is_empty() {
                return Err(ConfigError::Parse {
                    path: path.to_path_buf(),
                    message: format!("line {} has an empty key", index + 1),
                });
            }
            let value = strip_inline_comment(value.trim()).trim().to_string();
            match section.as_deref() {
                Some(section) => {
                    document
                        .sections
                        .entry(section.to_string())
                        .or_default()
                        .insert(key.to_string(), value);
                }
                None => {
                    document.top_level.insert(key.to_string(), value);
                }
            }
        }

        Ok(document)
    }

    fn endpoint_string(&self, kind: ConfigKind, key: &str) -> Option<String> {
        for section in kind.endpoint_sections() {
            if let Some(value) = self
                .sections
                .get(*section)
                .and_then(|section| section.get(key))
                .map(|value| parse_toml_string(value))
            {
                return Some(value);
            }
        }
        self.top_level
            .get(key)
            .map(|value| parse_toml_string(value))
    }

    fn auth_token(&self) -> Option<String> {
        self.sections
            .get("auth")
            .and_then(|section| section.get("token"))
            .map(|value| parse_toml_string(value))
            .or_else(|| {
                self.top_level
                    .get("auth_token")
                    .map(|value| parse_toml_string(value))
            })
    }

    fn auth_bool(&self, key: &str) -> Result<Option<bool>, ConfigError> {
        match self
            .sections
            .get("auth")
            .and_then(|section| section.get(key))
        {
            Some(value) => parse_bool(key, value).map(Some),
            None => Ok(None),
        }
    }

    fn endpoint_port(&self, kind: ConfigKind, key: &str) -> Result<Option<u16>, ConfigError> {
        match self.endpoint_string(kind, key) {
            Some(value) => parse_port(&value).map(Some),
            None => Ok(None),
        }
    }

    fn with_endpoint(mut self, kind: ConfigKind, endpoint: &EndpointConfig) -> Self {
        let section = self
            .sections
            .entry(kind.endpoint_section().to_string())
            .or_default();
        section.insert("ip".to_string(), format_toml_string(&endpoint.ip));
        section.insert("port".to_string(), endpoint.port.to_string());
        self
    }

    fn with_auth_token(mut self, token: &str) -> Self {
        let section = self.sections.entry("auth".to_string()).or_default();
        section.insert("token".to_string(), format_toml_string(token));
        self
    }

    fn with_require_client_auth(mut self, required: bool) -> Self {
        let section = self.sections.entry("auth".to_string()).or_default();
        section.insert("require_client_auth".to_string(), required.to_string());
        self
    }

    fn to_toml_string(&self, kind: ConfigKind) -> String {
        let mut out = String::new();
        out.push_str("# rust-desk-light configuration\n");
        out.push_str("# Priority: built-in defaults < config file < startup arguments.\n\n");

        for (key, value) in &self.top_level {
            out.push_str(key);
            out.push_str(" = ");
            out.push_str(value);
            out.push('\n');
        }
        if !self.top_level.is_empty() {
            out.push('\n');
        }

        let preferred = kind.endpoint_section();
        if let Some(section) = self.sections.get(preferred) {
            write_section(&mut out, preferred, section);
        }
        for (name, section) in &self.sections {
            if name != preferred {
                write_section(&mut out, name, section);
            }
        }

        out
    }
}

fn write_section(out: &mut String, name: &str, section: &BTreeMap<String, String>) {
    out.push('[');
    out.push_str(name);
    out.push_str("]\n");
    for (key, value) in section {
        out.push_str(key);
        out.push_str(" = ");
        out.push_str(value);
        out.push('\n');
    }
    out.push('\n');
}

fn strip_inline_comment(value: &str) -> &str {
    let mut in_quote = false;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            '#' if !in_quote => return &value[..index],
            _ => {}
        }
    }
    value
}

fn parse_toml_string(value: &str) -> String {
    let value = value.trim();
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        let mut out = String::new();
        let mut escaped = false;
        for ch in inner.chars() {
            if escaped {
                match ch {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    other => out.push(other),
                }
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else {
                out.push(ch);
            }
        }
        if escaped {
            out.push('\\');
        }
        return out;
    }
    value.to_string()
}

fn format_toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_endpoint_with_expected_priority() {
        let document = ConfigDocument::parse(
            r#"
            [server]
            ip = "10.0.0.5"
            port = 6000
            "#,
            Path::new("test.toml"),
        )
        .unwrap();

        assert_eq!(
            document
                .endpoint_string(ConfigKind::Client, "ip")
                .as_deref(),
            Some("10.0.0.5")
        );
        assert_eq!(
            document.endpoint_port(ConfigKind::Client, "port").unwrap(),
            Some(6000)
        );
    }

    #[test]
    fn parses_cli_overrides() {
        let parsed = parse_endpoint_args([
            "--config".to_string(),
            "client.toml".to_string(),
            "--ip=10.0.0.2".to_string(),
            "--port".to_string(),
            "7777".to_string(),
        ])
        .unwrap();

        assert_eq!(
            parsed.overrides.config_path,
            Some(PathBuf::from("client.toml"))
        );
        assert_eq!(parsed.overrides.ip.as_deref(), Some("10.0.0.2"));
        assert_eq!(parsed.overrides.port, Some(7777));
    }

    #[test]
    fn writes_endpoint_without_losing_other_sections() {
        let document = ConfigDocument::parse(
            r#"
            [ui]
            theme = "light"
            "#,
            Path::new("test.toml"),
        )
        .unwrap();

        let text = document
            .with_endpoint(ConfigKind::Admin, &EndpointConfig::new("192.168.1.9", 7000))
            .to_toml_string(ConfigKind::Admin);

        assert!(text.contains("[server]"));
        assert!(text.contains("ip = \"192.168.1.9\""));
        assert!(text.contains("port = 7000"));
        assert!(text.contains("[ui]"));
        assert!(text.contains("theme = \"light\""));
    }

    #[test]
    fn load_initializes_missing_config_file() {
        let path = std::env::temp_dir().join(format!(
            "rdl-config-test-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let loaded = load_endpoint_config(
            ConfigKind::Admin,
            &EndpointOverrides {
                config_path: Some(path.clone()),
                ip: Some("10.0.0.7".to_string()),
                port: Some(7777),
                ..EndpointOverrides::default()
            },
        )
        .unwrap();

        assert!(!loaded.config_exists);
        assert_eq!(loaded.endpoint, EndpointConfig::new("10.0.0.7", 7777));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[server]"));
        assert!(text.contains("ip = \"10.0.0.7\""));
        assert!(text.contains("port = 7777"));
        let _ = fs::remove_file(path);
    }
}

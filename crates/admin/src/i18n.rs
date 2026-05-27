use rdl_protocol::CommandKind;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

static CURRENT_LANGUAGE_INDEX: AtomicU8 = AtomicU8::new(0);
static LANGUAGES: OnceLock<Vec<LanguageInfo>> = OnceLock::new();
static TRANSLATIONS: OnceLock<HashMap<String, HashMap<String, &'static str>>> = OnceLock::new();

struct LanguageInfo {
    code: String,
    label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Language(u8);

impl Language {
    pub(crate) const ENGLISH: Language = Language(0);

    pub(crate) fn from_config(value: &str) -> Self {
        let code = value.trim().to_ascii_lowercase();
        let languages = get_languages();
        for (i, lang) in languages.iter().enumerate() {
            if lang.code.to_ascii_lowercase() == code {
                return Language(i as u8);
            }
        }
        // Fallbacks
        if matches!(code.as_str(), "en" | "english") {
            return Self::ENGLISH;
        }
        Self::ENGLISH
    }

    pub(crate) fn as_config(self) -> &'static str {
        get_languages()
            .get(self.0 as usize)
            .map(|l| leak_string(&l.code))
            .unwrap_or("en")
    }

    pub(crate) fn native_label(self) -> &'static str {
        get_languages()
            .get(self.0 as usize)
            .map(|l| leak_string(&l.label))
            .unwrap_or("English")
    }

    pub(crate) const ALL: LanguageList = LanguageList;
}

pub(crate) struct LanguageList;

impl IntoIterator for LanguageList {
    type Item = Language;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        (0..get_languages().len())
            .map(|i| Language(i as u8))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

fn get_languages() -> &'static [LanguageInfo] {
    LANGUAGES.get_or_init(|| {
        vec![LanguageInfo {
            code: "en".to_string(),
            label: "English".to_string(),
        }]
    })
}

pub(crate) fn initialize(config_dir: &Path) {
    let mut all_languages = vec![LanguageInfo {
        code: "en".to_string(),
        label: "English".to_string(),
    }];

    let mut all_translations = HashMap::new();

    // Load from i18n directory
    let i18n_dir = config_dir.join("i18n");
    if let Ok(entries) = std::fs::read_dir(i18n_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let code = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                    if code == "en" {
                        continue;
                    }
                    if let Some(label) = parse_toml_label(&text) {
                        if let Some(translations) = parse_i18n_toml(&text) {
                            // Update or add language
                            if let Some(existing) = all_languages.iter_mut().find(|l| l.code == code)
                            {
                                existing.label = label;
                            } else {
                                all_languages.push(LanguageInfo {
                                    code: code.clone(),
                                    label,
                                });
                            }
                            all_translations.insert(code, leak_translations(translations));
                        }
                    }
                }
            }
        }
    }

    let _ = LANGUAGES.set(all_languages);
    let _ = TRANSLATIONS.set(all_translations);
}

fn leak_translations(map: HashMap<String, String>) -> HashMap<String, &'static str> {
    let mut leaked = HashMap::new();
    for (k, v) in map {
        leaked.insert(k, leak_string(&v));
    }
    leaked
}

fn leak_string(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn parse_toml_label(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("label") {
            if let Some(value) = rest.trim().strip_prefix('=') {
                return Some(parse_toml_value(value.trim()));
            }
        }
    }
    None
}

fn parse_i18n_toml(text: &str) -> Option<HashMap<String, String>> {
    let mut map = HashMap::new();
    let mut in_translations = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[translations]" {
            in_translations = true;
            continue;
        }
        if line.starts_with('[') {
            in_translations = false;
            continue;
        }
        if in_translations {
            if let Some((key, value)) = line.split_once('=') {
                let key = parse_toml_value(key.trim());
                let value = parse_toml_value(value.trim());
                map.insert(key, value);
            }
        }
    }
    Some(map)
}

fn parse_toml_value(value: &str) -> String {
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
        return out;
    }
    value.to_string()
}

pub(crate) fn set_language(language: Language) {
    CURRENT_LANGUAGE_INDEX.store(language.0, Ordering::Relaxed);
}

pub(crate) fn current_language() -> Language {
    Language(CURRENT_LANGUAGE_INDEX.load(Ordering::Relaxed))
}

pub(crate) fn t(key: &str) -> &str {
    let index = CURRENT_LANGUAGE_INDEX.load(Ordering::Relaxed);
    if index == 0 {
        return key;
    }

    if let Some(languages) = LANGUAGES.get() {
        if let Some(lang) = languages.get(index as usize) {
            if let Some(translations) = TRANSLATIONS.get() {
                if let Some(map) = translations.get(&lang.code) {
                    if let Some(translated) = map.get(key) {
                        return translated;
                    }
                }
            }
        }
    }
    key
}

pub(crate) fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = t(key).to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

pub(crate) fn theme_label(theme: crate::theme::ThemeKind) -> &'static str {
    match theme {
        crate::theme::ThemeKind::System => t("System"),
        crate::theme::ThemeKind::Light => t("Light"),
        crate::theme::ThemeKind::Dark => t("Dark"),
    }
}

pub(crate) fn command_title(command: &CommandKind) -> &'static str {
    t(command_key(command))
}

pub(crate) fn command_key(command: &CommandKind) -> &'static str {
    match command {
        CommandKind::UpdateClient => "Update Client",
        CommandKind::UninstallClient => "Uninstall Client",
        CommandKind::KillClientProcess => "Kill Client Process",
        CommandKind::Shutdown => "Shutdown",
        CommandKind::Reboot => "Reboot",
        CommandKind::MoveToGroup => "Move To Group",
        CommandKind::CloneClientSettings => "Clone Client Settings",
        CommandKind::ClientConfig => "Client Config",
        CommandKind::DeleteClient => "Delete Client",
        CommandKind::FileManager => "File Manager",
        CommandKind::RemoteTerminal => "Remote Terminal",
        CommandKind::ProcessManager => "Process Manager",
        CommandKind::WindowManager => "Window Manager",
        CommandKind::StartupManager => "Startup Manager",
        CommandKind::RegistryManager => "Registry Manager",
        CommandKind::DriverManager => "Driver Manager",
        CommandKind::EventLog => "Event Log",
        CommandKind::ActiveConnections => "Active Connections",
        CommandKind::PerformanceMonitor => "Performance Monitor",
        CommandKind::RemoteDesktop => "Remote Desktop",
        CommandKind::Camera => "Camera",
        CommandKind::AudioListen => "Audio Listen",
        CommandKind::MessageBox => "Message Box",
        CommandKind::BalloonTip => "Balloon Tip",
        CommandKind::TextChat => "Text Chat",
        CommandKind::VoiceChat => "Voice Chat",
        CommandKind::OpenTextInNotepad => "Open Text In Notepad",
        CommandKind::ComputerInfo => "Computer Info",
        CommandKind::Clipboard => "Clipboard",
        CommandKind::Proxy => "Reverse Proxy",
        CommandKind::ExecuteFile => "Execute File",
        CommandKind::ExecuteCode => "Execute Code",
        CommandKind::ExecuteStaticCommand => "Execute Static Command",
        CommandKind::CreateTask => "Task Manager",
        CommandKind::CommandPreset => "Command Preset",
        CommandKind::PluginManager => "Plugin Manager",
        CommandKind::KillTargetProcess => "Kill Process",
    }
}

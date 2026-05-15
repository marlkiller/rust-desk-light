use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(crate) fn handle(payload: &str) -> String {
    let request = FileRequest::parse(payload);
    match request.action.as_str() {
        "list" => list_dir(request.path.as_deref()),
        "delete" => delete_path(required_path(&request)),
        "mkdir" => create_dir(
            required_path(&request),
            request.value.as_deref().unwrap_or(""),
        ),
        "rename" => rename_path(
            required_path(&request),
            request.value.as_deref().unwrap_or(""),
        ),
        "upload" => upload_file(
            required_path(&request),
            request.value.as_deref().unwrap_or(""),
        ),
        "download" => download_file(required_path(&request)),
        _ => file_error(
            current_dir_label(),
            &format!("unsupported file_manager action: {}", request.action),
        ),
    }
}

struct FileRequest {
    action: String,
    path: Option<String>,
    value: Option<String>,
}

impl FileRequest {
    fn parse(payload: &str) -> Self {
        let mut action = "list".to_string();
        let mut path = None;
        let mut value = None;
        for line in payload.lines() {
            if let Some(rest) = line.strip_prefix("action=") {
                action = rest.trim().to_ascii_lowercase();
            } else if let Some(rest) = line.strip_prefix("path=") {
                path = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("value=") {
                value = Some(rest.to_string());
            }
        }
        if payload.trim().is_empty() {
            action = "list".to_string();
        }
        Self {
            action,
            path,
            value,
        }
    }
}

fn required_path(request: &FileRequest) -> &str {
    request.path.as_deref().unwrap_or("")
}

fn list_dir(path: Option<&str>) -> String {
    let dir = resolve_path(path.unwrap_or(""));
    let display_dir = dir.display().to_string();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => return file_error(display_dir, &format!("list failed: {error}")),
    };

    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let kind = if metadata.is_dir() { "dir" } else { "file" };
        let size = if metadata.is_file() {
            metadata.len().to_string()
        } else {
            String::new()
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_default();
        let name = entry
            .file_name()
            .to_string_lossy()
            .replace(['\t', '\n'], " ");
        rows.push(format!("{kind}\t{name}\t{size}\t{modified}"));
    }
    rows.sort_by(|left, right| {
        let left_dir = left.starts_with("dir\t");
        let right_dir = right.starts_with("dir\t");
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase()))
    });

    let mut response = format!("ok\ncwd={display_dir}\nentries=kind\tname\tsize\tmodified");
    for row in rows {
        response.push('\n');
        response.push_str(&row);
    }
    response
}

fn delete_path(path: &str) -> String {
    let path = resolve_path(path);
    let cwd = parent_or_current(&path);
    let result = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path),
        Ok(_) => fs::remove_file(&path),
        Err(error) => {
            return file_error(
                cwd.display().to_string(),
                &format!("delete failed: {error}"),
            )
        }
    };
    match result {
        Ok(()) => list_dir(Some(&cwd.display().to_string())),
        Err(error) => file_error(
            cwd.display().to_string(),
            &format!("delete failed: {error}"),
        ),
    }
}

fn create_dir(path: &str, name: &str) -> String {
    let base = resolve_path(path);
    let cwd = if base.is_dir() {
        base
    } else {
        parent_or_current(&base)
    };
    let name = name.trim();
    if name.is_empty() || name.contains(['\\', '/', '\n', '\t']) {
        return file_error(
            cwd.display().to_string(),
            "mkdir failed: invalid folder name",
        );
    }
    match fs::create_dir_all(cwd.join(name)) {
        Ok(()) => list_dir(Some(&cwd.display().to_string())),
        Err(error) => file_error(cwd.display().to_string(), &format!("mkdir failed: {error}")),
    }
}

fn rename_path(path: &str, new_name: &str) -> String {
    let path = resolve_path(path);
    let cwd = parent_or_current(&path);
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name.contains(['\\', '/', '\n', '\t']) {
        return file_error(cwd.display().to_string(), "rename failed: invalid new name");
    }
    let target = cwd.join(new_name);
    match fs::rename(&path, &target) {
        Ok(()) => list_dir(Some(&cwd.display().to_string())),
        Err(error) => file_error(
            cwd.display().to_string(),
            &format!("rename failed: {error}"),
        ),
    }
}

fn upload_file(path: &str, hex: &str) -> String {
    let path = resolve_path(path);
    let cwd = parent_or_current(&path);
    let bytes = match decode_hex(hex.trim()) {
        Ok(bytes) => bytes,
        Err(error) => {
            return file_error(
                cwd.display().to_string(),
                &format!("upload failed: {error}"),
            )
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return file_error(
                cwd.display().to_string(),
                &format!("upload failed: {error}"),
            );
        }
    }
    match fs::write(&path, bytes) {
        Ok(()) => list_dir(Some(&cwd.display().to_string())),
        Err(error) => file_error(
            cwd.display().to_string(),
            &format!("upload failed: {error}"),
        ),
    }
}

fn download_file(path: &str) -> String {
    let path = resolve_path(path);
    let cwd = parent_or_current(&path);
    match fs::read(&path) {
        Ok(bytes) => format!(
            "download\ncwd={}\npath={}\nvalue={}",
            cwd.display(),
            path.display(),
            encode_hex(&bytes)
        ),
        Err(error) => file_error(
            cwd.display().to_string(),
            &format!("download failed: {error}"),
        ),
    }
}

fn file_error(cwd: String, message: &str) -> String {
    format!("error\ncwd={cwd}\nmessage={message}")
}

fn resolve_path(path: &str) -> PathBuf {
    let path = path.trim();
    if path.is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn parent_or_current(path: &Path) -> PathBuf {
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn current_dir_label() -> String {
    std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| ".".to_string())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("invalid hex length".to_string());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks(2) {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex data".to_string()),
    }
}

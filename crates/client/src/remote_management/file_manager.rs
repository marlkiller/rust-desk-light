use rdl_protocol::{FileTransferAction, FileTransferDirection, Message};
use std::collections::HashMap;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::UNIX_EPOCH;

const FILE_TRANSFER_CHUNK_SIZE: usize = 512 * 1024;

static UPLOAD_TRANSFERS: OnceLock<Mutex<HashMap<u64, UploadTransferState>>> = OnceLock::new();
static DOWNLOAD_TRANSFERS: OnceLock<Mutex<HashMap<u64, Arc<AtomicBool>>>> = OnceLock::new();

struct UploadTransferState {
    root: PathBuf,
    total_bytes: u64,
    transferred_bytes: u64,
    cancelled: Arc<AtomicBool>,
}

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

pub(crate) fn handle_transfer<F>(message: Message, mut send: F) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let Message::FileTransfer {
        target_id,
        transfer_id,
        direction,
        action,
        path,
        relative_path,
        total_bytes,
        transferred_bytes: _,
        file_size,
        offset,
        bytes,
        message,
    } = message
    else {
        return Ok(());
    };

    match (direction, action) {
        (FileTransferDirection::Download, FileTransferAction::Start) => {
            run_download_transfer(target_id, transfer_id, path, &mut send)
        }
        (FileTransferDirection::Download, FileTransferAction::Cancel) => {
            cancel_download_transfer(&target_id, transfer_id, path, &mut send)
        }
        (FileTransferDirection::Upload, FileTransferAction::Start) => {
            start_upload_transfer(&target_id, transfer_id, path, total_bytes, &mut send)
        }
        (FileTransferDirection::Upload, FileTransferAction::Directory) => {
            write_upload_directory(&target_id, transfer_id, path, relative_path, &mut send)
        }
        (FileTransferDirection::Upload, FileTransferAction::Chunk) => write_upload_chunk(
            &target_id,
            transfer_id,
            path,
            relative_path,
            total_bytes,
            file_size,
            offset,
            bytes,
            &mut send,
        ),
        (FileTransferDirection::Upload, FileTransferAction::Finish) => {
            finish_upload_transfer(&target_id, transfer_id, path, &mut send)
        }
        (FileTransferDirection::Upload, FileTransferAction::Cancel) => {
            cancel_upload_transfer(&target_id, transfer_id, path, &mut send)
        }
        _ => send(transfer_error(
            target_id,
            transfer_id,
            direction,
            path,
            relative_path,
            format!(
                "unsupported file transfer action: {} {} {}",
                direction.as_str(),
                action.as_str(),
                message
            ),
        )),
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

fn run_download_transfer<F>(
    client_id: String,
    transfer_id: u64,
    path: String,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let root = resolve_path(&path);
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut transfers) = download_transfers().lock() {
        transfers.insert(transfer_id, cancel.clone());
    }
    eprintln!(
        "debug event=client_file_download_start client={} id={} path={} root={}",
        client_id,
        transfer_id,
        sanitize_log_value(&path),
        sanitize_log_value(&root.display().to_string())
    );

    let result =
        send_download_contents(&client_id, transfer_id, &path, &root, cancel.clone(), send);
    if let Ok(mut transfers) = download_transfers().lock() {
        transfers.remove(&transfer_id);
    }
    match &result {
        Ok(()) => eprintln!(
            "debug event=client_file_download_end client={} id={} result=ok",
            client_id, transfer_id
        ),
        Err(error) => eprintln!(
            "debug event=client_file_download_end client={} id={} result=error error={}",
            client_id, transfer_id, error
        ),
    }
    result
}

fn send_download_contents<F>(
    client_id: &str,
    transfer_id: u64,
    requested_path: &str,
    root: &Path,
    cancel: Arc<AtomicBool>,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let base = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    eprintln!(
        "debug event=client_file_download_scan_start client={} id={} root={}",
        client_id,
        transfer_id,
        sanitize_log_value(&root.display().to_string())
    );
    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        FileTransferDirection::Download,
        FileTransferAction::Progress,
        requested_path.to_string(),
        String::new(),
        0,
        0,
        0,
        0,
        Vec::new(),
        "scanning download".to_string(),
    ))?;

    match collect_download_entries(root, Path::new(&base), &mut dirs, &mut files, &cancel) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::Interrupted => {
            eprintln!(
                "debug event=client_file_download_scan_cancelled client={} id={}",
                client_id, transfer_id
            );
            return send_cancelled(
                client_id,
                transfer_id,
                FileTransferDirection::Download,
                requested_path,
                0,
                0,
                send,
            );
        }
        Err(error) => return Err(error),
    }
    let total_bytes = files.iter().map(|file| file.size).sum::<u64>();
    let mut transferred_bytes = 0u64;
    eprintln!(
        "debug event=client_file_download_scan_done client={} id={} dirs={} files={} total_bytes={}",
        client_id,
        transfer_id,
        dirs.len(),
        files.len(),
        total_bytes
    );

    if cancel.load(Ordering::Relaxed) {
        eprintln!(
            "debug event=client_file_download_cancel_before_stream client={} id={} total_bytes={}",
            client_id, transfer_id, total_bytes
        );
        return send_cancelled(
            client_id,
            transfer_id,
            FileTransferDirection::Download,
            requested_path,
            total_bytes,
            transferred_bytes,
            send,
        );
    }

    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        FileTransferDirection::Download,
        FileTransferAction::Progress,
        requested_path.to_string(),
        String::new(),
        total_bytes,
        transferred_bytes,
        0,
        0,
        Vec::new(),
        "download started".to_string(),
    ))?;

    for dir in dirs {
        if cancel.load(Ordering::Relaxed) {
            eprintln!(
                "debug event=client_file_download_cancel_during_dirs client={} id={} transferred_bytes={} total_bytes={}",
                client_id, transfer_id, transferred_bytes, total_bytes
            );
            return send_cancelled(
                client_id,
                transfer_id,
                FileTransferDirection::Download,
                requested_path,
                total_bytes,
                transferred_bytes,
                send,
            );
        }
        send(transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Download,
            FileTransferAction::Directory,
            requested_path.to_string(),
            dir.to_string_lossy().to_string(),
            total_bytes,
            transferred_bytes,
            0,
            0,
            Vec::new(),
            String::new(),
        ))?;
    }

    let mut buffer = vec![0u8; FILE_TRANSFER_CHUNK_SIZE];
    for file in files {
        if cancel.load(Ordering::Relaxed) {
            eprintln!(
                "debug event=client_file_download_cancel_before_file client={} id={} transferred_bytes={} total_bytes={}",
                client_id, transfer_id, transferred_bytes, total_bytes
            );
            return send_cancelled(
                client_id,
                transfer_id,
                FileTransferDirection::Download,
                requested_path,
                total_bytes,
                transferred_bytes,
                send,
            );
        }
        let mut input = File::open(&file.path)?;
        let mut offset = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                eprintln!(
                    "debug event=client_file_download_cancel_during_file client={} id={} transferred_bytes={} total_bytes={} file={}",
                    client_id,
                    transfer_id,
                    transferred_bytes,
                    total_bytes,
                    sanitize_log_value(&file.relative.display().to_string())
                );
                return send_cancelled(
                    client_id,
                    transfer_id,
                    FileTransferDirection::Download,
                    requested_path,
                    total_bytes,
                    transferred_bytes,
                    send,
                );
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            transferred_bytes = transferred_bytes.saturating_add(count as u64);
            send(transfer_message(
                client_id.to_string(),
                transfer_id,
                FileTransferDirection::Download,
                FileTransferAction::Chunk,
                requested_path.to_string(),
                file.relative.to_string_lossy().to_string(),
                total_bytes,
                transferred_bytes,
                file.size,
                offset,
                buffer[..count].to_vec(),
                String::new(),
            ))?;
            offset = offset.saturating_add(count as u64);
        }
    }

    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        FileTransferDirection::Download,
        FileTransferAction::Complete,
        requested_path.to_string(),
        String::new(),
        total_bytes,
        transferred_bytes,
        0,
        0,
        Vec::new(),
        "download complete".to_string(),
    ))
}

fn cancel_download_transfer<F>(
    client_id: &str,
    transfer_id: u64,
    path: String,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let cancelled = download_transfers()
        .lock()
        .ok()
        .and_then(|transfers| transfers.get(&transfer_id).cloned());
    if let Some(cancelled) = cancelled {
        cancelled.store(true, Ordering::Relaxed);
        eprintln!(
            "debug event=client_file_download_cancel_request client={} id={} result=active path={}",
            client_id,
            transfer_id,
            sanitize_log_value(&path)
        );
        send(transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Download,
            FileTransferAction::Progress,
            path,
            String::new(),
            0,
            0,
            0,
            0,
            Vec::new(),
            "cancel requested".to_string(),
        ))
    } else {
        eprintln!(
            "debug event=client_file_download_cancel_request client={} id={} result=no_active path={}",
            client_id,
            transfer_id,
            sanitize_log_value(&path)
        );
        send(transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Download,
            FileTransferAction::Complete,
            path,
            String::new(),
            0,
            0,
            0,
            0,
            Vec::new(),
            "no active download".to_string(),
        ))
    }
}

fn start_upload_transfer<F>(
    client_id: &str,
    transfer_id: u64,
    path: String,
    total_bytes: u64,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let root = resolve_path(&path);
    let state = UploadTransferState {
        root,
        total_bytes,
        transferred_bytes: 0,
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    if let Ok(mut transfers) = upload_transfers().lock() {
        transfers.insert(transfer_id, state);
    }
    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        FileTransferDirection::Upload,
        FileTransferAction::Progress,
        path,
        String::new(),
        total_bytes,
        0,
        0,
        0,
        Vec::new(),
        "upload started".to_string(),
    ))
}

fn write_upload_directory<F>(
    client_id: &str,
    transfer_id: u64,
    path: String,
    relative_path: String,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let Some(target) = upload_target_path(transfer_id, &path, &relative_path) else {
        return send(transfer_error(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            path,
            relative_path,
            "upload directory failed: invalid path".to_string(),
        ));
    };
    match fs::create_dir_all(&target) {
        Ok(()) => Ok(()),
        Err(error) => send(transfer_error(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            path,
            relative_path,
            format!("upload directory failed: {error}"),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_upload_chunk<F>(
    client_id: &str,
    transfer_id: u64,
    path: String,
    relative_path: String,
    total_bytes: u64,
    file_size: u64,
    offset: u64,
    bytes: Vec<u8>,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let Some(target) = upload_target_path(transfer_id, &path, &relative_path) else {
        return send(transfer_error(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            path,
            relative_path,
            "upload chunk failed: invalid path".to_string(),
        ));
    };
    if upload_cancelled(transfer_id) {
        return send_cancelled(
            client_id,
            transfer_id,
            FileTransferDirection::Upload,
            &path,
            total_bytes,
            0,
            send,
        );
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(offset == 0)
        .open(&target)?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&bytes)?;
    if file_size > 0 && offset.saturating_add(bytes.len() as u64) >= file_size {
        let _ = file.set_len(file_size);
    }

    let transferred = update_upload_progress(transfer_id, bytes.len() as u64, total_bytes);
    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        FileTransferDirection::Upload,
        FileTransferAction::Progress,
        path,
        relative_path,
        total_bytes,
        transferred,
        file_size,
        offset.saturating_add(bytes.len() as u64),
        Vec::new(),
        String::new(),
    ))
}

fn finish_upload_transfer<F>(
    client_id: &str,
    transfer_id: u64,
    path: String,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let state = upload_transfers()
        .lock()
        .ok()
        .and_then(|mut transfers| transfers.remove(&transfer_id));
    let Some(state) = state else {
        return send(transfer_error(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            path,
            String::new(),
            "upload finish failed: no active upload".to_string(),
        ));
    };
    let cancelled = state.cancelled.load(Ordering::Relaxed);
    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        FileTransferDirection::Upload,
        FileTransferAction::Complete,
        path,
        String::new(),
        state.total_bytes,
        state.transferred_bytes,
        0,
        0,
        Vec::new(),
        if cancelled {
            "upload cancelled".to_string()
        } else {
            "upload complete".to_string()
        },
    ))
}

fn cancel_upload_transfer<F>(
    client_id: &str,
    transfer_id: u64,
    path: String,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    let state = upload_transfers()
        .lock()
        .ok()
        .and_then(|mut transfers| transfers.remove(&transfer_id));
    if let Some(state) = state {
        state.cancelled.store(true, Ordering::Relaxed);
    }
    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        FileTransferDirection::Upload,
        FileTransferAction::Complete,
        path,
        String::new(),
        0,
        0,
        0,
        0,
        Vec::new(),
        "upload cancelled".to_string(),
    ))
}

#[derive(Clone)]
struct DownloadFileEntry {
    path: PathBuf,
    relative: PathBuf,
    size: u64,
}

fn collect_download_entries(
    path: &Path,
    relative: &Path,
    dirs: &mut Vec<PathBuf>,
    files: &mut Vec<DownloadFileEntry>,
    cancel: &AtomicBool,
) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "download cancelled",
        ));
    }
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        dirs.push(relative.to_path_buf());
        let mut children = fs::read_dir(path)?.flatten().collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "download cancelled",
                ));
            }
            let child_name = child.file_name();
            let child_relative = relative.join(child_name);
            collect_download_entries(&child.path(), &child_relative, dirs, files, cancel)?;
        }
    } else {
        files.push(DownloadFileEntry {
            path: path.to_path_buf(),
            relative: relative.to_path_buf(),
            size: metadata.len(),
        });
    }
    Ok(())
}

fn upload_target_path(
    transfer_id: u64,
    fallback_root: &str,
    relative_path: &str,
) -> Option<PathBuf> {
    let root = upload_transfers()
        .lock()
        .ok()
        .and_then(|transfers| transfers.get(&transfer_id).map(|state| state.root.clone()))
        .unwrap_or_else(|| resolve_path(fallback_root));
    safe_join(&root, relative_path)
}

fn safe_join(root: &Path, relative_path: &str) -> Option<PathBuf> {
    let relative_path = relative_path.trim();
    if relative_path.is_empty() {
        return Some(root.to_path_buf());
    }
    let relative = Path::new(relative_path);
    if relative.is_absolute() {
        return None;
    }
    let mut path = root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => path.push(part),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(path)
}

fn update_upload_progress(transfer_id: u64, added: u64, total_bytes: u64) -> u64 {
    let Ok(mut transfers) = upload_transfers().lock() else {
        return added;
    };
    let Some(state) = transfers.get_mut(&transfer_id) else {
        return added;
    };
    if total_bytes > 0 {
        state.total_bytes = total_bytes;
    }
    state.transferred_bytes = state.transferred_bytes.saturating_add(added);
    state.transferred_bytes
}

fn upload_cancelled(transfer_id: u64) -> bool {
    upload_transfers()
        .lock()
        .ok()
        .and_then(|transfers| {
            transfers
                .get(&transfer_id)
                .map(|state| state.cancelled.load(Ordering::Relaxed))
        })
        .unwrap_or(false)
}

fn send_cancelled<F>(
    client_id: &str,
    transfer_id: u64,
    direction: FileTransferDirection,
    path: &str,
    total_bytes: u64,
    transferred_bytes: u64,
    send: &mut F,
) -> io::Result<()>
where
    F: FnMut(Message) -> io::Result<()>,
{
    send(transfer_message(
        client_id.to_string(),
        transfer_id,
        direction,
        FileTransferAction::Complete,
        path.to_string(),
        String::new(),
        total_bytes,
        transferred_bytes,
        0,
        0,
        Vec::new(),
        "transfer cancelled".to_string(),
    ))
}

fn transfer_error(
    target_id: String,
    transfer_id: u64,
    direction: FileTransferDirection,
    path: String,
    relative_path: String,
    message: String,
) -> Message {
    transfer_message(
        target_id,
        transfer_id,
        direction,
        FileTransferAction::Error,
        path,
        relative_path,
        0,
        0,
        0,
        0,
        Vec::new(),
        message,
    )
}

#[allow(clippy::too_many_arguments)]
fn transfer_message(
    target_id: String,
    transfer_id: u64,
    direction: FileTransferDirection,
    action: FileTransferAction,
    path: String,
    relative_path: String,
    total_bytes: u64,
    transferred_bytes: u64,
    file_size: u64,
    offset: u64,
    bytes: Vec<u8>,
    message: String,
) -> Message {
    Message::FileTransfer {
        target_id,
        transfer_id,
        direction,
        action,
        path,
        relative_path,
        total_bytes,
        transferred_bytes,
        file_size,
        offset,
        bytes,
        message,
    }
}

fn sanitize_log_value(value: &str) -> String {
    let mut value = value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    const MAX_LOG_VALUE_LEN: usize = 180;
    if value.len() > MAX_LOG_VALUE_LEN {
        value.truncate(MAX_LOG_VALUE_LEN);
        value.push_str("...");
    }
    value
}

fn upload_transfers() -> &'static Mutex<HashMap<u64, UploadTransferState>> {
    UPLOAD_TRANSFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn download_transfers() -> &'static Mutex<HashMap<u64, Arc<AtomicBool>>> {
    DOWNLOAD_TRANSFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn upload_transfer_writes_directory_chunks() {
        let root = test_dir("rdl-upload-transfer");
        let target = root.join("remote-root");
        let mut replies = Vec::new();

        handle_transfer(
            transfer_message(
                "client-a".to_string(),
                10,
                FileTransferDirection::Upload,
                FileTransferAction::Start,
                target.display().to_string(),
                String::new(),
                6,
                0,
                0,
                0,
                Vec::new(),
                String::new(),
            ),
            |message| {
                replies.push(message);
                Ok(())
            },
        )
        .unwrap();
        handle_transfer(
            transfer_message(
                "client-a".to_string(),
                10,
                FileTransferDirection::Upload,
                FileTransferAction::Directory,
                target.display().to_string(),
                "nested".to_string(),
                6,
                0,
                0,
                0,
                Vec::new(),
                String::new(),
            ),
            |message| {
                replies.push(message);
                Ok(())
            },
        )
        .unwrap();
        for (offset, bytes) in [(0, b"abc".to_vec()), (3, b"def".to_vec())] {
            handle_transfer(
                transfer_message(
                    "client-a".to_string(),
                    10,
                    FileTransferDirection::Upload,
                    FileTransferAction::Chunk,
                    target.display().to_string(),
                    "nested/file.bin".to_string(),
                    6,
                    offset + bytes.len() as u64,
                    6,
                    offset,
                    bytes,
                    String::new(),
                ),
                |message| {
                    replies.push(message);
                    Ok(())
                },
            )
            .unwrap();
        }
        handle_transfer(
            transfer_message(
                "client-a".to_string(),
                10,
                FileTransferDirection::Upload,
                FileTransferAction::Finish,
                target.display().to_string(),
                String::new(),
                6,
                6,
                0,
                0,
                Vec::new(),
                String::new(),
            ),
            |message| {
                replies.push(message);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(fs::read(target.join("nested/file.bin")).unwrap(), b"abcdef");
        assert!(replies.iter().any(|message| {
            matches!(
                message,
                Message::FileTransfer {
                    action: FileTransferAction::Complete,
                    direction: FileTransferDirection::Upload,
                    ..
                }
            )
        }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn download_transfer_streams_directory_entries_and_chunks() {
        let root = test_dir("rdl-download-transfer");
        let source = root.join("source-dir");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/file.txt"), b"hello").unwrap();
        let mut replies = Vec::new();

        handle_transfer(
            transfer_message(
                "client-b".to_string(),
                11,
                FileTransferDirection::Download,
                FileTransferAction::Start,
                source.display().to_string(),
                String::new(),
                0,
                0,
                0,
                0,
                Vec::new(),
                String::new(),
            ),
            |message| {
                replies.push(message);
                Ok(())
            },
        )
        .unwrap();

        assert!(matches!(
            replies.first(),
            Some(Message::FileTransfer {
                action: FileTransferAction::Progress,
                direction: FileTransferDirection::Download,
                message,
                ..
            }) if message == "scanning download"
        ));
        assert!(replies.iter().any(|message| {
            matches!(
                message,
                Message::FileTransfer {
                    action: FileTransferAction::Progress,
                    direction: FileTransferDirection::Download,
                    total_bytes: 5,
                    message,
                    ..
                } if message == "download started"
            )
        }));
        assert!(replies.iter().any(|message| {
            matches!(
                message,
                Message::FileTransfer {
                    action: FileTransferAction::Directory,
                    direction: FileTransferDirection::Download,
                    relative_path,
                    ..
                } if relative_path.ends_with("nested")
            )
        }));
        assert!(replies.iter().any(|message| {
            matches!(
                message,
                Message::FileTransfer {
                    action: FileTransferAction::Chunk,
                    direction: FileTransferDirection::Download,
                    relative_path,
                    bytes,
                    ..
                } if relative_path.ends_with("nested/file.txt") && bytes == b"hello"
            )
        }));
        assert!(replies.iter().any(|message| {
            matches!(
                message,
                Message::FileTransfer {
                    action: FileTransferAction::Complete,
                    direction: FileTransferDirection::Download,
                    ..
                }
            )
        }));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn download_entry_collection_can_be_cancelled() {
        let root = test_dir("rdl-download-cancel-scan");
        let source = root.join("source-dir");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/file.txt"), b"hello").unwrap();
        let cancel = AtomicBool::new(true);
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        let error = collect_download_entries(
            &source,
            Path::new("source-dir"),
            &mut dirs,
            &mut files,
            &cancel,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(dirs.is_empty());
        assert!(files.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    fn test_dir(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}

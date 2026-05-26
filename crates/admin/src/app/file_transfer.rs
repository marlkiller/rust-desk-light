use super::event::AdminInput;
use rdl_protocol::{
    file_transfer_message, FileTransferAction, FileTransferDirection, Message,
};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{SyncSender, TrySendError},
    Arc,
};
use std::thread;
use std::time::Duration;

const FILE_TRANSFER_CHUNK_SIZE: usize = 512 * 1024;

pub(super) fn should_log_admin_file_transfer_event(
    action: FileTransferAction,
    message: &str,
) -> bool {
    matches!(
        action,
        FileTransferAction::Start
            | FileTransferAction::Cancel
            | FileTransferAction::Complete
            | FileTransferAction::Error
    ) || !message.trim().is_empty()
}

pub(super) fn run_file_upload_transfer(
    input_tx: &SyncSender<AdminInput>,
    client_id: &str,
    transfer_id: u64,
    local_path: &str,
    remote_path: &str,
    cancel_flag: Arc<AtomicBool>,
) -> io::Result<()> {
    let source = PathBuf::from(local_path);
    let metadata = fs::metadata(&source)?;
    let total_bytes = compute_total_bytes(&source, &metadata)?;
    let mut transferred_bytes = 0u64;

    send_file_transfer_input_cancelable(
        input_tx,
        file_transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            FileTransferAction::Start,
            remote_path.to_string(),
            String::new(),
            total_bytes,
            0,
            0,
            0,
            Vec::new(),
            "upload started".to_string(),
        ),
        &cancel_flag,
    )?;

    let mut file_entries = Vec::new();
    walk_upload_entries(&source, Path::new(""), &metadata, &mut |abs, rel, is_dir, size| {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if is_dir {
            send_file_transfer_input_cancelable(
                input_tx,
                file_transfer_message(
                    client_id.to_string(),
                    transfer_id,
                    FileTransferDirection::Upload,
                    FileTransferAction::Directory,
                    remote_path.to_string(),
                    protocol_relative_path(rel),
                    total_bytes,
                    transferred_bytes,
                    0,
                    0,
                    Vec::new(),
                    String::new(),
                ),
                &cancel_flag,
            )?;
        } else {
            file_entries.push((abs.to_path_buf(), rel.to_path_buf(), size));
        }
        Ok(())
    })?;

    let mut buffer = vec![0u8; FILE_TRANSFER_CHUNK_SIZE];
    for (abs_path, rel_path, file_size) in &file_entries {
        if cancel_flag.load(Ordering::Relaxed) {
            return send_upload_cancel(input_tx, client_id, transfer_id, remote_path);
        }
        let mut input = File::open(abs_path)?;
        let mut offset = 0u64;
        let relative = protocol_relative_path(rel_path);
        loop {
            if cancel_flag.load(Ordering::Relaxed) {
                return send_upload_cancel(input_tx, client_id, transfer_id, remote_path);
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            transferred_bytes = transferred_bytes.saturating_add(count as u64);
            send_file_transfer_input_cancelable(
                input_tx,
                file_transfer_message(
                    client_id.to_string(),
                    transfer_id,
                    FileTransferDirection::Upload,
                    FileTransferAction::Chunk,
                    remote_path.to_string(),
                    relative.clone(),
                    total_bytes,
                    transferred_bytes,
                    *file_size,
                    offset,
                    buffer[..count].to_vec(),
                    String::new(),
                ),
                &cancel_flag,
            )?;
            offset = offset.saturating_add(count as u64);
        }
    }

    send_file_transfer_input_cancelable(
        input_tx,
        file_transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            FileTransferAction::Finish,
            remote_path.to_string(),
            String::new(),
            total_bytes,
            transferred_bytes,
            0,
            0,
            Vec::new(),
            "upload finished".to_string(),
        ),
        &cancel_flag,
    )
}

pub(super) fn send_upload_cancel(
    input_tx: &SyncSender<AdminInput>,
    client_id: &str,
    transfer_id: u64,
    remote_path: &str,
) -> io::Result<()> {
    send_file_transfer_input(
        input_tx,
        file_transfer_message(
            client_id.to_string(),
            transfer_id,
            FileTransferDirection::Upload,
            FileTransferAction::Cancel,
            remote_path.to_string(),
            String::new(),
            0,
            0,
            0,
            0,
            Vec::new(),
            "upload cancelled".to_string(),
        ),
    )
}

fn walk_upload_entries<F>(
    path: &Path,
    relative: &Path,
    metadata: &fs::Metadata,
    f: &mut F,
) -> io::Result<()>
where
    F: FnMut(&Path, &Path, bool, u64) -> io::Result<()>,
{
    if metadata.is_dir() {
        f(path, relative, true, 0)?;
        let mut children = fs::read_dir(path)?.flatten().collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let child_metadata = child.metadata()?;
            let child_relative = relative.join(child.file_name());
            walk_upload_entries(&child.path(), &child_relative, &child_metadata, f)?;
        }
    } else {
        f(path, relative, false, metadata.len())?;
    }
    Ok(())
}

fn compute_total_bytes(path: &Path, metadata: &fs::Metadata) -> io::Result<u64> {
    if metadata.is_dir() {
        let mut total = 0u64;
        fn walk_size(path: &Path, total: &mut u64) -> io::Result<()> {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let meta = entry.metadata()?;
                if meta.is_dir() {
                    walk_size(&entry.path(), total)?;
                } else {
                    *total = total.saturating_add(meta.len());
                }
            }
            Ok(())
        }
        walk_size(path, &mut total)?;
        Ok(total)
    } else {
        Ok(metadata.len())
    }
}

fn protocol_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn send_file_transfer_input(
    input_tx: &SyncSender<AdminInput>,
    message: Message,
) -> io::Result<()> {
    input_tx
        .send(AdminInput::FileTransfer(message))
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
}

fn send_file_transfer_input_cancelable(
    input_tx: &SyncSender<AdminInput>,
    message: Message,
    cancel_flag: &AtomicBool,
) -> io::Result<()> {
    let mut input = AdminInput::FileTransfer(message);
    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "file upload cancelled",
            ));
        }
        match input_tx.try_send(input) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                input = returned;
                thread::sleep(Duration::from_millis(5));
            }
            Err(TrySendError::Disconnected(returned)) => {
                drop(returned);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "admin input queue disconnected",
                ));
            }
        }
    }
}

use crate::realtime_video::RealtimeVideoReceiver;
use rdl_protocol::{write_envelope, FileTransferAction, FileTransferDirection, Message, Role};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

const BULK_POLL_MS: u64 = 2;

pub(crate) fn writer_loop(
    peer_id: usize,
    mut writer: TcpStream,
    high_rx: Receiver<Message>,
    video_rx: RealtimeVideoReceiver<Message>,
    bulk_rx: Receiver<Message>,
) {
    let mut next_message_id = 1u64;
    let mut high_open = true;
    let mut video_open = true;
    let mut bulk_open = true;
    while high_open || video_open || bulk_open {
        loop {
            match high_rx.try_recv() {
                Ok(message) => {
                    if !write_server_message(peer_id, &mut writer, &mut next_message_id, message) {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    high_open = false;
                    break;
                }
            }
        }

        loop {
            match video_rx.try_recv() {
                Ok(message) => {
                    if !write_server_message(peer_id, &mut writer, &mut next_message_id, message) {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    video_open = false;
                    break;
                }
            }
        }

        if !bulk_open {
            thread::sleep(Duration::from_millis(BULK_POLL_MS));
            continue;
        }

        match bulk_rx.recv_timeout(Duration::from_millis(BULK_POLL_MS)) {
            Ok(message) => {
                if !write_server_message(peer_id, &mut writer, &mut next_message_id, message) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if !high_open && !video_open && !bulk_open {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => bulk_open = false,
        }
    }
}

fn write_server_message(
    peer_id: usize,
    writer: &mut TcpStream,
    next_message_id: &mut u64,
    message: Message,
) -> bool {
    let result = write_envelope(writer, Role::Server, *next_message_id, None, message);
    *next_message_id = next_message_id.saturating_add(1);
    if let Err(error) = result {
        eprintln!("peer {peer_id} write failed: {error}");
        return false;
    }
    true
}

pub(crate) fn message_is_video_realtime(message: &Message) -> bool {
    matches!(message, Message::VideoFrame { .. })
}

pub(crate) fn message_is_bulk(message: &Message) -> bool {
    matches!(
        message,
        Message::FileTransfer {
            direction: FileTransferDirection::Download,
            action: FileTransferAction::Directory
                | FileTransferAction::Chunk
                | FileTransferAction::Complete,
            ..
        } | Message::FileTransfer {
            direction: FileTransferDirection::Upload,
            action: FileTransferAction::Directory
                | FileTransferAction::Chunk
                | FileTransferAction::Finish,
            ..
        } | Message::ProxyData { .. }
    )
}

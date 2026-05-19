use crate::live_control::realtime_video::RealtimeVideoReceiver;
use crate::payload::sanitize_log_value;
use rdl_protocol::{
    write_envelope_with_token, FileTransferAction, FileTransferDirection, Message, Role,
};
use std::io;
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::Duration;

const BULK_POLL_MS: u64 = 2;

pub(crate) struct ClientOutbound {
    pub(crate) session_token: String,
    pub(crate) message: Message,
}

pub(crate) fn queue_message(
    out_tx: &SyncSender<ClientOutbound>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    out_tx
        .send(ClientOutbound {
            session_token: session_token.to_string(),
            message,
        })
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string()))
}

pub(crate) fn queue_file_transfer_reply(
    out_tx: &SyncSender<ClientOutbound>,
    bulk_out_tx: &SyncSender<ClientOutbound>,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    if file_transfer_reply_is_bulk(&message) {
        log_client_file_transfer_queue("bulk", &message);
        queue_message(bulk_out_tx, session_token, message)
    } else {
        log_client_file_transfer_queue("high", &message);
        queue_message(out_tx, session_token, message)
    }
}

fn file_transfer_reply_is_bulk(message: &Message) -> bool {
    matches!(
        message,
        Message::FileTransfer {
            direction: FileTransferDirection::Download,
            action: FileTransferAction::Directory
                | FileTransferAction::Chunk
                | FileTransferAction::Complete,
            ..
        }
    )
}

fn log_client_file_transfer_queue(queue: &str, message: &Message) {
    let Message::FileTransfer {
        target_id,
        transfer_id,
        direction,
        action,
        total_bytes,
        transferred_bytes,
        message,
        ..
    } = message
    else {
        return;
    };
    if matches!(
        action,
        FileTransferAction::Directory | FileTransferAction::Chunk
    ) && message.trim().is_empty()
    {
        return;
    }
    debug_log!(
        "debug event=client_file_transfer_queue queue={} client={} id={} direction={} action={} bytes={}/{} message={}",
        queue,
        target_id,
        transfer_id,
        direction.as_str(),
        action.as_str(),
        transferred_bytes,
        total_bytes,
        sanitize_log_value(message)
    );
}

pub(crate) fn writer_loop(
    mut writer: TcpStream,
    out_rx: Receiver<ClientOutbound>,
    video_rx: RealtimeVideoReceiver<ClientOutbound>,
    bulk_out_rx: Receiver<ClientOutbound>,
) {
    let mut next_message_id = 1u64;
    let mut out_open = true;
    let mut video_open = true;
    let mut bulk_open = true;
    while out_open || video_open || bulk_open {
        loop {
            match out_rx.try_recv() {
                Ok(outbound) => {
                    if !write_client_outbound(&mut writer, &mut next_message_id, outbound) {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    out_open = false;
                    break;
                }
            }
        }

        if video_open {
            loop {
                match video_rx.try_recv() {
                    Ok(outbound) => {
                        if !write_client_outbound(&mut writer, &mut next_message_id, outbound) {
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
        }

        if !bulk_open {
            thread::sleep(Duration::from_millis(BULK_POLL_MS));
            continue;
        }

        match bulk_out_rx.recv_timeout(Duration::from_millis(BULK_POLL_MS)) {
            Ok(outbound) => {
                if !write_client_outbound(&mut writer, &mut next_message_id, outbound) {
                    return;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !out_open && !video_open && !bulk_open {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bulk_open = false;
            }
        }
    }
}

fn write_client_outbound(
    writer: &mut TcpStream,
    next_message_id: &mut u64,
    outbound: ClientOutbound,
) -> bool {
    let fallback = command_ack_send_failure(&outbound.message);
    if let Err(error) = send(
        writer,
        next_message_id,
        &outbound.session_token,
        outbound.message,
    ) {
        if let Some(message) = fallback {
            eprintln!("client write failed, sending command error ack: {error}");
            if let Err(fallback_error) = send(
                writer,
                next_message_id,
                &outbound.session_token,
                message(error),
            ) {
                eprintln!("client fallback write failed: {fallback_error}");
                return false;
            }
            return true;
        }
        eprintln!("client write failed: {error}");
        return false;
    }
    true
}

fn command_ack_send_failure(
    message: &Message,
) -> Option<Box<dyn FnOnce(io::Error) -> Message + Send + 'static>> {
    let Message::CommandAck {
        client_id,
        command,
        accepted: _,
        detail: _,
    } = message
    else {
        return None;
    };
    let client_id = client_id.clone();
    let command = command.clone();
    Some(Box::new(move |error| Message::CommandAck {
        client_id,
        command,
        accepted: false,
        detail: format!("client failed to send command result: {error}"),
    }))
}

fn send(
    writer: &mut TcpStream,
    next_message_id: &mut u64,
    session_token: &str,
    message: Message,
) -> io::Result<()> {
    let result = write_envelope_with_token(
        writer,
        Role::Client,
        *next_message_id,
        None,
        session_token,
        message,
    );
    *next_message_id = next_message_id.saturating_add(1);
    result
}

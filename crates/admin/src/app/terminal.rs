use super::{
    event::{AdminEvent, AdminEventSink, AdminInput},
    network::admin_network_loop,
    ADMIN_INPUT_QUEUE_CAPACITY,
};
use crate::runtime::Config;
use rdl_protocol::{CommandKind, Message};
use std::collections::HashSet;
use std::io::{self, BufRead};
use std::sync::{
    mpsc::{self, SyncSender},
    Arc, Mutex,
};
use std::thread;

pub(super) fn run_terminal(config: Config) -> io::Result<()> {
    println!(
        "rust-desk-light admin {} terminal mode, server={}:{}",
        rdl_version::display_version(),
        config.ip,
        config.port
    );

    let (input_tx, input_rx) = mpsc::sync_channel(ADMIN_INPUT_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel();
    let ignored_file_transfers = Arc::new(Mutex::new(HashSet::new()));
    thread::spawn(move || {
        let event_sink = AdminEventSink::new(event_tx, None);
        if let Err(error) =
            admin_network_loop(config, input_rx, event_sink.clone(), ignored_file_transfers)
        {
            event_sink.send(AdminEvent::Log(format!("network stopped: {error}")));
        }
    });
    thread::spawn(move || terminal_input_loop(input_tx));

    for event in event_rx {
        match event {
            AdminEvent::Clients(clients) => {
                println!("online clients: {}", clients.len());
                for client in clients {
                    println!(
                        "- {} | fp={} | host={} os={} user={} gui={}",
                        client.id,
                        client.fingerprint,
                        client.hostname,
                        client.os,
                        client.username,
                        client.gui_available
                    );
                }
            }
            AdminEvent::Ack {
                client_id,
                command,
                accepted,
                detail,
            } => println!(
                "ack client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                detail
            ),
            AdminEvent::CommandOutput {
                client_id,
                command,
                stream,
                chunk,
                finished,
                success,
                ..
            } => println!(
                "command_output client={} command={} stream={} finished={} success={} chunk={}",
                client_id,
                command.as_str(),
                stream.as_str(),
                finished,
                success,
                chunk
            ),
            AdminEvent::DesktopFrame { client_id, payload } => {
                println!("desktop_frame client={} bytes={}", client_id, payload.len());
            }
            AdminEvent::DecodedDesktopFrame { client_id, result } => match result {
                Ok(_) => println!("decoded_desktop_frame client={client_id}"),
                Err(error) => println!("decoded_desktop_frame client={client_id} error={error}"),
            },
            AdminEvent::DecodedCameraFrame { client_id, result } => match result {
                Ok(_) => println!("decoded_camera_frame client={client_id}"),
                Err(error) => println!("decoded_camera_frame client={client_id} error={error}"),
            },
            AdminEvent::VideoFrame {
                client_id,
                source,
                bytes,
                ..
            } => {
                println!(
                    "video_frame client={} source={} bytes={}",
                    client_id,
                    source.as_str(),
                    bytes.len()
                );
            }
            AdminEvent::AudioFrame {
                client_id,
                source,
                bytes,
                sample_rate,
                channels,
                ..
            } => {
                println!(
                    "audio_frame client={} source={} rate={} channels={} bytes={}",
                    client_id,
                    source.as_str(),
                    sample_rate,
                    channels,
                    bytes.len()
                );
            }
            AdminEvent::FileTransfer(message) => {
                if let Message::FileTransfer {
                    target_id,
                    transfer_id,
                    direction,
                    action,
                    total_bytes,
                    transferred_bytes,
                    message,
                    ..
                } = message
                {
                    println!(
                        "file_transfer client={} id={} direction={} action={} bytes={}/{} message={}",
                        target_id,
                        transfer_id,
                        direction.as_str(),
                        action.as_str(),
                        transferred_bytes,
                        total_bytes,
                        message
                    );
                }
            }
            AdminEvent::Log(line) => println!("{line}"),
            AdminEvent::Connected => println!("connected"),
            AdminEvent::Disconnected => println!("disconnected"),
        }
    }

    Ok(())
}

fn terminal_input_loop(input_tx: SyncSender<AdminInput>) {
    println!("commands:");
    println!("  list");
    println!("  cmd <client-id> <command-kind> [payload]");
    println!("  quit");
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed == "quit" || trimmed == "exit" {
            thread::sleep(std::time::Duration::from_millis(1200));
            let _ = input_tx.send(AdminInput::Quit);
            break;
        }
        if trimmed == "list" {
            let _ = input_tx.send(AdminInput::List);
            continue;
        }
        let mut parts = trimmed.splitn(3, ' ');
        if let (Some("cmd"), Some(target_id), Some(command)) =
            (parts.next(), parts.next(), parts.next())
        {
            let (command_name, payload) = command
                .split_once(' ')
                .map(|(name, payload)| (name, payload.to_string()))
                .unwrap_or((command, String::new()));
            if let Some(command) = CommandKind::parse(command_name) {
                let _ = input_tx.send(AdminInput::Command {
                    target_id: target_id.to_string(),
                    command,
                    payload,
                });
            }
        }
    }
}

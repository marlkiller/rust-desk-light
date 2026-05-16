use rdl_protocol::{
    now_epoch_ms, read_envelope, write_envelope, AudioSource, ClientInfo, FileTransferAction,
    FileTransferDirection, Message, Role,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

const HEARTBEAT_INTERVAL_MS: u128 = 10_000;
const STALE_PEER_MS: u128 = 45_000;
const MAINTENANCE_TICK_MS: u64 = 100;
const WRITER_BULK_POLL_MS: u64 = 2;
const WRITER_AUDIO_QUEUE_CAPACITY: usize = 24;
const WRITER_VIDEO_QUEUE_CAPACITY: usize = 4;

#[derive(Debug)]
enum ServerEvent {
    Connected {
        peer_id: usize,
        sender: PeerSender,
        peer_addr: String,
    },
    Registered {
        peer_id: usize,
        role: Role,
        identity: String,
        fingerprint: String,
        info: Option<ClientInfo>,
    },
    Message {
        peer_id: usize,
        session_token: String,
        message: Message,
    },
    Disconnected {
        peer_id: usize,
    },
}

#[derive(Clone, Debug)]
struct PeerSender {
    high: Sender<Message>,
    audio: SyncSender<Message>,
    video: SyncSender<Message>,
    bulk: Sender<Message>,
}

impl PeerSender {
    fn send(&self, message: Message) -> Result<(), mpsc::SendError<Message>> {
        if server_message_is_audio_realtime(&message) {
            try_send_lossy(&self.audio, message)
        } else if server_message_is_video_realtime(&message) {
            try_send_lossy(&self.video, message)
        } else if server_message_is_bulk(&message) {
            self.bulk.send(message)
        } else {
            self.high.send(message)
        }
    }
}

fn try_send_lossy(
    tx: &SyncSender<Message>,
    message: Message,
) -> Result<(), mpsc::SendError<Message>> {
    match tx.try_send(message) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(message)) => Err(mpsc::SendError(message)),
    }
}

#[derive(Clone)]
struct Peer {
    role: Option<Role>,
    identity: Option<String>,
    fingerprint: Option<String>,
    session_token: Option<String>,
    sender: PeerSender,
    client_info: Option<ClientInfo>,
    peer_addr: String,
    last_seen_epoch_ms: u128,
    last_heartbeat_epoch_ms: u128,
    last_ping_epoch_ms: u128,
}

type FileTransferKey = (String, u64, &'static str);

fn main() -> io::Result<()> {
    let config = Config::from_env();
    let bind_addr = format!("{}:{}", config.ip, config.port);
    let listener = TcpListener::bind(&bind_addr)?;
    let (events_tx, events_rx) = mpsc::channel();

    println!(
        "rust-desk-light server listening on {bind_addr} version={}",
        rdl_version::display_version()
    );
    thread::spawn(move || accept_loop(listener, events_tx));
    event_loop(events_rx);
    Ok(())
}

fn accept_loop(listener: TcpListener, events_tx: Sender<ServerEvent>) {
    let mut next_peer_id = 1usize;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let peer_id = next_peer_id;
                next_peer_id += 1;
                let events_tx = events_tx.clone();
                thread::spawn(move || handle_peer(peer_id, stream, events_tx));
            }
            Err(error) => eprintln!("accept failed: {error}"),
        }
    }
}

fn handle_peer(peer_id: usize, stream: TcpStream, events_tx: Sender<ServerEvent>) {
    let peer_addr = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    if let Err(error) = stream.set_nodelay(true) {
        eprintln!("peer {peer_id} set TCP_NODELAY failed: {error}");
    }
    let (high_tx, high_rx) = mpsc::channel::<Message>();
    let (audio_tx, audio_rx) = mpsc::sync_channel::<Message>(WRITER_AUDIO_QUEUE_CAPACITY);
    let (video_tx, video_rx) = mpsc::sync_channel::<Message>(WRITER_VIDEO_QUEUE_CAPACITY);
    let (bulk_tx, bulk_rx) = mpsc::channel::<Message>();
    if events_tx
        .send(ServerEvent::Connected {
            peer_id,
            sender: PeerSender {
                high: high_tx,
                audio: audio_tx,
                video: video_tx,
                bulk: bulk_tx,
            },
            peer_addr,
        })
        .is_err()
    {
        return;
    }

    let writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(error) => {
            eprintln!("peer {peer_id} clone failed: {error}");
            return;
        }
    };

    thread::spawn(move || writer_loop(peer_id, writer, high_rx, audio_rx, video_rx, bulk_rx));

    let mut reader = stream;
    loop {
        let envelope = match read_envelope(&mut reader) {
            Ok(envelope) => envelope,
            Err(error) => {
                eprintln!("peer {peer_id} read failed: {error}");
                break;
            }
        };

        match envelope.message {
            Message::Hello {
                role,
                id,
                fingerprint,
                hostname,
                os,
                username,
                gui_available,
            } => {
                let info = if role == Role::Client {
                    Some(ClientInfo {
                        id: id.clone(),
                        fingerprint: fingerprint.clone(),
                        peer_addr: String::new(),
                        hostname,
                        os,
                        username,
                        gui_available,
                        started_at_epoch_ms: now_epoch_ms(),
                        last_seen_epoch_ms: now_epoch_ms(),
                    })
                } else {
                    None
                };
                let _ = events_tx.send(ServerEvent::Registered {
                    peer_id,
                    role,
                    identity: id,
                    fingerprint,
                    info,
                });
            }
            message => {
                let _ = events_tx.send(ServerEvent::Message {
                    peer_id,
                    session_token: envelope.session_token,
                    message,
                });
            }
        }
    }

    let _ = events_tx.send(ServerEvent::Disconnected { peer_id });
}

fn writer_loop(
    peer_id: usize,
    mut writer: TcpStream,
    high_rx: Receiver<Message>,
    audio_rx: Receiver<Message>,
    video_rx: Receiver<Message>,
    bulk_rx: Receiver<Message>,
) {
    let mut next_message_id = 1u64;
    let mut high_open = true;
    let mut audio_open = true;
    let mut video_open = true;
    let mut bulk_open = true;
    while high_open || audio_open || video_open || bulk_open {
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
            match audio_rx.try_recv() {
                Ok(message) => {
                    if !write_server_message(peer_id, &mut writer, &mut next_message_id, message) {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    audio_open = false;
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
            thread::sleep(Duration::from_millis(WRITER_BULK_POLL_MS));
            continue;
        }

        match bulk_rx.recv_timeout(Duration::from_millis(WRITER_BULK_POLL_MS)) {
            Ok(message) => {
                if !write_server_message(peer_id, &mut writer, &mut next_message_id, message) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if !high_open && !audio_open && !video_open && !bulk_open {
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

fn server_message_is_audio_realtime(message: &Message) -> bool {
    matches!(message, Message::AudioFrame { .. })
}

fn server_message_is_video_realtime(message: &Message) -> bool {
    matches!(message, Message::VideoFrame { .. })
}

fn server_message_is_bulk(message: &Message) -> bool {
    matches!(
        message,
        Message::FileTransfer {
            action: FileTransferAction::Directory
                | FileTransferAction::Chunk
                | FileTransferAction::Progress,
            ..
        }
    )
}

fn event_loop(events_rx: Receiver<ServerEvent>) {
    let mut peers: HashMap<usize, Peer> = HashMap::new();
    let mut cancelled_file_transfers = HashSet::<FileTransferKey>::new();

    loop {
        let event = match events_rx.recv_timeout(Duration::from_millis(MAINTENANCE_TICK_MS)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                maintain_peers(&mut peers);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };

        match event {
            ServerEvent::Connected {
                peer_id,
                sender,
                peer_addr,
            } => {
                peers.insert(
                    peer_id,
                    Peer {
                        role: None,
                        identity: None,
                        fingerprint: None,
                        session_token: None,
                        sender,
                        client_info: None,
                        peer_addr,
                        last_seen_epoch_ms: now_epoch_ms(),
                        last_heartbeat_epoch_ms: 0,
                        last_ping_epoch_ms: 0,
                    },
                );
                println!("audit event=connect peer=#{peer_id}");
            }
            ServerEvent::Registered {
                peer_id,
                role,
                identity,
                fingerprint,
                info,
            } => {
                let token = new_session_token(peer_id, &identity, &fingerprint);
                if let Some(peer) = peers.get_mut(&peer_id) {
                    peer.role = Some(role.clone());
                    peer.identity = Some(identity.clone());
                    peer.fingerprint = Some(fingerprint.clone());
                    peer.session_token = Some(token.clone());
                    peer.client_info = info.map(|mut info| {
                        info.peer_addr = peer.peer_addr.clone();
                        info
                    });
                    peer.last_seen_epoch_ms = now_epoch_ms();
                    peer.last_heartbeat_epoch_ms = now_epoch_ms();
                    let _ = peer.sender.send(Message::Session { token });
                }
                println!(
                    "audit event=register peer=#{peer_id} role={} identity={} fingerprint={}",
                    role.as_str(),
                    identity,
                    fingerprint
                );
                broadcast_clients(&peers);
            }
            ServerEvent::Message {
                peer_id,
                session_token,
                message,
            } => {
                if !session_token_valid(peer_id, &session_token, &peers) {
                    println!(
                        "audit event=invalid_token peer=#{peer_id} identity={}",
                        peer_identity(peer_id, &peers)
                    );
                    if let Some(peer) = peers.get(&peer_id) {
                        let _ = peer.sender.send(Message::Error {
                            detail: "invalid or missing session token".to_string(),
                        });
                    }
                    continue;
                }

                match message {
                    Message::ListClients => {
                        mark_seen(peer_id, &mut peers);
                        println!(
                            "audit event=list peer=#{peer_id} identity={}",
                            peer_identity(peer_id, &peers)
                        );
                        send_clients(peer_id, &peers);
                    }
                    Message::Command {
                        target_id,
                        command,
                        payload,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        if command.as_str() == "file_manager" {
                            println!(
                                "audit event=command peer=#{peer_id} identity={} target={} command={} action={} path={}",
                                peer_identity(peer_id, &peers),
                                target_id,
                                command.as_str(),
                                payload_field(&payload, "action")
                                    .unwrap_or_else(|| "list".to_string()),
                                payload_field(&payload, "path").unwrap_or_default()
                            );
                        } else {
                            println!(
                                "audit event=command peer=#{peer_id} identity={} target={} command={}",
                                peer_identity(peer_id, &peers),
                                target_id,
                                command.as_str()
                            );
                        }
                        let detail = route_command(&peers, &target_id, command.clone(), payload);
                        if let Some(peer) = peers.get(&peer_id) {
                            let _ = peer.sender.send(Message::CommandAck {
                                client_id: target_id,
                                command,
                                accepted: detail.is_none(),
                                detail: detail.unwrap_or_else(|| "forwarded".to_string()),
                            });
                        }
                    }
                    Message::CommandAck {
                        client_id,
                        command,
                        accepted,
                        detail,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        let identity = peer_identity(peer_id, &peers);
                        println!(
                            "audit event=ack peer=#{peer_id} identity={identity} client={client_id} command={} accepted={accepted}",
                            command.as_str()
                        );
                        let message = Message::CommandAck {
                            client_id,
                            command,
                            accepted,
                            detail,
                        };
                        for peer in peers.values() {
                            if peer.role == Some(Role::Admin) {
                                let _ = peer.sender.send(message.clone());
                            }
                        }
                    }
                    Message::CommandOutput {
                        client_id,
                        command,
                        stream_id,
                        sequence,
                        stream,
                        chunk,
                        current_dir,
                        finished,
                        success,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        if finished {
                            let identity = peer_identity(peer_id, &peers);
                            println!(
                                "audit event=command_output_finished peer=#{peer_id} identity={identity} client={client_id} command={} success={success}",
                                command.as_str()
                            );
                        }
                        let message = Message::CommandOutput {
                            client_id,
                            command,
                            stream_id,
                            sequence,
                            stream,
                            chunk,
                            current_dir,
                            finished,
                            success,
                        };
                        for peer in peers.values() {
                            if peer.role == Some(Role::Admin) {
                                let _ = peer.sender.send(message.clone());
                            }
                        }
                    }
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
                        message: status_message,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        let source_role = peers.get(&peer_id).and_then(|peer| peer.role.as_ref());
                        let key = file_transfer_key(&target_id, transfer_id, direction);
                        if source_role == Some(&Role::Admin) {
                            match action {
                                FileTransferAction::Start => {
                                    cancelled_file_transfers.remove(&key);
                                }
                                FileTransferAction::Cancel => {
                                    cancelled_file_transfers.insert(key.clone());
                                }
                                _ if cancelled_file_transfers.contains(&key) => {
                                    log_file_transfer_drop(
                                        peer_id,
                                        &peers,
                                        &target_id,
                                        transfer_id,
                                        direction,
                                        action,
                                        "cancelled_admin_send",
                                    );
                                    continue;
                                }
                                _ => {}
                            }
                        } else if cancelled_file_transfers.contains(&key) {
                            log_file_transfer_drop(
                                peer_id,
                                &peers,
                                &target_id,
                                transfer_id,
                                direction,
                                action,
                                "cancelled",
                            );
                            if matches!(
                                action,
                                FileTransferAction::Complete | FileTransferAction::Error
                            ) {
                                cancelled_file_transfers.remove(&key);
                            }
                            continue;
                        } else if matches!(
                            action,
                            FileTransferAction::Complete | FileTransferAction::Error
                        ) {
                            cancelled_file_transfers.remove(&key);
                        }
                        log_file_transfer(
                            peer_id,
                            &peers,
                            &target_id,
                            transfer_id,
                            direction,
                            action,
                            total_bytes,
                            transferred_bytes,
                            &status_message,
                        );
                        let message = Message::FileTransfer {
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
                            message: status_message,
                        };
                        if peers.get(&peer_id).and_then(|peer| peer.role.as_ref())
                            == Some(&Role::Admin)
                        {
                            if let Some(error) = route_file_transfer_to_client(&peers, &message) {
                                send_file_transfer_error(peer_id, &message, error, &peers);
                            }
                        } else {
                            for peer in peers.values() {
                                if peer.role == Some(Role::Admin) {
                                    let _ = peer.sender.send(message.clone());
                                }
                            }
                        }
                    }
                    Message::DesktopControl { target_id, payload } => {
                        mark_seen(peer_id, &mut peers);
                        println!(
                            "audit event=desktop_control peer=#{peer_id} identity={} target={}",
                            peer_identity(peer_id, &peers),
                            target_id
                        );
                        if let Some(error) =
                            route_desktop_to_client(&peers, &target_id, payload, false)
                        {
                            send_desktop_error(peer_id, &target_id, error, &peers);
                        }
                    }
                    Message::DesktopInput { target_id, payload } => {
                        mark_seen(peer_id, &mut peers);
                        if let Some(error) =
                            route_desktop_to_client(&peers, &target_id, payload, true)
                        {
                            send_desktop_error(peer_id, &target_id, error, &peers);
                        }
                    }
                    Message::DesktopFrame { client_id, payload } => {
                        mark_seen(peer_id, &mut peers);
                        let message = Message::DesktopFrame { client_id, payload };
                        for peer in peers.values() {
                            if peer.role == Some(Role::Admin) {
                                let _ = peer.sender.send(message.clone());
                            }
                        }
                    }
                    Message::VideoControl {
                        target_id,
                        source,
                        payload,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        println!(
                            "audit event=video_control peer=#{peer_id} identity={} target={} source={}",
                            peer_identity(peer_id, &peers),
                            target_id,
                            source.as_str()
                        );
                        if let Some(error) =
                            route_video_control_to_client(&peers, &target_id, source, payload)
                        {
                            send_video_error(peer_id, &target_id, error, &peers);
                        }
                    }
                    Message::VideoFrame {
                        client_id,
                        source,
                        seq,
                        source_width,
                        source_height,
                        image_width,
                        image_height,
                        format,
                        bytes,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        let message = Message::VideoFrame {
                            client_id,
                            source,
                            seq,
                            source_width,
                            source_height,
                            image_width,
                            image_height,
                            format,
                            bytes,
                        };
                        for peer in peers.values() {
                            if peer.role == Some(Role::Admin) {
                                let _ = peer.sender.send(message.clone());
                            }
                        }
                    }
                    Message::AudioControl {
                        target_id,
                        source,
                        payload,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        println!(
                            "audit event=audio_control peer=#{peer_id} identity={} target={} source={}",
                            peer_identity(peer_id, &peers),
                            target_id,
                            source.as_str()
                        );
                        if let Some(error) = route_audio_control_to_client(
                            &peers,
                            &target_id,
                            source.clone(),
                            payload,
                        ) {
                            send_audio_error(peer_id, &target_id, source, error, &peers);
                        }
                    }
                    Message::AudioFrame {
                        client_id,
                        source,
                        seq,
                        sample_rate,
                        channels,
                        format,
                        bytes,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        let from_admin = peers.get(&peer_id).and_then(|peer| peer.role.as_ref())
                            == Some(&Role::Admin);
                        let message = Message::AudioFrame {
                            client_id: client_id.clone(),
                            source,
                            seq,
                            sample_rate,
                            channels,
                            format,
                            bytes,
                        };
                        if from_admin {
                            if let Some(peer) = peers.values().find(|peer| {
                                peer.role == Some(Role::Client)
                                    && peer
                                        .client_info
                                        .as_ref()
                                        .map(|info| info.id == client_id)
                                        .unwrap_or(false)
                            }) {
                                let _ = peer.sender.send(message);
                            }
                        } else {
                            for peer in peers.values() {
                                if peer.role == Some(Role::Admin) {
                                    let _ = peer.sender.send(message.clone());
                                }
                            }
                        }
                    }
                    Message::Ping => {
                        mark_seen(peer_id, &mut peers);
                        if let Some(peer) = peers.get(&peer_id) {
                            let _ = peer.sender.send(Message::Pong);
                        }
                    }
                    Message::Pong => {
                        mark_seen(peer_id, &mut peers);
                        mark_heartbeat(peer_id, &mut peers);
                        if peers.get(&peer_id).and_then(|peer| peer.role.as_ref())
                            == Some(&Role::Client)
                        {
                            broadcast_clients(&peers);
                        }
                    }
                    other => eprintln!("peer #{peer_id} sent unsupported message: {other:?}"),
                }
            }
            ServerEvent::Disconnected { peer_id } => {
                let removed = peers.remove(&peer_id);
                let identity = removed
                    .as_ref()
                    .and_then(|peer| peer.identity.as_deref())
                    .unwrap_or("unknown")
                    .to_string();
                println!("audit event=disconnect peer=#{peer_id} identity={identity}");
                if removed.and_then(|peer| peer.client_info).is_some() {
                    broadcast_clients(&peers);
                }
            }
        }
        maintain_peers(&mut peers);
    }
}

fn mark_seen(peer_id: usize, peers: &mut HashMap<usize, Peer>) {
    if let Some(peer) = peers.get_mut(&peer_id) {
        peer.last_seen_epoch_ms = now_epoch_ms();
    }
}

fn mark_heartbeat(peer_id: usize, peers: &mut HashMap<usize, Peer>) {
    if let Some(peer) = peers.get_mut(&peer_id) {
        peer.last_heartbeat_epoch_ms = now_epoch_ms();
    }
}

fn session_token_valid(peer_id: usize, token: &str, peers: &HashMap<usize, Peer>) -> bool {
    peers
        .get(&peer_id)
        .and_then(|peer| peer.session_token.as_deref())
        .map(|expected| !expected.is_empty() && expected == token)
        .unwrap_or(false)
}

fn peer_identity(peer_id: usize, peers: &HashMap<usize, Peer>) -> String {
    peers
        .get(&peer_id)
        .and_then(|peer| peer.identity.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn new_session_token(peer_id: usize, identity: &str, fingerprint: &str) -> String {
    format!(
        "st-{:x}-{:x}-{:x}",
        now_epoch_ms(),
        simple_hash(identity),
        simple_hash(&format!("{peer_id}|{fingerprint}|{}", std::process::id()))
    )
}

fn simple_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn maintain_peers(peers: &mut HashMap<usize, Peer>) {
    let now = now_epoch_ms();
    let mut stale_client_removed = false;

    for (peer_id, peer) in peers.iter_mut() {
        if peer.session_token.is_some()
            && now.saturating_sub(peer.last_ping_epoch_ms) >= HEARTBEAT_INTERVAL_MS
        {
            peer.last_ping_epoch_ms = now;
            let _ = peer.sender.send(Message::Ping);
        }
        if now.saturating_sub(peer.last_seen_epoch_ms) > STALE_PEER_MS {
            println!(
                "audit event=stale peer=#{peer_id} identity={}",
                peer.identity.as_deref().unwrap_or("unknown")
            );
        }
    }

    let before = peers.len();
    peers.retain(|_, peer| now.saturating_sub(peer.last_seen_epoch_ms) <= STALE_PEER_MS);
    if peers.len() != before {
        stale_client_removed = true;
    }

    if stale_client_removed {
        broadcast_clients(peers);
    }
}

fn route_command(
    peers: &HashMap<usize, Peer>,
    target_id: &str,
    command: rdl_protocol::CommandKind,
    payload: String,
) -> Option<String> {
    let target = peers.values().find(|peer| {
        peer.role == Some(Role::Client)
            && peer
                .client_info
                .as_ref()
                .map(|info| info.id == target_id)
                .unwrap_or(false)
    });

    match target {
        Some(peer) => peer
            .sender
            .send(Message::Command {
                target_id: target_id.to_string(),
                command,
                payload,
            })
            .err()
            .map(|error| error.to_string()),
        None => Some(format!("client '{target_id}' is offline")),
    }
}

fn route_desktop_to_client(
    peers: &HashMap<usize, Peer>,
    target_id: &str,
    payload: String,
    input: bool,
) -> Option<String> {
    let target = peers.values().find(|peer| {
        peer.role == Some(Role::Client)
            && peer
                .client_info
                .as_ref()
                .map(|info| info.id == target_id)
                .unwrap_or(false)
    });

    let Some(peer) = target else {
        return Some(format!("client '{target_id}' is offline"));
    };
    let message = if input {
        Message::DesktopInput {
            target_id: target_id.to_string(),
            payload,
        }
    } else {
        Message::DesktopControl {
            target_id: target_id.to_string(),
            payload,
        }
    };
    peer.sender
        .send(message)
        .err()
        .map(|error| error.to_string())
}

fn route_video_control_to_client(
    peers: &HashMap<usize, Peer>,
    target_id: &str,
    source: rdl_protocol::VideoSource,
    payload: String,
) -> Option<String> {
    let target = peers.values().find(|peer| {
        peer.role == Some(Role::Client)
            && peer
                .client_info
                .as_ref()
                .map(|info| info.id == target_id)
                .unwrap_or(false)
    });

    let Some(peer) = target else {
        return Some(format!("client '{target_id}' is offline"));
    };
    peer.sender
        .send(Message::VideoControl {
            target_id: target_id.to_string(),
            source,
            payload,
        })
        .err()
        .map(|error| error.to_string())
}

fn route_audio_control_to_client(
    peers: &HashMap<usize, Peer>,
    target_id: &str,
    source: AudioSource,
    payload: String,
) -> Option<String> {
    let target = peers.values().find(|peer| {
        peer.role == Some(Role::Client)
            && peer
                .client_info
                .as_ref()
                .map(|info| info.id == target_id)
                .unwrap_or(false)
    });

    let Some(peer) = target else {
        return Some(format!("client '{target_id}' is offline"));
    };
    peer.sender
        .send(Message::AudioControl {
            target_id: target_id.to_string(),
            source,
            payload,
        })
        .err()
        .map(|error| error.to_string())
}

fn route_file_transfer_to_client(
    peers: &HashMap<usize, Peer>,
    message: &Message,
) -> Option<String> {
    let Message::FileTransfer { target_id, .. } = message else {
        return Some("invalid file transfer message".to_string());
    };
    let target = peers.values().find(|peer| {
        peer.role == Some(Role::Client)
            && peer
                .client_info
                .as_ref()
                .map(|info| info.id == *target_id)
                .unwrap_or(false)
    });

    let Some(peer) = target else {
        return Some(format!("client '{target_id}' is offline"));
    };
    peer.sender
        .send(message.clone())
        .err()
        .map(|error| error.to_string())
}

fn file_transfer_key(
    client_id: &str,
    transfer_id: u64,
    direction: FileTransferDirection,
) -> FileTransferKey {
    (client_id.to_string(), transfer_id, direction.as_str())
}

fn log_file_transfer(
    peer_id: usize,
    peers: &HashMap<usize, Peer>,
    client_id: &str,
    transfer_id: u64,
    direction: FileTransferDirection,
    action: FileTransferAction,
    total_bytes: u64,
    transferred_bytes: u64,
    status_message: &str,
) {
    if matches!(
        action,
        FileTransferAction::Directory | FileTransferAction::Chunk
    ) {
        return;
    }
    let message = log_message_suffix(status_message);
    println!(
        "audit event=file_transfer peer=#{peer_id} identity={} client={} id={} direction={} action={} bytes={}/{}{}",
        peer_identity(peer_id, peers),
        client_id,
        transfer_id,
        direction.as_str(),
        action.as_str(),
        transferred_bytes,
        total_bytes,
        message,
    );
}

fn log_file_transfer_drop(
    peer_id: usize,
    peers: &HashMap<usize, Peer>,
    client_id: &str,
    transfer_id: u64,
    direction: FileTransferDirection,
    action: FileTransferAction,
    reason: &str,
) {
    if !should_log_file_transfer_action(action) {
        return;
    }
    println!(
        "audit event=file_transfer_drop peer=#{peer_id} identity={} client={} id={} direction={} action={} reason={}",
        peer_identity(peer_id, peers),
        client_id,
        transfer_id,
        direction.as_str(),
        action.as_str(),
        reason,
    );
}

fn should_log_file_transfer_action(action: FileTransferAction) -> bool {
    matches!(
        action,
        FileTransferAction::Start
            | FileTransferAction::Finish
            | FileTransferAction::Cancel
            | FileTransferAction::Complete
            | FileTransferAction::Error
    )
}

fn log_message_suffix(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        return String::new();
    }
    let mut sanitized = message
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    const MAX_LOG_MESSAGE_LEN: usize = 160;
    if sanitized.len() > MAX_LOG_MESSAGE_LEN {
        sanitized.truncate(MAX_LOG_MESSAGE_LEN);
        sanitized.push_str("...");
    }
    format!(" message={sanitized}")
}

fn payload_field(payload: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    payload
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|value| {
            let mut value = value
                .trim()
                .chars()
                .map(|ch| if ch.is_control() { ' ' } else { ch })
                .collect::<String>();
            const MAX_PAYLOAD_FIELD_LEN: usize = 160;
            if value.len() > MAX_PAYLOAD_FIELD_LEN {
                value.truncate(MAX_PAYLOAD_FIELD_LEN);
                value.push_str("...");
            }
            value
        })
}

fn send_desktop_error(
    peer_id: usize,
    client_id: &str,
    error: String,
    peers: &HashMap<usize, Peer>,
) {
    if let Some(peer) = peers.get(&peer_id) {
        let _ = peer.sender.send(Message::DesktopFrame {
            client_id: client_id.to_string(),
            payload: format!("remote_desktop_error\nmessage={error}"),
        });
    }
}

fn send_video_error(peer_id: usize, client_id: &str, error: String, peers: &HashMap<usize, Peer>) {
    if let Some(peer) = peers.get(&peer_id) {
        let _ = peer.sender.send(Message::VideoFrame {
            client_id: client_id.to_string(),
            source: rdl_protocol::VideoSource::RemoteDesktop,
            seq: 0,
            source_width: 0,
            source_height: 0,
            image_width: 0,
            image_height: 0,
            format: format!("error:{error}"),
            bytes: Vec::new(),
        });
    }
}

fn send_audio_error(
    peer_id: usize,
    client_id: &str,
    source: AudioSource,
    error: String,
    peers: &HashMap<usize, Peer>,
) {
    if let Some(peer) = peers.get(&peer_id) {
        let (command, prefix) = match source {
            AudioSource::AudioListen => (rdl_protocol::CommandKind::AudioListen, "audio_listen"),
            AudioSource::VoiceChat => (rdl_protocol::CommandKind::VoiceChat, "voice_chat"),
        };
        let _ = peer.sender.send(Message::CommandAck {
            client_id: client_id.to_string(),
            command,
            accepted: false,
            detail: format!("{prefix}_error\nmessage={error}"),
        });
    }
}

fn send_file_transfer_error(
    peer_id: usize,
    message: &Message,
    error: String,
    peers: &HashMap<usize, Peer>,
) {
    if let (
        Some(peer),
        Message::FileTransfer {
            target_id,
            transfer_id,
            direction,
            path,
            relative_path,
            ..
        },
    ) = (peers.get(&peer_id), message)
    {
        let _ = peer.sender.send(Message::FileTransfer {
            target_id: target_id.clone(),
            transfer_id: *transfer_id,
            direction: *direction,
            action: rdl_protocol::FileTransferAction::Error,
            path: path.clone(),
            relative_path: relative_path.clone(),
            total_bytes: 0,
            transferred_bytes: 0,
            file_size: 0,
            offset: 0,
            bytes: Vec::new(),
            message: error,
        });
    }
}

fn send_clients(peer_id: usize, peers: &HashMap<usize, Peer>) {
    if let Some(peer) = peers.get(&peer_id) {
        let _ = peer.sender.send(Message::Clients(online_clients(peers)));
    }
}

fn broadcast_clients(peers: &HashMap<usize, Peer>) {
    let clients = online_clients(peers);
    for peer in peers.values() {
        if peer.role == Some(Role::Admin) {
            let _ = peer.sender.send(Message::Clients(clients.clone()));
        }
    }
}

fn online_clients(peers: &HashMap<usize, Peer>) -> Vec<ClientInfo> {
    peers
        .values()
        .filter_map(|peer| {
            let mut info = peer.client_info.clone()?;
            info.last_seen_epoch_ms = peer.last_heartbeat_epoch_ms;
            Some(info)
        })
        .collect()
}

struct Config {
    ip: String,
    port: u16,
}

impl Config {
    fn from_env() -> Self {
        let mut ip = "0.0.0.0".to_string();
        let mut port = rdl_protocol::DEFAULT_SERVER_PORT;
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--ip" => {
                    if let Some(value) = args.next() {
                        ip = value;
                    }
                }
                "--port" => {
                    if let Some(value) = args.next() {
                        if let Ok(value) = value.parse() {
                            port = value;
                        }
                    }
                }
                "--version" | "-V" => {
                    println!("{}", rdl_version::app_version("rdl-server"));
                    std::process::exit(0);
                }
                "--help" | "-h" => {
                    println!("Usage: rdl-server [--ip 0.0.0.0] [--port 5169] [--version]");
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        Self { ip, port }
    }
}

use rdl_protocol::{
    audio_udp, now_epoch_ms, read_envelope, write_envelope, AudioSource, ClientInfo,
    ClientLocation, FileTransferAction, FileTransferDirection, Message, Role,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

const HEARTBEAT_INTERVAL_MS: u128 = 10_000;
const STALE_PEER_MS: u128 = 45_000;
const MAINTENANCE_TICK_MS: u64 = 100;
const WRITER_BULK_POLL_MS: u64 = 2;
const WRITER_VIDEO_QUEUE_CAPACITY: usize = 4;
const UDP_RELAY_IDLE_TIMEOUT_MS: u64 = 30_000;
const UDP_RELAY_RECV_TIMEOUT_MS: u64 = 100;
const UDP_RELAY_MAINTENANCE_MS: u64 = 1_000;

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
        auth_token: String,
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
    video: SyncSender<Message>,
    bulk: Sender<Message>,
}

impl PeerSender {
    fn send(&self, message: Message) -> Result<(), mpsc::SendError<Message>> {
        if server_message_is_video_realtime(&message) {
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
type ProxyStreamKey = (String, u64);

#[derive(Clone)]
struct AuthConfig {
    token: String,
    generated: bool,
    require_client_auth: bool,
}

struct GeoIpLocator {
    reader: Option<maxminddb::Reader<Vec<u8>>>,
    path: Option<PathBuf>,
}

impl GeoIpLocator {
    fn open(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self {
                reader: None,
                path: None,
            };
        };
        match maxminddb::Reader::open_readfile(path) {
            Ok(reader) => Self {
                reader: Some(reader),
                path: Some(path.to_path_buf()),
            },
            Err(error) => {
                eprintln!("geoip disabled: {}: {error}", path.display());
                Self {
                    reader: None,
                    path: Some(path.to_path_buf()),
                }
            }
        }
    }

    fn status_label(&self) -> String {
        match (&self.reader, &self.path) {
            (Some(_), Some(path)) => path.display().to_string(),
            (Some(_), None) => "enabled".to_string(),
            (None, Some(path)) => format!("disabled({})", path.display()),
            (None, None) => "disabled".to_string(),
        }
    }

    fn lookup_peer_addr(&self, peer_addr: &str) -> Option<ClientLocation> {
        let reader = self.reader.as_ref()?;
        let ip = peer_ip(peer_addr)?;
        if !geoip_candidate(ip) {
            return None;
        }
        let result = reader.lookup(ip).ok()?;
        let city = result.decode::<maxminddb::geoip2::City>().ok()??;
        let latitude = city.location.latitude?;
        let longitude = city.location.longitude?;
        let accuracy_meters = city
            .location
            .accuracy_radius
            .map(|km| u32::from(km).saturating_mul(1_000))
            .unwrap_or(0);
        Some(ClientLocation::from_degrees(
            latitude,
            longitude,
            accuracy_meters,
            "ip",
            geoip_label(&city),
            now_epoch_ms(),
        ))
    }
}

fn peer_ip(peer_addr: &str) -> Option<IpAddr> {
    peer_addr
        .parse::<SocketAddr>()
        .map(|addr| addr.ip())
        .ok()
        .or_else(|| peer_addr.parse::<IpAddr>().ok())
}

fn geoip_candidate(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_multicast()
                || ip.is_unspecified())
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ((ip.segments()[0] & 0xffc0) == 0xfe80))
        }
    }
}

fn geoip_label(city: &maxminddb::geoip2::City<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(name) = preferred_name(&city.city.names) {
        parts.push(name.to_string());
    }
    if let Some(subdivision) = city
        .subdivisions
        .first()
        .and_then(|subdivision| preferred_name(&subdivision.names))
    {
        if !parts.iter().any(|part| part == subdivision) {
            parts.push(subdivision.to_string());
        }
    }
    if let Some(country) = preferred_name(&city.country.names).or(city.country.iso_code) {
        if !parts.iter().any(|part| part == country) {
            parts.push(country.to_string());
        }
    }
    if parts.is_empty() {
        "IP geolocation".to_string()
    } else {
        parts.join(", ")
    }
}

fn preferred_name<'a>(names: &'a maxminddb::geoip2::Names<'a>) -> Option<&'a str> {
    names.simplified_chinese.or(names.english)
}

fn main() -> io::Result<()> {
    let config = Config::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let geoip = GeoIpLocator::open(config.geoip_db_path.as_deref());
    let bind_addr = format!("{}:{}", config.ip, config.port);
    let listener = TcpListener::bind(&bind_addr)?;
    let (events_tx, events_rx) = mpsc::channel();

    println!(
        "rust-desk-light server listening on {bind_addr} version={} geoip={}",
        rdl_version::display_version(),
        geoip.status_label()
    );
    println!("{}", config.startup_notice);
    println!("auth token: {} ({})", config.auth.token, config.auth_source);
    if config.auth.generated {
        println!(
            "saved generated auth token to config: {}",
            config.config_path.display()
        );
    }
    println!("use this token in rdl-admin-gui; clients need it only when client_auth=required");
    start_audio_udp_relay(bind_addr.clone());
    thread::spawn(move || accept_loop(listener, events_tx));
    event_loop(events_rx, geoip, config.auth);
    Ok(())
}

fn start_audio_udp_relay(bind_addr: String) {
    thread::spawn(move || match UdpSocket::bind(&bind_addr) {
        Ok(socket) => {
            println!("audio udp relay listening on {bind_addr}");
            if let Err(error) = audio_udp_relay_loop(socket) {
                eprintln!("audio udp relay stopped: {error}");
            }
        }
        Err(error) => eprintln!("audio udp relay bind failed on {bind_addr}: {error}"),
    });
}

#[derive(Clone, Copy)]
struct AudioUdpRoute {
    receiver_addr: SocketAddr,
    last_seen: Instant,
}

fn audio_udp_relay_loop(socket: UdpSocket) -> io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_millis(UDP_RELAY_RECV_TIMEOUT_MS)))?;
    let mut routes = HashMap::<u64, AudioUdpRoute>::new();
    let mut buf = [0_u8; audio_udp::MAX_PACKET_BYTES];
    let mut last_maintenance = Instant::now();
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, addr)) => match audio_udp::decode(&buf[..len]) {
                Ok(audio_udp::Packet::Register { stream_id }) => {
                    routes.insert(
                        stream_id,
                        AudioUdpRoute {
                            receiver_addr: addr,
                            last_seen: Instant::now(),
                        },
                    );
                }
                Ok(audio_udp::Packet::Unregister { stream_id }) => {
                    if routes
                        .get(&stream_id)
                        .map(|route| route.receiver_addr == addr)
                        .unwrap_or(false)
                    {
                        routes.remove(&stream_id);
                    }
                }
                Ok(audio_udp::Packet::Audio { stream_id, .. }) => {
                    if let Some(route) = routes.get_mut(&stream_id) {
                        route.last_seen = Instant::now();
                        let _ = socket.send_to(&buf[..len], route.receiver_addr);
                    }
                }
                Err(error) => {
                    eprintln!("audio udp relay ignored packet from {addr}: {error}");
                }
            },
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error),
        }

        if last_maintenance.elapsed() >= Duration::from_millis(UDP_RELAY_MAINTENANCE_MS) {
            let now = Instant::now();
            routes.retain(|_, route| {
                now.duration_since(route.last_seen)
                    < Duration::from_millis(UDP_RELAY_IDLE_TIMEOUT_MS)
            });
            last_maintenance = now;
        }
    }
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
    let (video_tx, video_rx) = mpsc::sync_channel::<Message>(WRITER_VIDEO_QUEUE_CAPACITY);
    let (bulk_tx, bulk_rx) = mpsc::channel::<Message>();
    if events_tx
        .send(ServerEvent::Connected {
            peer_id,
            sender: PeerSender {
                high: high_tx,
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

    thread::spawn(move || writer_loop(peer_id, writer, high_rx, video_rx, bulk_rx));

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
                auth_token,
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
                        location: None,
                    })
                } else {
                    None
                };
                let _ = events_tx.send(ServerEvent::Registered {
                    peer_id,
                    role,
                    auth_token,
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
    video_rx: Receiver<Message>,
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
        } | Message::ProxyData { .. }
    )
}

fn event_loop(events_rx: Receiver<ServerEvent>, geoip: GeoIpLocator, auth: AuthConfig) {
    let mut peers: HashMap<usize, Peer> = HashMap::new();
    let mut cancelled_file_transfers = HashSet::<FileTransferKey>::new();
    let mut proxy_routes = HashMap::<ProxyStreamKey, usize>::new();

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
                auth_token,
                identity,
                fingerprint,
                info,
            } => {
                if !registration_auth_valid(&role, &auth_token, &auth) {
                    println!(
                        "audit event=auth_reject peer=#{peer_id} role={} identity={}",
                        role.as_str(),
                        identity
                    );
                    if let Some(peer) = peers.get(&peer_id) {
                        let _ = peer.sender.send(Message::Error {
                            detail: "auth failed: invalid or missing token".to_string(),
                        });
                    }
                    continue;
                }
                let token = new_session_token(peer_id, &identity, &fingerprint);
                if let Some(peer) = peers.get_mut(&peer_id) {
                    peer.role = Some(role.clone());
                    peer.identity = Some(identity.clone());
                    peer.fingerprint = Some(fingerprint.clone());
                    peer.session_token = Some(token.clone());
                    peer.client_info = info.map(|mut info| {
                        info.peer_addr = peer.peer_addr.clone();
                        info.location = geoip.lookup_peer_addr(&peer.peer_addr);
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
                    Message::ProxyOpen {
                        target_id,
                        stream_id,
                        host,
                        port,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        if peer_role(peer_id, &peers) != Some(Role::Admin) {
                            eprintln!("peer #{peer_id} sent proxy open without admin role");
                            continue;
                        }
                        println!(
                            "audit event=proxy_open peer=#{peer_id} identity={} client={} stream={} target={}:{}",
                            peer_identity(peer_id, &peers),
                            target_id,
                            stream_id,
                            host,
                            port
                        );
                        let key = (target_id.clone(), stream_id);
                        proxy_routes.insert(key.clone(), peer_id);
                        if let Some(error) =
                            route_proxy_open_to_client(&peers, &target_id, stream_id, host, port)
                        {
                            proxy_routes.remove(&key);
                            send_proxy_open_result(
                                peer_id, &target_id, stream_id, false, error, &peers,
                            );
                        }
                    }
                    Message::ProxyOpenResult {
                        client_id,
                        stream_id,
                        accepted,
                        detail,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        if peer_role(peer_id, &peers) != Some(Role::Client) {
                            eprintln!("peer #{peer_id} sent proxy open result without client role");
                            continue;
                        }
                        let key = (client_id.clone(), stream_id);
                        if let Some(error) = route_proxy_to_admin(
                            &peers,
                            &proxy_routes,
                            &key,
                            Message::ProxyOpenResult {
                                client_id: client_id.clone(),
                                stream_id,
                                accepted,
                                detail,
                            },
                        ) {
                            eprintln!("proxy open result route failed: {error}");
                        }
                        if !accepted {
                            proxy_routes.remove(&key);
                        }
                    }
                    Message::ProxyData {
                        client_id,
                        stream_id,
                        bytes,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        let key = (client_id.clone(), stream_id);
                        match peer_role(peer_id, &peers) {
                            Some(Role::Admin) => {
                                if let Some(error) =
                                    route_proxy_data_to_client(&peers, &client_id, stream_id, bytes)
                                {
                                    proxy_routes.remove(&key);
                                    send_proxy_close(peer_id, &client_id, stream_id, error, &peers);
                                }
                            }
                            Some(Role::Client) => {
                                if let Some(error) = route_proxy_to_admin(
                                    &peers,
                                    &proxy_routes,
                                    &key,
                                    Message::ProxyData {
                                        client_id,
                                        stream_id,
                                        bytes,
                                    },
                                ) {
                                    eprintln!("proxy data route failed: {error}");
                                }
                            }
                            _ => eprintln!("peer #{peer_id} sent proxy data before registration"),
                        }
                    }
                    Message::ProxyClose {
                        client_id,
                        stream_id,
                        reason,
                    } => {
                        mark_seen(peer_id, &mut peers);
                        let key = (client_id.clone(), stream_id);
                        match peer_role(peer_id, &peers) {
                            Some(Role::Admin) => {
                                if let Some(error) = route_proxy_close_to_client(
                                    &peers, &client_id, stream_id, reason,
                                ) {
                                    send_proxy_close(peer_id, &client_id, stream_id, error, &peers);
                                }
                                proxy_routes.remove(&key);
                            }
                            Some(Role::Client) => {
                                if let Some(error) = route_proxy_to_admin(
                                    &peers,
                                    &proxy_routes,
                                    &key,
                                    Message::ProxyClose {
                                        client_id,
                                        stream_id,
                                        reason,
                                    },
                                ) {
                                    eprintln!("proxy close route failed: {error}");
                                }
                                proxy_routes.remove(&key);
                            }
                            _ => eprintln!("peer #{peer_id} sent proxy close before registration"),
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
                remove_proxy_routes_for_peer(peer_id, removed.as_ref(), &mut proxy_routes, &peers);
                if removed.and_then(|peer| peer.client_info).is_some() {
                    broadcast_clients(&peers);
                }
            }
        }
        maintain_peers(&mut peers);
    }
}

fn peer_role(peer_id: usize, peers: &HashMap<usize, Peer>) -> Option<Role> {
    peers.get(&peer_id).and_then(|peer| peer.role.clone())
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

fn registration_auth_valid(role: &Role, auth_token: &str, auth: &AuthConfig) -> bool {
    let required = *role == Role::Admin || auth.require_client_auth;
    if !required {
        return true;
    }
    constant_time_eq(auth_token.as_bytes(), auth.token.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
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

fn route_proxy_open_to_client(
    peers: &HashMap<usize, Peer>,
    target_id: &str,
    stream_id: u64,
    host: String,
    port: u16,
) -> Option<String> {
    let Some(peer) = find_client_peer(peers, target_id) else {
        return Some(format!("client '{target_id}' is offline"));
    };
    peer.sender
        .send(Message::ProxyOpen {
            target_id: target_id.to_string(),
            stream_id,
            host,
            port,
        })
        .err()
        .map(|error| error.to_string())
}

fn route_proxy_data_to_client(
    peers: &HashMap<usize, Peer>,
    client_id: &str,
    stream_id: u64,
    bytes: Vec<u8>,
) -> Option<String> {
    let Some(peer) = find_client_peer(peers, client_id) else {
        return Some(format!("client '{client_id}' is offline"));
    };
    peer.sender
        .send(Message::ProxyData {
            client_id: client_id.to_string(),
            stream_id,
            bytes,
        })
        .err()
        .map(|error| error.to_string())
}

fn route_proxy_close_to_client(
    peers: &HashMap<usize, Peer>,
    client_id: &str,
    stream_id: u64,
    reason: String,
) -> Option<String> {
    let Some(peer) = find_client_peer(peers, client_id) else {
        return Some(format!("client '{client_id}' is offline"));
    };
    peer.sender
        .send(Message::ProxyClose {
            client_id: client_id.to_string(),
            stream_id,
            reason,
        })
        .err()
        .map(|error| error.to_string())
}

fn route_proxy_to_admin(
    peers: &HashMap<usize, Peer>,
    proxy_routes: &HashMap<ProxyStreamKey, usize>,
    key: &ProxyStreamKey,
    message: Message,
) -> Option<String> {
    let Some(admin_peer_id) = proxy_routes.get(key) else {
        return Some(format!(
            "proxy stream {} for client '{}' has no admin route",
            key.1, key.0
        ));
    };
    let Some(peer) = peers.get(admin_peer_id) else {
        return Some(format!("admin peer #{admin_peer_id} is offline"));
    };
    peer.sender
        .send(message)
        .err()
        .map(|error| error.to_string())
}

fn find_client_peer<'a>(peers: &'a HashMap<usize, Peer>, client_id: &str) -> Option<&'a Peer> {
    peers.values().find(|peer| {
        peer.role == Some(Role::Client)
            && peer
                .client_info
                .as_ref()
                .map(|info| info.id == client_id)
                .unwrap_or(false)
    })
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

fn send_proxy_open_result(
    peer_id: usize,
    client_id: &str,
    stream_id: u64,
    accepted: bool,
    detail: String,
    peers: &HashMap<usize, Peer>,
) {
    if let Some(peer) = peers.get(&peer_id) {
        let _ = peer.sender.send(Message::ProxyOpenResult {
            client_id: client_id.to_string(),
            stream_id,
            accepted,
            detail,
        });
    }
}

fn send_proxy_close(
    peer_id: usize,
    client_id: &str,
    stream_id: u64,
    reason: String,
    peers: &HashMap<usize, Peer>,
) {
    if let Some(peer) = peers.get(&peer_id) {
        let _ = peer.sender.send(Message::ProxyClose {
            client_id: client_id.to_string(),
            stream_id,
            reason,
        });
    }
}

fn remove_proxy_routes_for_peer(
    peer_id: usize,
    removed: Option<&Peer>,
    proxy_routes: &mut HashMap<ProxyStreamKey, usize>,
    peers: &HashMap<usize, Peer>,
) {
    let removed_client_id = removed
        .and_then(|peer| peer.client_info.as_ref())
        .map(|info| info.id.clone());
    let routes = proxy_routes
        .iter()
        .filter_map(|((client_id, stream_id), admin_peer_id)| {
            if *admin_peer_id == peer_id || removed_client_id.as_deref() == Some(client_id) {
                Some((client_id.clone(), *stream_id, *admin_peer_id))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for (client_id, stream_id, admin_peer_id) in routes {
        proxy_routes.remove(&(client_id.clone(), stream_id));
        if admin_peer_id == peer_id {
            let _ = route_proxy_close_to_client(
                peers,
                &client_id,
                stream_id,
                "admin disconnected".to_string(),
            );
        } else if let Some(peer) = peers.get(&admin_peer_id) {
            let _ = peer.sender.send(Message::ProxyClose {
                client_id,
                stream_id,
                reason: "client disconnected".to_string(),
            });
        }
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
    config_path: PathBuf,
    startup_notice: String,
    auth_source: &'static str,
    geoip_db_path: Option<PathBuf>,
    auth: AuthConfig,
}

impl Config {
    fn from_env() -> Result<Self, rdl_config::ConfigError> {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let parsed = rdl_config::parse_endpoint_args(args.clone())?;
        if parsed.version {
            println!("{}", rdl_version::app_version("rdl-server-cli"));
            std::process::exit(0);
        }
        if parsed.help {
            println!("{}", server_help_text());
            std::process::exit(0);
        }

        let loaded =
            rdl_config::load_endpoint_config(rdl_config::ConfigKind::Server, &parsed.overrides)?;
        let geoip_db_path = parse_geoip_db_path(&args)?;
        let (auth_token, generated) = loaded
            .auth_token
            .clone()
            .filter(|token| !token.trim().is_empty())
            .map(|token| (token, false))
            .unwrap_or_else(|| (generate_auth_token(), true));
        if generated {
            rdl_config::write_auth_token_config(
                rdl_config::ConfigKind::Server,
                &loaded.config_path,
                &auth_token,
            )?;
        }
        let startup_notice = server_startup_config_notice(&loaded);
        let auth_source = auth_source_label(&loaded, generated);
        Ok(Self {
            ip: loaded.endpoint.ip,
            port: loaded.endpoint.port,
            config_path: loaded.config_path,
            startup_notice,
            auth_source,
            geoip_db_path,
            auth: AuthConfig {
                token: auth_token,
                generated,
                require_client_auth: loaded.require_client_auth,
            },
        })
    }
}

fn server_startup_config_notice(loaded: &rdl_config::LoadedEndpointConfig) -> String {
    format!(
        "config file: {}\nlisten: {}:{} ({})\nclient auth: {} ({})",
        loaded.config_path.display(),
        loaded.endpoint.ip,
        loaded.endpoint.port,
        endpoint_source_label(loaded),
        client_auth_status_label(loaded.require_client_auth),
        client_auth_source_label(loaded)
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

fn client_auth_source_label(loaded: &rdl_config::LoadedEndpointConfig) -> &'static str {
    if loaded.cli_require_client_auth.is_some() {
        "args"
    } else if loaded.file_require_client_auth.is_some() {
        "file"
    } else {
        "default"
    }
}

fn client_auth_status_label(required: bool) -> &'static str {
    if required {
        "required"
    } else {
        "optional"
    }
}

fn parse_geoip_db_path(args: &[String]) -> Result<Option<PathBuf>, rdl_config::ConfigError> {
    let mut value = std::env::var_os("RDL_GEOIP_DB").map(PathBuf::from);
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--geoip-db" {
            let Some(path) = args.get(index + 1) else {
                return Err(rdl_config::ConfigError::MissingValue("--geoip-db"));
            };
            value = Some(PathBuf::from(path));
            index += 2;
            continue;
        }
        if let Some(path) = arg.strip_prefix("--geoip-db=") {
            value = Some(PathBuf::from(path));
        }
        index += 1;
    }
    Ok(value)
}

fn generate_auth_token() -> String {
    let mut bytes = [0_u8; 24];
    if getrandom::fill(&mut bytes).is_err() {
        let fallback = format!(
            "{}|{}|{}",
            now_epoch_ms(),
            std::process::id(),
            thread::current().name().unwrap_or("server")
        );
        let first = simple_hash(&fallback).to_be_bytes();
        let second = simple_hash(&format!("{fallback}|fallback")).to_be_bytes();
        let third = simple_hash(&format!("{fallback}|token")).to_be_bytes();
        bytes[..8].copy_from_slice(&first);
        bytes[8..16].copy_from_slice(&second);
        bytes[16..24].copy_from_slice(&third);
    }
    let mut token = String::from("rdl-");
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    token
}

fn server_help_text() -> String {
    format!(
        "{}\n\nAuth:\n  --auth-token TOKEN           Shared token required for admin registration.\n  --require-client-auth        Also require the shared token for clients.\n  --no-require-client-auth     Allow clients without the token (default).\n  RDL_AUTH_TOKEN=TOKEN         Environment variable fallback for --auth-token.\n\nGeoIP:\n  --geoip-db PATH              Optional MaxMind GeoLite2/GeoIP2 City .mmdb used to place clients on the admin map.\n  RDL_GEOIP_DB=PATH            Environment variable fallback for --geoip-db.",
        rdl_config::help_text("rdl-server-cli", rdl_config::ConfigKind::Server)
    )
}

#[cfg(test)]
mod tests {
    use super::{geoip_candidate, peer_ip};
    use std::net::IpAddr;

    #[test]
    fn peer_ip_parses_socket_addresses() {
        assert_eq!(
            peer_ip("203.0.113.10:5169"),
            Some("203.0.113.10".parse::<IpAddr>().unwrap())
        );
        assert_eq!(
            peer_ip("[2001:db8::1]:5169"),
            Some("2001:db8::1".parse::<IpAddr>().unwrap())
        );
    }

    #[test]
    fn geoip_candidate_skips_local_addresses() {
        assert!(!geoip_candidate("127.0.0.1".parse().unwrap()));
        assert!(!geoip_candidate("10.0.0.2".parse().unwrap()));
        assert!(!geoip_candidate("::1".parse().unwrap()));
        assert!(!geoip_candidate("fe80::1".parse().unwrap()));
    }
}

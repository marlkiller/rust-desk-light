use rdl_protocol::{now_epoch_ms, read_envelope, write_envelope, ClientInfo, Message, Role};
use std::collections::HashMap;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

const HEARTBEAT_INTERVAL_MS: u128 = 10_000;
const STALE_PEER_MS: u128 = 45_000;
const MAINTENANCE_TICK_MS: u64 = 100;

#[derive(Debug)]
enum ServerEvent {
    Connected {
        peer_id: usize,
        sender: Sender<Message>,
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

#[derive(Clone)]
struct Peer {
    role: Option<Role>,
    identity: Option<String>,
    fingerprint: Option<String>,
    session_token: Option<String>,
    sender: Sender<Message>,
    client_info: Option<ClientInfo>,
    last_seen_epoch_ms: u128,
    last_heartbeat_epoch_ms: u128,
    last_ping_epoch_ms: u128,
}

fn main() -> io::Result<()> {
    let config = Config::from_env();
    let bind_addr = format!("{}:{}", config.ip, config.port);
    let listener = TcpListener::bind(&bind_addr)?;
    let (events_tx, events_rx) = mpsc::channel();

    println!("rust-desk-light server listening on {bind_addr}");
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
    if let Err(error) = stream.set_nodelay(true) {
        eprintln!("peer {peer_id} set TCP_NODELAY failed: {error}");
    }
    let (out_tx, out_rx) = mpsc::channel::<Message>();
    if events_tx
        .send(ServerEvent::Connected {
            peer_id,
            sender: out_tx,
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

    thread::spawn(move || writer_loop(peer_id, writer, out_rx));

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

fn writer_loop(peer_id: usize, mut writer: TcpStream, out_rx: Receiver<Message>) {
    let mut next_message_id = 1u64;
    for message in out_rx {
        let result = write_envelope(&mut writer, Role::Server, next_message_id, None, message);
        next_message_id = next_message_id.saturating_add(1);
        if let Err(error) = result {
            eprintln!("peer {peer_id} write failed: {error}");
            break;
        }
    }
}

fn event_loop(events_rx: Receiver<ServerEvent>) {
    let mut peers: HashMap<usize, Peer> = HashMap::new();

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
            ServerEvent::Connected { peer_id, sender } => {
                peers.insert(
                    peer_id,
                    Peer {
                        role: None,
                        identity: None,
                        fingerprint: None,
                        session_token: None,
                        sender,
                        client_info: None,
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
                    peer.client_info = info.clone();
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
                        println!(
                            "audit event=command peer=#{peer_id} identity={} target={} command={}",
                            peer_identity(peer_id, &peers),
                            target_id,
                            command.as_str()
                        );
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
                "--help" | "-h" => {
                    println!("Usage: rdl-server [--ip 0.0.0.0] [--port 21115]");
                    std::process::exit(0);
                }
                _ => {}
            }
        }

        Self { ip, port }
    }
}

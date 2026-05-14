use rdl_protocol::{now_epoch_ms, read_envelope, write_envelope, ClientInfo, Message, Role};
use std::collections::HashMap;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Debug)]
enum ServerEvent {
    Connected {
        peer_id: usize,
        sender: Sender<Message>,
    },
    Registered {
        peer_id: usize,
        role: Role,
        info: Option<ClientInfo>,
    },
    Message {
        peer_id: usize,
        message: Message,
    },
    Disconnected {
        peer_id: usize,
    },
}

#[derive(Clone)]
struct Peer {
    role: Option<Role>,
    sender: Sender<Message>,
    client_info: Option<ClientInfo>,
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
                hostname,
                os,
                username,
                gui_available,
            } => {
                let info = if role == Role::Client {
                    Some(ClientInfo {
                        id,
                        hostname,
                        os,
                        username,
                        gui_available,
                        started_at_epoch_ms: now_epoch_ms(),
                    })
                } else {
                    None
                };
                let _ = events_tx.send(ServerEvent::Registered {
                    peer_id,
                    role,
                    info,
                });
            }
            message => {
                let _ = events_tx.send(ServerEvent::Message { peer_id, message });
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

    for event in events_rx {
        match event {
            ServerEvent::Connected { peer_id, sender } => {
                peers.insert(
                    peer_id,
                    Peer {
                        role: None,
                        sender,
                        client_info: None,
                    },
                );
                println!("peer #{peer_id} connected");
            }
            ServerEvent::Registered {
                peer_id,
                role,
                info,
            } => {
                if let Some(peer) = peers.get_mut(&peer_id) {
                    peer.role = Some(role.clone());
                    peer.client_info = info.clone();
                }
                println!("peer #{peer_id} registered as {}", role.as_str());
                broadcast_clients(&peers);
            }
            ServerEvent::Message { peer_id, message } => match message {
                Message::ListClients => send_clients(peer_id, &peers),
                Message::Command {
                    target_id,
                    command,
                    payload,
                } => {
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
                Message::CommandAck { .. } => {
                    for peer in peers.values() {
                        if peer.role == Some(Role::Admin) {
                            let _ = peer.sender.send(message.clone());
                        }
                    }
                }
                Message::Ping => {
                    if let Some(peer) = peers.get(&peer_id) {
                        let _ = peer.sender.send(Message::Pong);
                    }
                }
                other => eprintln!("peer #{peer_id} sent unsupported message: {other:?}"),
            },
            ServerEvent::Disconnected { peer_id } => {
                let removed = peers.remove(&peer_id);
                println!("peer #{peer_id} disconnected");
                if removed.and_then(|peer| peer.client_info).is_some() {
                    broadcast_clients(&peers);
                }
            }
        }
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
        .filter_map(|peer| peer.client_info.clone())
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

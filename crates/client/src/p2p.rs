use crate::app_event::{ClientEvent, ClientEventSink};
use crate::outbound::{queue_message, ClientOutbound};
use rdl_protocol::{now_epoch_ms, p2p_udp, Message, Role};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::SyncSender,
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const REGISTER_INTERVAL_MS: u64 = 200;
const PROBE_INTERVAL_MS: u64 = 80;
const RECV_TIMEOUT_MS: u64 = 40;
const TEST_TIMEOUT_MS: u64 = 8_000;

pub(crate) struct P2pTestSession {
    stop: Arc<AtomicBool>,
    peer_addr: Arc<Mutex<Option<SocketAddr>>>,
}

impl P2pTestSession {
    pub(crate) fn set_peer_addr(&self, addr: SocketAddr) {
        if let Ok(mut peer_addr) = self.peer_addr.lock() {
            *peer_addr = Some(addr);
        }
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn start_test(
    client_id: String,
    session_id: u64,
    nonce: u64,
    advertised_server_udp_addr: String,
    fallback_server_udp_addr: String,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    event_sink: ClientEventSink,
) -> P2pTestSession {
    let stop = Arc::new(AtomicBool::new(false));
    let peer_addr = Arc::new(Mutex::new(None));
    let worker_stop = stop.clone();
    let worker_peer_addr = peer_addr.clone();
    thread::spawn(move || {
        p2p_test_loop(
            client_id,
            session_id,
            nonce,
            advertised_server_udp_addr,
            fallback_server_udp_addr,
            out_tx,
            session_token,
            event_sink,
            worker_stop,
            worker_peer_addr,
        );
    });
    P2pTestSession { stop, peer_addr }
}

fn p2p_test_loop(
    client_id: String,
    session_id: u64,
    nonce: u64,
    advertised_server_udp_addr: String,
    fallback_server_udp_addr: String,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    event_sink: ClientEventSink,
    stop: Arc<AtomicBool>,
    peer_addr: Arc<Mutex<Option<SocketAddr>>>,
) {
    let server_addr = match resolve_server_addr(&advertised_server_udp_addr, &fallback_server_udp_addr)
    {
        Ok(addr) => addr,
        Err(error) => {
            report(
                &out_tx,
                &session_token,
                &client_id,
                session_id,
                false,
                true,
                "",
                0,
                format!("p2p server udp address invalid: {error}"),
            );
            return;
        }
    };
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => socket,
        Err(error) => {
            report(
                &out_tx,
                &session_token,
                &client_id,
                session_id,
                false,
                true,
                "",
                0,
                format!("p2p udp bind failed: {error}"),
            );
            return;
        }
    };
    if let Err(error) = socket.set_read_timeout(Some(Duration::from_millis(RECV_TIMEOUT_MS))) {
        report(
            &out_tx,
            &session_token,
            &client_id,
            session_id,
            false,
            true,
            "",
            0,
            format!("p2p udp timeout setup failed: {error}"),
        );
        return;
    }
    let local_endpoint = socket
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| String::new());
    event_sink.send(ClientEvent::Log(format!(
        "p2p test started session={session_id} local={local_endpoint} server={server_addr}"
    )));
    report(
        &out_tx,
        &session_token,
        &client_id,
        session_id,
        true,
        false,
        &local_endpoint,
        0,
        format!("client udp bound local={local_endpoint}"),
    );

    let started_at = Instant::now();
    let mut last_register = Instant::now() - Duration::from_millis(REGISTER_INTERVAL_MS);
    let mut last_probe = Instant::now() - Duration::from_millis(PROBE_INTERVAL_MS);
    let mut sequence = 1_u64;
    let mut buf = [0_u8; p2p_udp::MAX_PACKET_BYTES];
    let mut packet = Vec::with_capacity(p2p_udp::MAX_PACKET_BYTES);
    let mut success_reported = false;
    let mut peer_logged = false;

    while !stop.load(Ordering::Relaxed) && started_at.elapsed() < Duration::from_millis(TEST_TIMEOUT_MS)
    {
        if last_register.elapsed() >= Duration::from_millis(REGISTER_INTERVAL_MS) {
            p2p_udp::encode_register(Role::Client, session_id, nonce, &mut packet);
            let _ = socket.send_to(&packet, server_addr);
            last_register = Instant::now();
        }

        let current_peer = peer_addr.lock().ok().and_then(|addr| *addr);
        if let Some(peer) = current_peer {
            if !peer_logged {
                event_sink.send(ClientEvent::Log(format!(
                    "p2p peer endpoint received session={session_id} peer={peer}"
                )));
                peer_logged = true;
            }
            if last_probe.elapsed() >= Duration::from_millis(PROBE_INTERVAL_MS) {
                p2p_udp::encode_probe(
                    Role::Client,
                    session_id,
                    nonce,
                    sequence,
                    now_epoch_ms(),
                    &mut packet,
                );
                let _ = socket.send_to(&packet, peer);
                sequence = sequence.saturating_add(1);
                last_probe = Instant::now();
            }
        }

        match socket.recv_from(&mut buf) {
            Ok((len, from)) => match p2p_udp::decode(&buf[..len]) {
                Ok(p2p_udp::Packet::Probe {
                    session_id: packet_session_id,
                    nonce: packet_nonce,
                    sequence,
                    sent_epoch_ms,
                    ..
                }) if packet_session_id == session_id && packet_nonce == nonce => {
                    p2p_udp::encode_ack(
                        Role::Client,
                        session_id,
                        nonce,
                        sequence,
                        sent_epoch_ms,
                        &mut packet,
                    );
                    let _ = socket.send_to(&packet, from);
                    if !success_reported {
                        success_reported = true;
                        report(
                            &out_tx,
                            &session_token,
                            &client_id,
                            session_id,
                            true,
                            true,
                            &from.to_string(),
                            rtt_ms(sent_epoch_ms),
                            format!("direct probe received from {from}"),
                        );
                    }
                }
                Ok(p2p_udp::Packet::Ack {
                    session_id: packet_session_id,
                    nonce: packet_nonce,
                    sent_epoch_ms,
                    ..
                }) if packet_session_id == session_id && packet_nonce == nonce => {
                    if !success_reported {
                        success_reported = true;
                        report(
                            &out_tx,
                            &session_token,
                            &client_id,
                            session_id,
                            true,
                            true,
                            &from.to_string(),
                            rtt_ms(sent_epoch_ms),
                            format!("direct ack received from {from}"),
                        );
                    }
                }
                _ => {}
            },
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                report(
                    &out_tx,
                    &session_token,
                    &client_id,
                    session_id,
                    false,
                    true,
                    &local_endpoint,
                    0,
                    format!("p2p udp receive failed: {error}"),
                );
                return;
            }
        }

        if success_reported {
            thread::sleep(Duration::from_millis(150));
            return;
        }
    }

    if stop.load(Ordering::Relaxed) {
        report(
            &out_tx,
            &session_token,
            &client_id,
            session_id,
            false,
            true,
            &local_endpoint,
            0,
            "p2p test stopped".to_string(),
        );
    } else {
        report(
            &out_tx,
            &session_token,
            &client_id,
            session_id,
            false,
            true,
            &local_endpoint,
            0,
            "p2p direct probe timed out".to_string(),
        );
    }
}

fn report(
    out_tx: &SyncSender<ClientOutbound>,
    session_token: &str,
    client_id: &str,
    session_id: u64,
    success: bool,
    finished: bool,
    endpoint: impl Into<String>,
    rtt_ms: u32,
    detail: String,
) {
    let _ = queue_message(
        out_tx,
        session_token,
        Message::P2pResult {
            client_id: client_id.to_string(),
            session_id,
            success,
            finished,
            endpoint: endpoint.into(),
            rtt_ms,
            detail,
        },
    );
}

fn resolve_server_addr(advertised: &str, fallback: &str) -> Result<SocketAddr, String> {
    let candidate = if advertised_addr_is_usable(advertised) {
        advertised.trim()
    } else {
        fallback.trim()
    };
    candidate
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or_else(|| format!("{candidate} resolved to no socket addresses"))
}

fn advertised_addr_is_usable(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.starts_with("0.0.0.0:")
        && !value.starts_with("[::]:")
        && !value.starts_with(":::")
}

fn rtt_ms(sent_epoch_ms: u128) -> u32 {
    now_epoch_ms()
        .saturating_sub(sent_epoch_ms)
        .min(u128::from(u32::MAX)) as u32
}

use crate::outbound::{queue_message, ClientOutbound};
use rdl_protocol::Message;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc,
};
use std::time::Duration;

const PROXY_BUFFER_BYTES: usize = 16 * 1024;
const PROXY_WRITE_POLL_MS: u64 = 50;

pub(crate) struct ClientProxyStream {
    pub(crate) data_tx: Sender<Vec<u8>>,
    pub(crate) stop: Arc<AtomicBool>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn client_proxy_stream_loop(
    client_id: String,
    stream_id: u64,
    host: String,
    port: u16,
    data_rx: Receiver<Vec<u8>>,
    out_tx: SyncSender<ClientOutbound>,
    bulk_out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stop: Arc<AtomicBool>,
    done_tx: Sender<u64>,
) {
    let addr_label = format!("{host}:{port}");
    let mut target = match TcpStream::connect((host.as_str(), port)) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::ProxyOpenResult {
                    client_id,
                    stream_id,
                    accepted: false,
                    detail: format!("connect {addr_label} failed: {error}"),
                },
            );
            let _ = done_tx.send(stream_id);
            return;
        }
    };
    let reader = match target.try_clone() {
        Ok(reader) => reader,
        Err(error) => {
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::ProxyOpenResult {
                    client_id,
                    stream_id,
                    accepted: false,
                    detail: format!("clone target stream failed: {error}"),
                },
            );
            let _ = done_tx.send(stream_id);
            return;
        }
    };
    let _ = target.set_nodelay(true);
    let _ = reader.set_nodelay(true);

    if queue_message(
        &out_tx,
        &session_token,
        Message::ProxyOpenResult {
            client_id: client_id.clone(),
            stream_id,
            accepted: true,
            detail: format!("connected {addr_label}"),
        },
    )
    .is_err()
    {
        let _ = done_tx.send(stream_id);
        return;
    }

    let close_sent = Arc::new(AtomicBool::new(false));
    let reader_close_sent = close_sent.clone();
    let reader_client_id = client_id.clone();
    let reader_token = session_token.clone();
    let reader_stop = stop.clone();
    let reader_done_tx = done_tx.clone();
    std::thread::spawn(move || {
        client_proxy_target_reader_loop(
            reader,
            reader_client_id,
            stream_id,
            bulk_out_tx,
            reader_token,
            reader_stop,
            reader_close_sent,
            reader_done_tx,
        );
    });

    while !stop.load(Ordering::Relaxed) {
        match data_rx.recv_timeout(Duration::from_millis(PROXY_WRITE_POLL_MS)) {
            Ok(bytes) => {
                if let Err(error) = target.write_all(&bytes) {
                    send_proxy_close_once(
                        &out_tx,
                        &session_token,
                        &client_id,
                        stream_id,
                        format!("target write failed: {error}"),
                        &close_sent,
                    );
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = target.shutdown(Shutdown::Both);
    send_proxy_close_once(
        &out_tx,
        &session_token,
        &client_id,
        stream_id,
        "proxy stream closed".to_string(),
        &close_sent,
    );
    let _ = done_tx.send(stream_id);
}

#[allow(clippy::too_many_arguments)]
fn client_proxy_target_reader_loop(
    mut target: TcpStream,
    client_id: String,
    stream_id: u64,
    bulk_out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stop: Arc<AtomicBool>,
    close_sent: Arc<AtomicBool>,
    done_tx: Sender<u64>,
) {
    let mut buffer = [0_u8; PROXY_BUFFER_BYTES];
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match target.read(&mut buffer) {
            Ok(0) => {
                send_proxy_close_once(
                    &bulk_out_tx,
                    &session_token,
                    &client_id,
                    stream_id,
                    "target closed".to_string(),
                    &close_sent,
                );
                break;
            }
            Ok(len) => {
                if queue_message(
                    &bulk_out_tx,
                    &session_token,
                    Message::ProxyData {
                        client_id: client_id.clone(),
                        stream_id,
                        bytes: buffer[..len].to_vec(),
                    },
                )
                .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                send_proxy_close_once(
                    &bulk_out_tx,
                    &session_token,
                    &client_id,
                    stream_id,
                    format!("target read failed: {error}"),
                    &close_sent,
                );
                break;
            }
        }
    }
    let _ = target.shutdown(Shutdown::Both);
    let _ = done_tx.send(stream_id);
}

fn send_proxy_close_once(
    out_tx: &SyncSender<ClientOutbound>,
    session_token: &str,
    client_id: &str,
    stream_id: u64,
    reason: String,
    close_sent: &AtomicBool,
) {
    if close_sent.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = queue_message(
        out_tx,
        session_token,
        Message::ProxyClose {
            client_id: client_id.to_string(),
            stream_id,
            reason,
        },
    );
}

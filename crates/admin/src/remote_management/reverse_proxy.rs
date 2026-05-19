use crate::{
    app::event::AdminInput,
    i18n::{t, tf},
    theme::{
        COLOR_BAD, COLOR_GOOD, COLOR_WARN, COMPACT_CONTROL_HEIGHT, PANEL_MARGIN, SECTION_GAP,
        TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT,
    },
    windowing,
};
use eframe::egui;
use egui_extras::Column;
use rdl_protocol::{now_epoch_ms, Message};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_LISTEN_IP: &str = "127.0.0.1";
const DEFAULT_LISTEN_PORT: &str = "5269";
const DEFAULT_TEST_TARGET: &str = "www.cloudflare.com:80";
const SOCKS_CONNECT_TIMEOUT_MS: u64 = 10_000;
const LISTENER_POLL_MS: u64 = 50;
const PROXY_BUFFER_BYTES: usize = 16 * 1024;
const TEST_CONNECT_TIMEOUT_MS: u64 = 10_000;
const TEST_DETAIL_WAIT_MS: u64 = 800;
const TEST_DETAIL_POLL_MS: u64 = 25;
const MAX_PRE_OPEN_FAILURES: usize = 16;
const SOCKS_REPLY_SUCCEEDED: u8 = 0x00;
const SOCKS_REPLY_GENERAL_FAILURE: u8 = 0x01;
const SOCKS_REPLY_HOST_UNREACHABLE: u8 = 0x04;
const SOCKS_REPLY_CONNECTION_REFUSED: u8 = 0x05;
const SOCKS_REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const SOCKS_REPLY_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;

pub(crate) struct ReverseProxyWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    listen_ip: Arc<Mutex<String>>,
    listen_port: Arc<Mutex<String>>,
    test_target: Arc<Mutex<String>>,
    status: Arc<Mutex<ProxyStatus>>,
    notice: Arc<Mutex<String>>,
    streams: Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    pre_open_failures: Arc<Mutex<Vec<ProxyPreOpenFailure>>>,
    selected_stream: Arc<Mutex<Option<u64>>>,
    next_stream_id: Arc<AtomicU64>,
    listener_stop: Option<Arc<AtomicBool>>,
    start_requested: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    clear_closed_requested: Arc<AtomicBool>,
    kill_selected_requested: Arc<AtomicBool>,
    test_requested: Arc<AtomicBool>,
    test_in_flight: Arc<AtomicBool>,
    close_requested: Arc<AtomicBool>,
    open: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ProxyStatus {
    Stopped,
    Starting,
    Listening,
    Stopping,
    Error,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ProxyStreamStatus {
    Opening,
    Open,
    Closing,
    Closed,
    Failed,
}

struct ProxyStreamState {
    target: String,
    status: ProxyStreamStatus,
    tx_bytes: u64,
    rx_bytes: u64,
    started_at: Instant,
    closed_at: Option<Instant>,
    detail: String,
    inbound_tx: Sender<ProxyInbound>,
}

struct ProxyPreOpenFailure {
    occurred_at: Instant,
    detail: String,
}

enum ProxyInbound {
    OpenResult { accepted: bool, detail: String },
    Data(Vec<u8>),
    Close(String),
}

pub(crate) fn open_window(
    windows: &mut Vec<ReverseProxyWindow>,
    client_id: &str,
    hostname: String,
    username: String,
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    {
        window.open = true;
        window.hostname = hostname;
        window.username = username;
        window.close_requested.store(false, Ordering::Relaxed);
        return;
    }

    windows.push(ReverseProxyWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        listen_ip: Arc::new(Mutex::new(DEFAULT_LISTEN_IP.to_string())),
        listen_port: Arc::new(Mutex::new(DEFAULT_LISTEN_PORT.to_string())),
        test_target: Arc::new(Mutex::new(DEFAULT_TEST_TARGET.to_string())),
        status: Arc::new(Mutex::new(ProxyStatus::Stopped)),
        notice: Arc::new(Mutex::new(t("Stopped").to_string())),
        streams: Arc::new(Mutex::new(HashMap::new())),
        pre_open_failures: Arc::new(Mutex::new(Vec::new())),
        selected_stream: Arc::new(Mutex::new(None)),
        next_stream_id: Arc::new(AtomicU64::new(initial_stream_id())),
        listener_stop: None,
        start_requested: Arc::new(AtomicBool::new(false)),
        stop_requested: Arc::new(AtomicBool::new(false)),
        clear_closed_requested: Arc::new(AtomicBool::new(false)),
        kill_selected_requested: Arc::new(AtomicBool::new(false)),
        test_requested: Arc::new(AtomicBool::new(false)),
        test_in_flight: Arc::new(AtomicBool::new(false)),
        close_requested: Arc::new(AtomicBool::new(false)),
        open: true,
    });
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<ReverseProxyWindow>,
    input_tx: &SyncSender<AdminInput>,
) {
    for window in windows.iter_mut() {
        if window.close_requested.swap(false, Ordering::Relaxed) {
            stop_window(window, input_tx, true);
            window.open = false;
        }
        if !window.open {
            continue;
        }

        let client_id = window.client_id.clone();
        let title = format!(
            "{} - {}",
            t("Reverse Proxy"),
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("admin_reverse_proxy", &client_id));
        let builder = windowing::child_viewport_builder(title, [720.0, 500.0], [480.0, 340.0]);

        let listen_ip = window.listen_ip.clone();
        let listen_port = window.listen_port.clone();
        let test_target = window.test_target.clone();
        let status = window.status.clone();
        let notice = window.notice.clone();
        let streams = window.streams.clone();
        let selected_stream = window.selected_stream.clone();
        let start_requested = window.start_requested.clone();
        let stop_requested = window.stop_requested.clone();
        let clear_closed_requested = window.clear_closed_requested.clone();
        let kill_selected_requested = window.kill_selected_requested.clone();
        let test_requested = window.test_requested.clone();
        let test_in_flight = window.test_in_flight.clone();
        let close_requested = window.close_requested.clone();
        let identity = identity_title(&window.hostname, &window.username);

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(crate::theme::page_frame())
                .show_inside(ui, |ui| {
                    windowing::render_child_window_controls(ui);
                    render_toolbar(
                        ui,
                        &listen_ip,
                        &listen_port,
                        &status,
                        &start_requested,
                        &stop_requested,
                    );
                    ui.add_space(SECTION_GAP);
                    render_endpoint(
                        ui,
                        &identity,
                        &listen_ip,
                        &listen_port,
                        &test_target,
                        &status,
                        &test_requested,
                        &test_in_flight,
                    );
                    ui.add_space(SECTION_GAP);
                    let status_reserved_height =
                        crate::theme::CONTROL_HEIGHT + PANEL_MARGIN * 2.0 + SECTION_GAP;
                    let connections_height =
                        (ui.available_height() - status_reserved_height).max(0.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), connections_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            render_connections(
                                ui,
                                &streams,
                                &selected_stream,
                                &clear_closed_requested,
                                &kill_selected_requested,
                            );
                        },
                    );
                    ui.add_space(SECTION_GAP);
                    render_status_bar(ui, &status, &notice);
                });
        });

        if window.start_requested.swap(false, Ordering::Relaxed) {
            start_window(window, input_tx.clone());
        }
        if window.stop_requested.swap(false, Ordering::Relaxed) {
            stop_window(window, input_tx, true);
        }
        if window.clear_closed_requested.swap(false, Ordering::Relaxed) {
            clear_closed_streams(window);
        }
        if window
            .kill_selected_requested
            .swap(false, Ordering::Relaxed)
        {
            kill_selected_stream(window, input_tx);
        }
        if window.test_requested.swap(false, Ordering::Relaxed) {
            start_test_connection(window, ctx.clone());
        }
    }

    windows.retain(|window| window.open || proxy_is_running(window));
}

pub(crate) fn handle_open_result(
    windows: &mut [ReverseProxyWindow],
    client_id: &str,
    stream_id: u64,
    accepted: bool,
    detail: String,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if let Ok(mut streams) = window.streams.lock() {
        if let Some(stream) = streams.get_mut(&stream_id) {
            let detail = proxy_detail_text(&detail);
            stream.status = if accepted {
                ProxyStreamStatus::Open
            } else {
                ProxyStreamStatus::Failed
            };
            stream.detail = detail.clone();
            if !accepted {
                stream.closed_at = Some(Instant::now());
            }
            let _ = stream
                .inbound_tx
                .send(ProxyInbound::OpenResult { accepted, detail });
        }
    }
}

pub(crate) fn handle_data(
    windows: &mut [ReverseProxyWindow],
    client_id: &str,
    stream_id: u64,
    bytes: Vec<u8>,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    if let Ok(mut streams) = window.streams.lock() {
        if let Some(stream) = streams.get_mut(&stream_id) {
            stream.rx_bytes = stream.rx_bytes.saturating_add(bytes.len() as u64);
            let _ = stream.inbound_tx.send(ProxyInbound::Data(bytes));
        }
    }
}

pub(crate) fn handle_close(
    windows: &mut [ReverseProxyWindow],
    client_id: &str,
    stream_id: u64,
    reason: String,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };
    mark_stream_closed(
        &window.streams,
        stream_id,
        ProxyStreamStatus::Closed,
        &proxy_detail_text(&reason),
    );
}

pub(crate) fn stop_all(windows: &mut [ReverseProxyWindow]) {
    for window in windows {
        stop_window_local(window, true);
    }
}

fn start_window(window: &mut ReverseProxyWindow, input_tx: SyncSender<AdminInput>) {
    if proxy_is_running(window) {
        set_notice(&window.notice, t("Already listening"));
        return;
    }
    set_status(&window.status, ProxyStatus::Starting);
    let listen_ip = locked_string(&window.listen_ip);
    let listen_port = locked_string(&window.listen_port);
    let port = match listen_port.trim().parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => {
            set_status(&window.status, ProxyStatus::Error);
            set_notice(&window.notice, t("Invalid listen port"));
            return;
        }
    };
    let addr = format!("{}:{port}", listen_ip.trim());
    let listener = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(error) => {
            set_status(&window.status, ProxyStatus::Error);
            let error = error.to_string();
            set_notice(
                &window.notice,
                tf("Listen failed: {error}", &[("error", error.as_str())]),
            );
            return;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        set_status(&window.status, ProxyStatus::Error);
        let error = error.to_string();
        set_notice(
            &window.notice,
            tf(
                "Listener setup failed: {error}",
                &[("error", error.as_str())],
            ),
        );
        return;
    }

    let stop = Arc::new(AtomicBool::new(false));
    window.listener_stop = Some(stop.clone());
    set_status(&window.status, ProxyStatus::Listening);
    set_notice(
        &window.notice,
        tf("Listening on {addr}", &[("addr", addr.as_str())]),
    );
    if let Ok(mut failures) = window.pre_open_failures.lock() {
        failures.clear();
    }

    let worker = ListenerContext {
        client_id: window.client_id.clone(),
        input_tx,
        streams: window.streams.clone(),
        pre_open_failures: window.pre_open_failures.clone(),
        next_stream_id: window.next_stream_id.clone(),
        status: window.status.clone(),
        notice: window.notice.clone(),
        stop,
    };
    thread::spawn(move || listener_loop(listener, worker));
}

fn stop_window(
    window: &mut ReverseProxyWindow,
    input_tx: &SyncSender<AdminInput>,
    notify_remote: bool,
) {
    if notify_remote {
        close_all_remote_streams(window, input_tx, "proxy stopped");
    }
    stop_window_local(window, notify_remote);
}

fn stop_window_local(window: &mut ReverseProxyWindow, set_stopped: bool) {
    set_status(&window.status, ProxyStatus::Stopping);
    if let Some(stop) = window.listener_stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    if let Ok(mut streams) = window.streams.lock() {
        for stream in streams.values_mut() {
            if matches!(
                stream.status,
                ProxyStreamStatus::Open | ProxyStreamStatus::Opening
            ) {
                stream.status = ProxyStreamStatus::Closed;
                stream.closed_at = Some(Instant::now());
                stream.detail = t("Proxy stopped").to_string();
                let _ = stream
                    .inbound_tx
                    .send(ProxyInbound::Close(t("Proxy stopped").to_string()));
            }
        }
    }
    if set_stopped {
        set_status(&window.status, ProxyStatus::Stopped);
        set_notice(&window.notice, t("Stopped"));
    }
}

fn clear_closed_streams(window: &mut ReverseProxyWindow) {
    if let Ok(mut streams) = window.streams.lock() {
        streams.retain(|_, stream| {
            !matches!(
                stream.status,
                ProxyStreamStatus::Closed | ProxyStreamStatus::Failed
            )
        });
    }
}

fn kill_selected_stream(window: &mut ReverseProxyWindow, input_tx: &SyncSender<AdminInput>) {
    let selected = window.selected_stream.lock().ok().and_then(|value| *value);
    let Some(stream_id) = selected else {
        return;
    };
    let _ = input_tx.send(AdminInput::Proxy(Message::ProxyClose {
        client_id: window.client_id.clone(),
        stream_id,
        reason: "killed by admin".to_string(),
    }));
    mark_stream_closed(
        &window.streams,
        stream_id,
        ProxyStreamStatus::Closing,
        t("Killed by admin"),
    );
}

fn start_test_connection(window: &ReverseProxyWindow, egui_ctx: egui::Context) {
    if !matches!(locked_status(&window.status), ProxyStatus::Listening) {
        set_notice(&window.notice, t("Start proxy before testing"));
        return;
    }
    if window.test_in_flight.swap(true, Ordering::Relaxed) {
        return;
    }

    let listen_ip = locked_string(&window.listen_ip);
    let listen_port = locked_string(&window.listen_port);
    let target = match parse_test_target(&locked_string(&window.test_target)) {
        Ok(target) => target,
        Err(error) => {
            window.test_in_flight.store(false, Ordering::Relaxed);
            set_notice(&window.notice, error);
            return;
        }
    };
    set_notice(
        &window.notice,
        tf(
            "Testing connection to {target}",
            &[("target", target.label.as_str())],
        ),
    );
    egui_ctx.request_repaint();

    let notice = window.notice.clone();
    let streams = window.streams.clone();
    let pre_open_failures = window.pre_open_failures.clone();
    let test_in_flight = window.test_in_flight.clone();
    let started_at = Instant::now();
    thread::spawn(move || {
        let result = run_proxy_test(&listen_ip, &listen_port, &target);
        match result {
            Ok(()) => set_notice(
                &notice,
                tf(
                    "Test succeeded: {target}",
                    &[("target", target.label.as_str())],
                ),
            ),
            Err(error) => {
                let detail = wait_recent_test_failure_detail(
                    &streams,
                    &pre_open_failures,
                    &target.stream_target,
                    started_at,
                    Duration::from_millis(TEST_DETAIL_WAIT_MS),
                )
                .unwrap_or(error);
                set_notice(
                    &notice,
                    tf("Test failed: {error}", &[("error", detail.as_str())]),
                );
            }
        }
        test_in_flight.store(false, Ordering::Relaxed);
        egui_ctx.request_repaint();
    });
}

fn run_proxy_test(listen_ip: &str, listen_port: &str, target: &TestTarget) -> Result<(), String> {
    let port = listen_port
        .trim()
        .parse::<u16>()
        .map_err(|_| t("Invalid listen port").to_string())?;
    let connect_host = proxy_connect_host(listen_ip);
    let proxy_addr = (connect_host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| {
            tf(
                "Resolve proxy endpoint failed: {error}",
                &[("error", error.to_string().as_str())],
            )
        })?
        .next()
        .ok_or_else(|| t("Proxy endpoint has no address").to_string())?;
    let timeout = Duration::from_millis(TEST_CONNECT_TIMEOUT_MS);
    let mut socket = TcpStream::connect_timeout(&proxy_addr, timeout).map_err(|error| {
        tf(
            "Connect proxy endpoint failed: {error}",
            &[("error", error.to_string().as_str())],
        )
    })?;
    let _ = socket.set_read_timeout(Some(timeout));
    let _ = socket.set_write_timeout(Some(timeout));

    socket.write_all(&[0x05, 0x01, 0x00]).map_err(|error| {
        tf(
            "SOCKS handshake failed: {error}",
            &[("error", error.to_string().as_str())],
        )
    })?;
    let mut method_reply = [0_u8; 2];
    socket.read_exact(&mut method_reply).map_err(|error| {
        tf(
            "SOCKS handshake failed: {error}",
            &[("error", error.to_string().as_str())],
        )
    })?;
    if method_reply != [0x05, 0x00] {
        return Err(t("SOCKS authentication method rejected").to_string());
    }

    let host_bytes = target.host.as_bytes();
    if host_bytes.len() > u8::MAX as usize {
        return Err(t("Test target host is too long").to_string());
    }
    let mut request = Vec::with_capacity(7 + host_bytes.len());
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8]);
    request.extend_from_slice(host_bytes);
    request.extend_from_slice(&target.port.to_be_bytes());
    socket.write_all(&request).map_err(|error| {
        tf(
            "SOCKS connect request failed: {error}",
            &[("error", error.to_string().as_str())],
        )
    })?;

    let mut reply = [0_u8; 4];
    socket.read_exact(&mut reply).map_err(|error| {
        tf(
            "SOCKS connect reply failed: {error}",
            &[("error", error.to_string().as_str())],
        )
    })?;
    if reply[0] != 0x05 {
        return Err(t("Invalid SOCKS reply").to_string());
    }
    if reply[1] != SOCKS_REPLY_SUCCEEDED {
        let code = socks_reply_code_text(reply[1]);
        return Err(tf(
            "SOCKS connect failed: {code}",
            &[("code", code.as_str())],
        ));
    }
    read_socks5_bound_address(&mut socket, reply[3])?;
    let _ = socket.shutdown(Shutdown::Both);
    debug_log!("debug event=proxy_test_ok target={}", target.label);
    Ok(())
}

#[derive(Clone)]
struct TestTarget {
    host: String,
    port: u16,
    label: String,
    stream_target: String,
}

fn parse_test_target(value: &str) -> Result<TestTarget, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(t("Test target is required").to_string());
    }

    let (host, port_text) = if let Some(rest) = value.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Err(t("Test target must be host:port").to_string());
        };
        let host = &rest[..end];
        let rest = &rest[end + 1..];
        let Some(port_text) = rest.strip_prefix(':') else {
            return Err(t("Test target must be host:port").to_string());
        };
        (host.trim(), port_text.trim())
    } else {
        let Some((host, port_text)) = value.rsplit_once(':') else {
            return Err(t("Test target must be host:port").to_string());
        };
        if host.contains(':') {
            return Err(t("Use brackets for IPv6 test targets").to_string());
        }
        (host.trim(), port_text.trim())
    };

    if host.is_empty() {
        return Err(t("Test target is required").to_string());
    }
    if host.len() > u8::MAX as usize {
        return Err(t("Test target host is too long").to_string());
    }
    let port = port_text
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| t("Invalid test target port").to_string())?;
    let label = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Ok(TestTarget {
        host: host.to_string(),
        port,
        label,
        stream_target: format!("{host}:{port}"),
    })
}

fn recent_test_stream_detail(
    streams: &Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    target: &str,
    started_at: Instant,
) -> Option<String> {
    let streams = streams.lock().ok()?;
    streams
        .values()
        .filter(|stream| stream.target == target && stream.started_at >= started_at)
        .max_by_key(|stream| stream.started_at)
        .and_then(|stream| {
            let detail = proxy_detail_text(&stream.detail);
            (!detail.trim().is_empty()).then_some(detail)
        })
}

fn wait_recent_test_failure_detail(
    streams: &Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    pre_open_failures: &Arc<Mutex<Vec<ProxyPreOpenFailure>>>,
    target: &str,
    started_at: Instant,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(detail) = recent_test_stream_detail(streams, target, started_at) {
            return Some(detail);
        }
        if let Some(detail) = recent_pre_open_failure_detail(pre_open_failures, started_at) {
            return Some(detail);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(TEST_DETAIL_POLL_MS));
    }
}

fn recent_pre_open_failure_detail(
    failures: &Arc<Mutex<Vec<ProxyPreOpenFailure>>>,
    started_at: Instant,
) -> Option<String> {
    let failures = failures.lock().ok()?;
    failures
        .iter()
        .filter(|failure| failure.occurred_at >= started_at)
        .max_by_key(|failure| failure.occurred_at)
        .map(|failure| failure.detail.clone())
}

fn proxy_connect_host(listen_ip: &str) -> String {
    match listen_ip.trim() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_string(),
        value => value.trim_matches(['[', ']']).to_string(),
    }
}

fn socks_reply_code_text(code: u8) -> String {
    let label = match code {
        SOCKS_REPLY_SUCCEEDED => t("Succeeded"),
        SOCKS_REPLY_GENERAL_FAILURE => t("General SOCKS server failure"),
        0x02 => t("Connection not allowed by ruleset"),
        0x03 => t("Network unreachable"),
        SOCKS_REPLY_HOST_UNREACHABLE => t("Host unreachable"),
        SOCKS_REPLY_CONNECTION_REFUSED => t("Connection refused"),
        0x06 => t("TTL expired"),
        SOCKS_REPLY_COMMAND_NOT_SUPPORTED => t("Command not supported"),
        SOCKS_REPLY_ADDRESS_TYPE_NOT_SUPPORTED => t("Address type not supported"),
        _ => t("Unassigned SOCKS reply code"),
    };
    format!("0x{code:02x} {label}")
}

fn read_socks5_bound_address(socket: &mut TcpStream, atyp: u8) -> Result<(), String> {
    match atyp {
        0x01 => {
            let mut raw = [0_u8; 4];
            socket.read_exact(&mut raw).map_err(|error| {
                tf(
                    "SOCKS connect reply failed: {error}",
                    &[("error", error.to_string().as_str())],
                )
            })?;
        }
        0x03 => {
            let mut len = [0_u8; 1];
            socket.read_exact(&mut len).map_err(|error| {
                tf(
                    "SOCKS connect reply failed: {error}",
                    &[("error", error.to_string().as_str())],
                )
            })?;
            let mut raw = vec![0_u8; len[0] as usize];
            socket.read_exact(&mut raw).map_err(|error| {
                tf(
                    "SOCKS connect reply failed: {error}",
                    &[("error", error.to_string().as_str())],
                )
            })?;
        }
        0x04 => {
            let mut raw = [0_u8; 16];
            socket.read_exact(&mut raw).map_err(|error| {
                tf(
                    "SOCKS connect reply failed: {error}",
                    &[("error", error.to_string().as_str())],
                )
            })?;
        }
        _ => return Err(t("Unsupported SOCKS reply address type").to_string()),
    }
    let mut port = [0_u8; 2];
    socket.read_exact(&mut port).map_err(|error| {
        tf(
            "SOCKS connect reply failed: {error}",
            &[("error", error.to_string().as_str())],
        )
    })?;
    Ok(())
}

fn close_all_remote_streams(
    window: &ReverseProxyWindow,
    input_tx: &SyncSender<AdminInput>,
    reason: &str,
) {
    let stream_ids = window
        .streams
        .lock()
        .map(|streams| {
            streams
                .iter()
                .filter_map(|(stream_id, stream)| {
                    matches!(
                        stream.status,
                        ProxyStreamStatus::Opening | ProxyStreamStatus::Open
                    )
                    .then_some(*stream_id)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for stream_id in stream_ids {
        let _ = input_tx.send(AdminInput::Proxy(Message::ProxyClose {
            client_id: window.client_id.clone(),
            stream_id,
            reason: reason.to_string(),
        }));
    }
}

fn listener_loop(listener: TcpListener, ctx: ListenerContext) {
    while !ctx.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((socket, _addr)) => {
                if let Err(error) = socket.set_nonblocking(false) {
                    let detail = tf(
                        "SOCKS request failed: {error}",
                        &[("error", error.to_string().as_str())],
                    );
                    remember_pre_open_failure(&ctx.pre_open_failures, detail.clone());
                    set_notice(&ctx.notice, detail);
                    continue;
                }
                let connection = ctx.clone();
                thread::spawn(move || handle_socks_connection(socket, connection));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(LISTENER_POLL_MS));
            }
            Err(error) => {
                set_status(&ctx.status, ProxyStatus::Error);
                let error = error.to_string();
                set_notice(
                    &ctx.notice,
                    tf("Accept failed: {error}", &[("error", error.as_str())]),
                );
                break;
            }
        }
    }
    if !matches!(locked_status(&ctx.status), ProxyStatus::Error) {
        set_status(&ctx.status, ProxyStatus::Stopped);
        set_notice(&ctx.notice, t("Stopped"));
    }
}

#[derive(Clone)]
struct ListenerContext {
    client_id: String,
    input_tx: SyncSender<AdminInput>,
    streams: Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    pre_open_failures: Arc<Mutex<Vec<ProxyPreOpenFailure>>>,
    next_stream_id: Arc<AtomicU64>,
    status: Arc<Mutex<ProxyStatus>>,
    notice: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

fn handle_socks_connection(mut socket: TcpStream, ctx: ListenerContext) {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(SOCKS_CONNECT_TIMEOUT_MS)));
    let (host, port) = match socks5_connect(&mut socket) {
        Ok(target) => target,
        Err(error) => {
            let detail = tf(
                "SOCKS request failed: {error}",
                &[("error", error.detail.as_str())],
            );
            remember_pre_open_failure(&ctx.pre_open_failures, detail.clone());
            set_notice(&ctx.notice, detail.clone());
            if let Some(reply) = error.reply {
                let _ = send_socks5_failure(&mut socket, reply);
            }
            debug_log!("debug event=proxy_socks_failed error={detail}");
            return;
        }
    };
    let _ = socket.set_read_timeout(None);
    let target = format!("{host}:{port}");
    let stream_id = ctx.next_stream_id.fetch_add(1, Ordering::Relaxed).max(1);
    let (inbound_tx, inbound_rx) = mpsc::channel::<ProxyInbound>();
    if let Ok(mut streams) = ctx.streams.lock() {
        streams.insert(
            stream_id,
            ProxyStreamState {
                target: target.clone(),
                status: ProxyStreamStatus::Opening,
                tx_bytes: 0,
                rx_bytes: 0,
                started_at: Instant::now(),
                closed_at: None,
                detail: String::new(),
                inbound_tx,
            },
        );
    }

    if ctx
        .input_tx
        .send(AdminInput::Proxy(Message::ProxyOpen {
            target_id: ctx.client_id.clone(),
            stream_id,
            host,
            port,
        }))
        .is_err()
    {
        let detail = t("Admin network queue is closed").to_string();
        remember_pre_open_failure(&ctx.pre_open_failures, detail.clone());
        mark_stream_closed(&ctx.streams, stream_id, ProxyStreamStatus::Failed, &detail);
        let _ = send_socks5_failure(&mut socket, SOCKS_REPLY_GENERAL_FAILURE);
        return;
    }

    match wait_for_open_result(&inbound_rx) {
        Ok(()) => {
            if send_socks5_success(&mut socket).is_err() {
                mark_stream_closed(
                    &ctx.streams,
                    stream_id,
                    ProxyStreamStatus::Failed,
                    t("Local SOCKS client disconnected"),
                );
                return;
            }
        }
        Err(error) => {
            let _ = send_socks5_failure(&mut socket, socks_reply_for_proxy_error(&error));
            let _ = ctx.input_tx.send(AdminInput::Proxy(Message::ProxyClose {
                client_id: ctx.client_id.clone(),
                stream_id,
                reason: error.clone(),
            }));
            mark_stream_closed(&ctx.streams, stream_id, ProxyStreamStatus::Failed, &error);
            return;
        }
    }

    let writer = match socket.try_clone() {
        Ok(writer) => writer,
        Err(error) => {
            let error = error.to_string();
            let detail = tf(
                "Clone local socket failed: {error}",
                &[("error", error.as_str())],
            );
            mark_stream_closed(&ctx.streams, stream_id, ProxyStreamStatus::Failed, &detail);
            return;
        }
    };
    let close_sent = Arc::new(AtomicBool::new(false));
    let writer_streams = ctx.streams.clone();
    let writer_close_sent = close_sent.clone();
    thread::spawn(move || {
        local_writer_loop(
            writer,
            inbound_rx,
            writer_streams,
            stream_id,
            writer_close_sent,
        );
    });

    let mut buffer = [0_u8; PROXY_BUFFER_BYTES];
    loop {
        match socket.read(&mut buffer) {
            Ok(0) => {
                send_proxy_close_once(
                    &ctx.input_tx,
                    &ctx.client_id,
                    stream_id,
                    t("Local socket closed"),
                    &close_sent,
                );
                break;
            }
            Ok(len) => {
                add_stream_tx_bytes(&ctx.streams, stream_id, len as u64);
                if ctx
                    .input_tx
                    .send(AdminInput::Proxy(Message::ProxyData {
                        client_id: ctx.client_id.clone(),
                        stream_id,
                        bytes: buffer[..len].to_vec(),
                    }))
                    .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                let error = error.to_string();
                let reason = tf("Local read failed: {error}", &[("error", error.as_str())]);
                send_proxy_close_once(
                    &ctx.input_tx,
                    &ctx.client_id,
                    stream_id,
                    &reason,
                    &close_sent,
                );
                break;
            }
        }
    }
    let _ = socket.shutdown(Shutdown::Both);
    mark_stream_closed(
        &ctx.streams,
        stream_id,
        ProxyStreamStatus::Closed,
        t("Local side closed"),
    );
}

fn wait_for_open_result(inbound_rx: &Receiver<ProxyInbound>) -> Result<(), String> {
    match inbound_rx.recv_timeout(Duration::from_millis(SOCKS_CONNECT_TIMEOUT_MS)) {
        Ok(ProxyInbound::OpenResult { accepted: true, .. }) => Ok(()),
        Ok(ProxyInbound::OpenResult {
            accepted: false,
            detail,
        }) => Err(proxy_detail_text(&detail)),
        Ok(ProxyInbound::Close(reason)) => Err(proxy_detail_text(&reason)),
        Ok(ProxyInbound::Data(_)) => Err(t("Proxy data arrived before open result").to_string()),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(t("Proxy open timed out").to_string()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(t("Proxy open result channel closed").to_string())
        }
    }
}

fn proxy_detail_text(detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        return String::new();
    }
    match detail {
        "killed by admin" => t("Killed by admin").to_string(),
        "proxy stream is not open" => t("Proxy stream is not open").to_string(),
        "proxy stream closed" => t("Proxy stream closed").to_string(),
        "proxy stopped" => t("Proxy stopped").to_string(),
        "proxy target writer stopped" => t("Proxy target writer stopped").to_string(),
        "target closed" => t("Target closed").to_string(),
        _ => {
            if let Some(target) = detail.strip_prefix("connected ") {
                return tf("Connected {target}", &[("target", target)]);
            }
            if let Some(rest) = detail.strip_prefix("connect ") {
                if let Some((target, error)) = rest.split_once(" failed: ") {
                    return tf(
                        "Connect target failed: {target} ({error})",
                        &[("target", target), ("error", error)],
                    );
                }
            }
            if let Some(error) = detail.strip_prefix("clone target stream failed: ") {
                return tf("Clone target stream failed: {error}", &[("error", error)]);
            }
            if let Some(error) = detail.strip_prefix("target read failed: ") {
                return tf("Target read failed: {error}", &[("error", error)]);
            }
            if let Some(error) = detail.strip_prefix("target write failed: ") {
                return tf("Target write failed: {error}", &[("error", error)]);
            }
            detail.to_string()
        }
    }
}

fn local_writer_loop(
    mut socket: TcpStream,
    inbound_rx: Receiver<ProxyInbound>,
    streams: Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    stream_id: u64,
    close_sent: Arc<AtomicBool>,
) {
    let mut remote_closed = false;
    while let Ok(message) = inbound_rx.recv() {
        match message {
            ProxyInbound::OpenResult { .. } => {}
            ProxyInbound::Data(bytes) => {
                if socket.write_all(&bytes).is_err() {
                    break;
                }
            }
            ProxyInbound::Close(reason) => {
                let detail = proxy_detail_text(&reason);
                mark_stream_closed(&streams, stream_id, ProxyStreamStatus::Closed, &detail);
                remote_closed = true;
                break;
            }
        }
    }
    if remote_closed {
        close_sent.store(true, Ordering::Relaxed);
    }
    let _ = socket.shutdown(Shutdown::Both);
}

struct SocksRequestError {
    reply: Option<u8>,
    detail: String,
}

impl SocksRequestError {
    fn with_reply(reply: u8, detail: impl Into<String>) -> Self {
        Self {
            reply: Some(reply),
            detail: detail.into(),
        }
    }

    fn without_reply(detail: impl Into<String>) -> Self {
        Self {
            reply: None,
            detail: detail.into(),
        }
    }
}

fn socks_io_error(error: io::Error) -> SocksRequestError {
    let detail = if matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) {
        t("SOCKS request timed out").to_string()
    } else {
        error.to_string()
    };
    SocksRequestError::with_reply(SOCKS_REPLY_GENERAL_FAILURE, detail)
}

fn socks5_connect(socket: &mut TcpStream) -> Result<(String, u16), SocksRequestError> {
    let mut header = [0_u8; 2];
    socket.read_exact(&mut header).map_err(socks_io_error)?;
    if header[0] != 0x05 {
        return Err(SocksRequestError::with_reply(
            SOCKS_REPLY_GENERAL_FAILURE,
            t("Unsupported SOCKS version"),
        ));
    }
    let methods_len = header[1] as usize;
    let mut methods = vec![0_u8; methods_len];
    socket.read_exact(&mut methods).map_err(socks_io_error)?;
    if !methods.contains(&0x00) {
        let _ = socket.write_all(&[0x05, 0xff]);
        return Err(SocksRequestError::without_reply(t(
            "SOCKS no acceptable authentication method",
        )));
    }
    socket.write_all(&[0x05, 0x00]).map_err(socks_io_error)?;

    let mut req = [0_u8; 4];
    socket.read_exact(&mut req).map_err(socks_io_error)?;
    if req[0] != 0x05 || req[1] != 0x01 || req[2] != 0x00 {
        return Err(SocksRequestError::with_reply(
            SOCKS_REPLY_COMMAND_NOT_SUPPORTED,
            t("Only SOCKS5 CONNECT is supported"),
        ));
    }
    let host = match req[3] {
        0x01 => {
            let mut raw = [0_u8; 4];
            socket.read_exact(&mut raw).map_err(socks_io_error)?;
            std::net::Ipv4Addr::from(raw).to_string()
        }
        0x03 => {
            let mut len = [0_u8; 1];
            socket.read_exact(&mut len).map_err(socks_io_error)?;
            let mut raw = vec![0_u8; len[0] as usize];
            socket.read_exact(&mut raw).map_err(socks_io_error)?;
            String::from_utf8(raw).map_err(|_| {
                SocksRequestError::with_reply(
                    SOCKS_REPLY_HOST_UNREACHABLE,
                    t("Invalid SOCKS domain name"),
                )
            })?
        }
        0x04 => {
            let mut raw = [0_u8; 16];
            socket.read_exact(&mut raw).map_err(socks_io_error)?;
            std::net::Ipv6Addr::from(raw).to_string()
        }
        _ => {
            return Err(SocksRequestError::with_reply(
                SOCKS_REPLY_ADDRESS_TYPE_NOT_SUPPORTED,
                t("Unsupported SOCKS address type"),
            ));
        }
    };
    let mut raw_port = [0_u8; 2];
    socket.read_exact(&mut raw_port).map_err(socks_io_error)?;
    Ok((host, u16::from_be_bytes(raw_port)))
}

fn send_socks5_success(socket: &mut TcpStream) -> io::Result<()> {
    socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
}

fn send_socks5_failure(socket: &mut TcpStream, code: u8) -> io::Result<()> {
    socket.write_all(&[0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
}

fn render_toolbar(
    ui: &mut egui::Ui,
    listen_ip: &Arc<Mutex<String>>,
    listen_port: &Arc<Mutex<String>>,
    status: &Arc<Mutex<ProxyStatus>>,
    start_requested: &Arc<AtomicBool>,
    stop_requested: &Arc<AtomicBool>,
) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = COMPACT_CONTROL_HEIGHT;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t("Reverse Proxy"))
                    .size(13.0)
                    .color(crate::theme::palette().text)
                    .strong(),
            );
            ui.separator();
            let running = matches!(
                locked_status(status),
                ProxyStatus::Starting | ProxyStatus::Listening | ProxyStatus::Stopping
            );
            edit_locked_text(ui, listen_ip, 104.0, "127.0.0.1", !running);
            edit_locked_text(ui, listen_port, 64.0, DEFAULT_LISTEN_PORT, !running);
            if ui
                .add_enabled(!running, egui::Button::new(t("Start")))
                .clicked()
            {
                start_requested.store(true, Ordering::Relaxed);
            }
            if ui
                .add_enabled(running, egui::Button::new(t("Stop")))
                .clicked()
            {
                stop_requested.store(true, Ordering::Relaxed);
            }
        });
    });
}

fn render_endpoint(
    ui: &mut egui::Ui,
    identity: &str,
    listen_ip: &Arc<Mutex<String>>,
    listen_port: &Arc<Mutex<String>>,
    test_target: &Arc<Mutex<String>>,
    status: &Arc<Mutex<ProxyStatus>>,
    test_requested: &Arc<AtomicBool>,
    test_in_flight: &Arc<AtomicBool>,
) {
    crate::theme::panel_frame_with_margin(PANEL_MARGIN).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = COMPACT_CONTROL_HEIGHT;
            let endpoint = format!(
                "SOCKS5 {}:{}",
                locked_string(listen_ip),
                locked_string(listen_port)
            );
            let copy_enabled = matches!(locked_status(status), ProxyStatus::Listening);
            let spacing = ui.spacing().item_spacing.x;
            let action_width = action_area_width(ui, &["Copy"]);
            let left_width = (ui.available_width() - action_width - spacing).max(140.0);
            ui.allocate_ui_with_layout(
                egui::vec2(left_width, COMPACT_CONTROL_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(crate::theme::muted_text(t("Client")).strong());
                    ui.add_sized(
                        [
                            (left_width * 0.34).clamp(90.0, 220.0),
                            COMPACT_CONTROL_HEIGHT,
                        ],
                        egui::Label::new(identity).truncate(),
                    );
                    ui.separator();
                    ui.add_sized(
                        [ui.available_width().max(80.0), COMPACT_CONTROL_HEIGHT],
                        egui::Label::new(
                            egui::RichText::new(endpoint.clone())
                                .font(egui::FontId::monospace(12.0)),
                        )
                        .truncate(),
                    );
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(action_width, COMPACT_CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui
                        .add_enabled(copy_enabled, egui::Button::new(t("Copy")))
                        .clicked()
                    {
                        ui.ctx().copy_text(endpoint);
                    }
                },
            );
        });
        ui.add_space(SECTION_GAP);
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = COMPACT_CONTROL_HEIGHT;
            let copy_enabled = matches!(locked_status(status), ProxyStatus::Listening);
            let testing = test_in_flight.load(Ordering::Relaxed);
            let test_label = if testing { "Testing..." } else { "Test" };
            let target_preview = locked_string(test_target);
            let spacing = ui.spacing().item_spacing.x;
            let action_width = action_area_width(ui, &[test_label]);
            ui.label(crate::theme::muted_text(t("Target")).strong());
            let input_width =
                (ui.available_width() - action_width - spacing).max(ui.spacing().interact_size.x);
            edit_test_target(ui, test_target, !testing, input_width);
            ui.allocate_ui_with_layout(
                egui::vec2(action_width, COMPACT_CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui
                        .add_enabled(copy_enabled && !testing, egui::Button::new(t(test_label)))
                        .on_hover_text(tf(
                            "Test SOCKS5 through {target}",
                            &[("target", target_preview.as_str())],
                        ))
                        .clicked()
                    {
                        test_requested.store(true, Ordering::Relaxed);
                    }
                },
            );
        });
        ui.add_space(SECTION_GAP);
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = COMPACT_CONTROL_HEIGHT;
            let example = curl_example_command(
                &locked_string(listen_ip),
                &locked_string(listen_port),
                &locked_string(test_target),
            );
            let copy_enabled = matches!(locked_status(status), ProxyStatus::Listening);
            let spacing = ui.spacing().item_spacing.x;
            let action_width = action_area_width(ui, &["Copy"]);
            let label_width = action_button_width(ui, t("Example"));
            let command_width = (ui.available_width() - label_width - action_width - spacing * 2.0)
                .max(ui.spacing().interact_size.x);
            ui.add_sized(
                [label_width, COMPACT_CONTROL_HEIGHT],
                egui::Label::new(crate::theme::muted_text(t("Example")).strong()),
            );
            ui.add_sized(
                [command_width, COMPACT_CONTROL_HEIGHT],
                egui::Label::new(
                    egui::RichText::new(example.clone()).font(egui::FontId::monospace(12.0)),
                )
                .truncate(),
            )
            .on_hover_text(example.clone());
            ui.allocate_ui_with_layout(
                egui::vec2(action_width, COMPACT_CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui
                        .add_enabled(copy_enabled, egui::Button::new(t("Copy")))
                        .clicked()
                    {
                        ui.ctx().copy_text(example);
                    }
                },
            );
        });
    });
}

fn curl_example_command(listen_ip: &str, listen_port: &str, target: &str) -> String {
    format!(
        "curl.exe --socks5-hostname {}:{} {}",
        proxy_connect_host(listen_ip),
        listen_port.trim(),
        curl_example_target_url(target)
    )
}

fn curl_example_target_url(target: &str) -> String {
    match parse_test_target(target).or_else(|_| parse_test_target(DEFAULT_TEST_TARGET)) {
        Ok(target) => {
            let host = if target.host.contains(':') {
                format!("[{}]", target.host)
            } else {
                target.host
            };
            match target.port {
                80 => format!("http://{host}"),
                443 => format!("https://{host}"),
                port => format!("http://{host}:{port}"),
            }
        }
        Err(_) => "https://www.google.com".to_string(),
    }
}

fn edit_test_target(ui: &mut egui::Ui, value: &Arc<Mutex<String>>, enabled: bool, width: f32) {
    let mut text = locked_string(value);
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.add_sized(
                [width, COMPACT_CONTROL_HEIGHT],
                egui::TextEdit::singleline(&mut text)
                    .hint_text(DEFAULT_TEST_TARGET)
                    .vertical_align(egui::Align::Center),
            )
        })
        .inner;
    if response.changed() {
        if let Ok(mut value) = value.lock() {
            *value = text;
        }
    }
    response.on_hover_text(t("Test target host:port"));
}

fn render_connections(
    ui: &mut egui::Ui,
    streams: &Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    selected_stream: &Arc<Mutex<Option<u64>>>,
    clear_closed_requested: &Arc<AtomicBool>,
    kill_selected_requested: &Arc<AtomicBool>,
) {
    crate::theme::panel_frame_with_margin(PANEL_MARGIN).show(ui, |ui| {
        ui.set_max_height(ui.available_height());
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = COMPACT_CONTROL_HEIGHT;
            let spacing = ui.spacing().item_spacing.x;
            let action_width = action_area_width(ui, &["Kill Selected", "Clear Closed"]);
            let left_width = (ui.available_width() - action_width - spacing).max(120.0);
            ui.allocate_ui_with_layout(
                egui::vec2(left_width, COMPACT_CONTROL_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new(t("Connections"))
                            .size(12.0)
                            .color(crate::theme::palette().text)
                            .strong(),
                    );
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(action_width, COMPACT_CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui.button(t("Clear Closed")).clicked() {
                        clear_closed_requested.store(true, Ordering::Relaxed);
                    }
                    if ui.button(t("Kill Selected")).clicked() {
                        kill_selected_requested.store(true, Ordering::Relaxed);
                    }
                },
            );
        });
        ui.add_space(SECTION_GAP);
        let available_height = ui.available_height().max(0.0);
        egui::ScrollArea::vertical()
            .id_salt("reverse_proxy_connections")
            .max_height(available_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_connection_table(ui, streams, selected_stream);
            });
    });
}

fn render_status_bar(
    ui: &mut egui::Ui,
    status: &Arc<Mutex<ProxyStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let status = locked_status(status);
    let (label, color) = match status {
        ProxyStatus::Stopped => (t("Stopped"), crate::theme::palette().muted),
        ProxyStatus::Starting => (t("Starting"), COLOR_WARN),
        ProxyStatus::Listening => (t("Listening"), COLOR_GOOD),
        ProxyStatus::Stopping => (t("Stopping"), COLOR_WARN),
        ProxyStatus::Error => (t("Error"), COLOR_BAD),
    };
    let progress = locked_string(notice);
    crate::theme::status_frame().show(ui, |ui| {
        ui.set_min_height(26.0);
        crate::theme::render_status_line(ui, label, color, &progress, |_| {});
    });
}

fn render_connection_table(
    ui: &mut egui::Ui,
    streams: &Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    selected_stream: &Arc<Mutex<Option<u64>>>,
) {
    let rows = connection_rows(streams);
    if rows.is_empty() {
        ui.label(crate::theme::muted_text(t("No connections")));
        return;
    }

    let available_width = ui.available_width().max(640.0);
    let state_width = 72.0;
    let rx_width = 72.0;
    let tx_width = 72.0;
    let time_width = 62.0;
    let detail_width = 170.0;
    let target_width =
        (available_width - state_width - rx_width - tx_width - time_width - detail_width - 30.0)
            .max(160.0);
    let selected = selected_stream.lock().ok().and_then(|value| *value);

    crate::theme::clickable_table(ui, "reverse_proxy_connections_table", true)
        .column(Column::initial(target_width).at_least(150.0).clip(true))
        .column(Column::initial(state_width).at_least(64.0).clip(true))
        .column(Column::initial(rx_width).at_least(62.0).clip(true))
        .column(Column::initial(tx_width).at_least(62.0).clip(true))
        .column(Column::initial(time_width).at_least(56.0).clip(true))
        .column(Column::initial(detail_width).at_least(120.0).clip(true))
        .header(TABLE_HEADER_HEIGHT, |mut header| {
            header.col(|ui| table_header_label(ui, t("Target")));
            header.col(|ui| table_header_label(ui, t("State")));
            header.col(|ui| table_header_label(ui, t("Rx")));
            header.col(|ui| table_header_label(ui, t("Tx")));
            header.col(|ui| table_header_label(ui, t("Time")));
            header.col(|ui| table_header_label(ui, t("Detail")));
        })
        .body(|body| {
            body.rows(TABLE_ROW_HEIGHT, rows.len(), |mut row| {
                let row_data = &rows[row.index()];
                let is_selected = selected == Some(row_data.stream_id);
                row.set_selected(is_selected);
                row.col(|ui| table_text(ui, &compact_text(&row_data.target, 34)));
                row.col(|ui| table_text(ui, t(row_data.status_label)));
                row.col(|ui| table_text(ui, &format_bytes(row_data.rx_bytes)));
                row.col(|ui| table_text(ui, &format_bytes(row_data.tx_bytes)));
                row.col(|ui| table_text(ui, &format_duration(row_data.duration)));
                row.col(|ui| table_text(ui, &compact_text(&row_data.detail, 42)));
                let response = row.response();
                if response.hovered() {
                    response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    if let Ok(mut selected) = selected_stream.lock() {
                        *selected = Some(row_data.stream_id);
                    }
                }
            });
        });
}

fn table_header_label(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(crate::theme::muted_text(text).strong())
            .selectable(false)
            .sense(egui::Sense::hover()),
    );
}

fn table_text(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(crate::theme::body_text(text))
            .selectable(false)
            .sense(egui::Sense::hover()),
    );
}

fn remember_pre_open_failure(
    failures: &Arc<Mutex<Vec<ProxyPreOpenFailure>>>,
    detail: impl Into<String>,
) {
    if let Ok(mut failures) = failures.lock() {
        failures.push(ProxyPreOpenFailure {
            occurred_at: Instant::now(),
            detail: detail.into(),
        });
        let overflow = failures.len().saturating_sub(MAX_PRE_OPEN_FAILURES);
        if overflow > 0 {
            failures.drain(0..overflow);
        }
    }
}

fn socks_reply_for_proxy_error(error: &str) -> u8 {
    let error = error.to_ascii_lowercase();
    if error.contains("connection refused")
        || error.contains("actively refused")
        || error.contains("os error 10061")
    {
        SOCKS_REPLY_CONNECTION_REFUSED
    } else if error.contains("host unreachable")
        || error.contains("network unreachable")
        || error.contains("os error 10051")
        || error.contains("os error 10065")
    {
        SOCKS_REPLY_HOST_UNREACHABLE
    } else {
        SOCKS_REPLY_GENERAL_FAILURE
    }
}

fn action_area_width(ui: &egui::Ui, labels: &[&'static str]) -> f32 {
    let spacing = ui.spacing().item_spacing.x * labels.len().saturating_sub(1) as f32;
    labels
        .iter()
        .map(|label| action_button_width(ui, t(label)))
        .sum::<f32>()
        + spacing
}

fn action_button_width(ui: &egui::Ui, label: &str) -> f32 {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_string(), font_id, crate::theme::palette().text)
        .size()
        .x;
    (text_width + ui.spacing().button_padding.x * 2.0).max(ui.spacing().interact_size.x)
}

struct ConnectionRow {
    stream_id: u64,
    target: String,
    status_label: &'static str,
    rx_bytes: u64,
    tx_bytes: u64,
    duration: Duration,
    detail: String,
}

fn connection_rows(streams: &Arc<Mutex<HashMap<u64, ProxyStreamState>>>) -> Vec<ConnectionRow> {
    let now = Instant::now();
    let mut rows = streams
        .lock()
        .map(|streams| {
            streams
                .iter()
                .map(|(stream_id, stream)| {
                    let end = stream.closed_at.unwrap_or(now);
                    ConnectionRow {
                        stream_id: *stream_id,
                        target: stream.target.clone(),
                        status_label: stream_status_label(stream.status),
                        rx_bytes: stream.rx_bytes,
                        tx_bytes: stream.tx_bytes,
                        duration: end.saturating_duration_since(stream.started_at),
                        detail: stream.detail.clone(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    rows.sort_by_key(|row| row.stream_id);
    rows.reverse();
    rows
}

fn stream_status_label(status: ProxyStreamStatus) -> &'static str {
    match status {
        ProxyStreamStatus::Opening => "Opening",
        ProxyStreamStatus::Open => "Open",
        ProxyStreamStatus::Closing => "Closing",
        ProxyStreamStatus::Closed => "Closed",
        ProxyStreamStatus::Failed => "Failed",
    }
}

fn edit_locked_text(
    ui: &mut egui::Ui,
    value: &Arc<Mutex<String>>,
    width: f32,
    hint: &'static str,
    enabled: bool,
) {
    let mut text = locked_string(value);
    let response = ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(&mut text)
            .hint_text(hint)
            .desired_width(width)
            .vertical_align(egui::Align::Center),
    );
    if response.changed() {
        if let Ok(mut value) = value.lock() {
            *value = text;
        }
    }
}

fn mark_stream_closed(
    streams: &Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    stream_id: u64,
    status: ProxyStreamStatus,
    detail: &str,
) {
    if let Ok(mut streams) = streams.lock() {
        if let Some(stream) = streams.get_mut(&stream_id) {
            stream.status = status;
            stream.closed_at = Some(Instant::now());
            stream.detail = detail.to_string();
            let _ = stream
                .inbound_tx
                .send(ProxyInbound::Close(detail.to_string()));
        }
    }
}

fn add_stream_tx_bytes(
    streams: &Arc<Mutex<HashMap<u64, ProxyStreamState>>>,
    stream_id: u64,
    bytes: u64,
) {
    if let Ok(mut streams) = streams.lock() {
        if let Some(stream) = streams.get_mut(&stream_id) {
            stream.tx_bytes = stream.tx_bytes.saturating_add(bytes);
        }
    }
}

fn send_proxy_close_once(
    input_tx: &SyncSender<AdminInput>,
    client_id: &str,
    stream_id: u64,
    reason: &str,
    close_sent: &AtomicBool,
) {
    if close_sent.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = input_tx.send(AdminInput::Proxy(Message::ProxyClose {
        client_id: client_id.to_string(),
        stream_id,
        reason: reason.to_string(),
    }));
}

fn proxy_is_running(window: &ReverseProxyWindow) -> bool {
    matches!(
        locked_status(&window.status),
        ProxyStatus::Starting | ProxyStatus::Listening | ProxyStatus::Stopping
    )
}

fn set_status(status: &Arc<Mutex<ProxyStatus>>, value: ProxyStatus) {
    if let Ok(mut status) = status.lock() {
        *status = value;
    }
}

fn locked_status(status: &Arc<Mutex<ProxyStatus>>) -> ProxyStatus {
    status
        .lock()
        .map(|status| *status)
        .unwrap_or(ProxyStatus::Error)
}

fn set_notice(notice: &Arc<Mutex<String>>, value: impl Into<String>) {
    if let Ok(mut notice) = notice.lock() {
        *notice = value.into();
    }
}

fn locked_string(value: &Arc<Mutex<String>>) -> String {
    value.lock().map(|value| value.clone()).unwrap_or_default()
}

fn initial_stream_id() -> u64 {
    ((now_epoch_ms() as u64) << 16).max(1)
}

fn identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn compact_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut text = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    text.push_str("...");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socks_reply_for_proxy_error_maps_connection_refused() {
        assert_eq!(
            socks_reply_for_proxy_error("connect 127.0.0.1:9 failed: os error 10061"),
            SOCKS_REPLY_CONNECTION_REFUSED
        );
    }

    #[test]
    fn pre_open_failure_can_be_reported_to_test_status() {
        let failures = Arc::new(Mutex::new(Vec::new()));
        let started_at = Instant::now();

        remember_pre_open_failure(&failures, "SOCKS request failed: unsupported version");

        assert_eq!(
            recent_pre_open_failure_detail(&failures, started_at).as_deref(),
            Some("SOCKS request failed: unsupported version")
        );
    }

    #[test]
    fn socks_io_error_maps_would_block_to_timeout_detail() {
        let error = socks_io_error(io::Error::from(io::ErrorKind::WouldBlock));

        assert_eq!(error.reply, Some(SOCKS_REPLY_GENERAL_FAILURE));
        assert_eq!(error.detail, t("SOCKS request timed out"));
    }

    #[test]
    fn curl_example_tracks_listen_endpoint() {
        assert_eq!(
            curl_example_command("127.0.0.1", "5269", "www.google.com:443"),
            "curl.exe --socks5-hostname 127.0.0.1:5269 https://www.google.com"
        );
        assert_eq!(
            curl_example_command("0.0.0.0", "7000", "example.com:80"),
            "curl.exe --socks5-hostname 127.0.0.1:7000 http://example.com"
        );
    }

    #[test]
    fn curl_example_tracks_target() {
        assert_eq!(
            curl_example_command("127.0.0.1", "5269", "127.0.0.1:5169"),
            "curl.exe --socks5-hostname 127.0.0.1:5269 http://127.0.0.1:5169"
        );
        assert_eq!(
            curl_example_command("127.0.0.1", "5269", "[::1]:8080"),
            "curl.exe --socks5-hostname 127.0.0.1:5269 http://[::1]:8080"
        );
    }
}

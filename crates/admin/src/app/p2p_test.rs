use super::{
    event::{AdminEvent, AdminInput},
    ui::{cell_label, centered_cell, compact_id, table_header, timestamped_log},
    ClientRow, ClientStatus,
};
use crate::{i18n::t, windowing};
use eframe::egui;
use rdl_protocol::{now_epoch_ms, p2p_udp, Message, P2pAction, Role};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Sender, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const REGISTER_INTERVAL_MS: u64 = 200;
const PROBE_INTERVAL_MS: u64 = 80;
const RECV_TIMEOUT_MS: u64 = 40;
const TEST_TIMEOUT_MS: u64 = 8_000;
const MAX_LOG_LINES: usize = 500;
const STATUS_BAR_HEIGHT: f32 = 44.0;
const STATUS_BAR_GAP: f32 = 8.0;

pub(super) struct P2pTestWindow {
    open: bool,
    selected_clients: HashSet<String>,
    sessions: HashMap<String, P2pClientSession>,
    logs: VecDeque<String>,
}

pub(super) enum P2pWindowAction {
    Start(Vec<String>),
    Stop(Vec<(String, u64)>),
}

struct P2pClientSession {
    client_id: String,
    label: String,
    session_id: u64,
    status: P2pStatus,
    detail: String,
    local_endpoint: String,
    peer_endpoint: String,
    rtt_ms: Option<u32>,
    worker: Option<AdminP2pSession>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum P2pStatus {
    Starting,
    WaitingPeer,
    Probing,
    Succeeded,
    Failed,
    Stopped,
}

struct AdminP2pSession {
    stop: Arc<AtomicBool>,
    peer_addr: Arc<Mutex<Option<SocketAddr>>>,
}

impl Default for P2pTestWindow {
    fn default() -> Self {
        Self {
            open: false,
            selected_clients: HashSet::new(),
            sessions: HashMap::new(),
            logs: VecDeque::new(),
        }
    }
}

impl P2pTestWindow {
    pub(super) fn open(&mut self) {
        self.open = true;
    }

    pub(super) fn stop_all_local(&mut self) {
        for session in self.sessions.values_mut() {
            if let Some(worker) = session.worker.take() {
                worker.stop.store(true, Ordering::Relaxed);
            }
            if matches!(
                session.status,
                P2pStatus::Starting | P2pStatus::WaitingPeer | P2pStatus::Probing
            ) {
                session.status = P2pStatus::Stopped;
                session.detail = t("Stopped").to_string();
            }
        }
    }

    pub(super) fn active_sessions(&self) -> Vec<(String, u64)> {
        self.sessions
            .values()
            .filter(|session| {
                session.session_id != 0
                    && matches!(
                        session.status,
                        P2pStatus::Starting | P2pStatus::WaitingPeer | P2pStatus::Probing
                    )
            })
            .map(|session| (session.client_id.clone(), session.session_id))
            .collect()
    }

    pub(super) fn mark_starting(&mut self, client: &ClientRow) {
        let client_id = client.info.id.clone();
        self.sessions.insert(
            client_id.clone(),
            P2pClientSession {
                client_id: client_id.clone(),
                label: client_log_label(client),
                session_id: 0,
                status: P2pStatus::Starting,
                detail: t("Waiting for server session...").to_string(),
                local_endpoint: String::new(),
                peer_endpoint: String::new(),
                rtt_ms: None,
                worker: None,
            },
        );
        self.push_log(format!(
            "{} {}",
            t("P2P test requested for"),
            self.client_label(&client_id)
        ));
    }

    pub(super) fn handle_control(
        &mut self,
        target_id: String,
        session_id: u64,
        nonce: u64,
        action: P2pAction,
        server_udp_addr: String,
        peer_udp_addr: String,
        detail: String,
        fallback_server_udp_addr: String,
        event_tx: Sender<AdminEvent>,
    ) {
        match action {
            P2pAction::ServerReady => {
                let session = self
                    .sessions
                    .entry(target_id.clone())
                    .or_insert_with(|| P2pClientSession::new_placeholder(&target_id));
                session.session_id = session_id;
                session.status = P2pStatus::WaitingPeer;
                session.detail = detail.clone();
                if let Some(worker) = session.worker.take() {
                    worker.stop.store(true, Ordering::Relaxed);
                }
                let worker = start_admin_udp_worker(
                    target_id.clone(),
                    session_id,
                    nonce,
                    server_udp_addr,
                    fallback_server_udp_addr,
                    event_tx,
                );
                session.worker = Some(worker);
                self.push_log(format!(
                    "{} {} session={session_id}",
                    t("P2P server ready for"),
                    self.client_label(&target_id)
                ));
            }
            P2pAction::PeerReady => {
                let Some(session) = self.sessions.get_mut(&target_id) else {
                    return;
                };
                match peer_udp_addr.parse() {
                    Ok(addr) => {
                        if let Some(worker) = session.worker.as_ref() {
                            if let Ok(mut peer_addr) = worker.peer_addr.lock() {
                                *peer_addr = Some(addr);
                            }
                        }
                        session.peer_endpoint = peer_udp_addr.clone();
                        session.status = P2pStatus::Probing;
                        session.detail = t("Probing direct UDP path...").to_string();
                        self.push_log(format!(
                            "{} {} peer={}",
                            t("P2P peer endpoint ready for"),
                            self.client_label(&target_id),
                            peer_udp_addr
                        ));
                    }
                    Err(error) => {
                        session.status = P2pStatus::Failed;
                        session.detail = error.to_string();
                        self.push_log(format!(
                            "{} {}: {}",
                            t("P2P peer endpoint invalid for"),
                            self.client_label(&target_id),
                            error
                        ));
                    }
                }
            }
            P2pAction::Error => {
                let session = self
                    .sessions
                    .entry(target_id.clone())
                    .or_insert_with(|| P2pClientSession::new_placeholder(&target_id));
                session.session_id = session_id;
                session.status = P2pStatus::Failed;
                session.detail = detail.clone();
                self.push_log(format!(
                    "{} {}: {}",
                    t("P2P test failed for"),
                    self.client_label(&target_id),
                    detail
                ));
            }
            P2pAction::Stop => {
                if let Some(session) = self.sessions.get_mut(&target_id) {
                    if let Some(worker) = session.worker.take() {
                        worker.stop.store(true, Ordering::Relaxed);
                    }
                    session.status = P2pStatus::Stopped;
                    session.detail = t("Stopped").to_string();
                }
            }
            P2pAction::Start => {}
        }
    }

    pub(super) fn handle_result(
        &mut self,
        client_id: String,
        session_id: u64,
        success: bool,
        finished: bool,
        endpoint: String,
        rtt_ms: u32,
        detail: String,
    ) {
        let session = self
            .sessions
            .entry(client_id.clone())
            .or_insert_with(|| P2pClientSession::new_placeholder(&client_id));
        if session.session_id == 0 {
            session.session_id = session_id;
        }
        if !endpoint.trim().is_empty() {
            if detail.contains("local=") || detail.contains("udp bound") {
                session.local_endpoint = endpoint.clone();
            } else {
                session.peer_endpoint = endpoint.clone();
            }
        }
        if rtt_ms > 0 {
            session.rtt_ms = Some(rtt_ms);
        }
        if finished {
            session.status = if success {
                P2pStatus::Succeeded
            } else {
                P2pStatus::Failed
            };
            if let Some(worker) = session.worker.take() {
                worker.stop.store(true, Ordering::Relaxed);
            }
        } else if matches!(session.status, P2pStatus::Starting) {
            session.status = P2pStatus::WaitingPeer;
        }
        session.detail = detail.clone();
        self.push_log(format!("{}: {}", self.client_label(&client_id), detail));
    }

    pub(super) fn render(
        &mut self,
        ctx: &egui::Context,
        clients: &[ClientRow],
        connected: bool,
    ) -> Option<P2pWindowAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let viewport_id = egui::ViewportId::from_hash_of("admin_p2p_test");
        let builder =
            windowing::child_viewport_builder(t("P2P Test"), [920.0, 620.0], [680.0, 460.0]);
        let mut open = self.open;
        ctx.show_viewport_immediate(viewport_id, builder, |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                open = false;
            }
            egui::CentralPanel::default()
                .frame(crate::theme::page_frame())
                .show_inside(ui, |ui| {
                    windowing::render_child_window_controls(ui);
                    let content_height =
                        (ui.available_height() - STATUS_BAR_HEIGHT - STATUS_BAR_GAP).max(0.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), content_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            self.render_toolbar(ui, clients, connected, &mut action);
                            ui.add_space(crate::theme::SECTION_GAP);
                            self.render_clients(ui, clients);
                            ui.add_space(crate::theme::SECTION_GAP);
                            self.render_logs(ui);
                        },
                    );
                    ui.add_space(STATUS_BAR_GAP);
                    self.render_status_bar(ui, connected);
                });
        });
        if !open {
            let active_sessions = self.active_sessions();
            self.open = false;
            self.stop_all_local();
            action = Some(P2pWindowAction::Stop(active_sessions));
        }
        action
    }

    fn render_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        clients: &[ClientRow],
        connected: bool,
        action: &mut Option<P2pWindowAction>,
    ) {
        ui.horizontal(|ui| {
            let selected_online = self.selected_online_clients(clients);
            if ui
                .add_enabled(
                    connected && !selected_online.is_empty(),
                    egui::Button::new(t("Start Test")),
                )
                .clicked()
            {
                *action = Some(P2pWindowAction::Start(selected_online));
            }
            if ui
                .add_enabled(
                    !self.active_sessions().is_empty(),
                    egui::Button::new(t("Stop Test")),
                )
                .clicked()
            {
                *action = Some(P2pWindowAction::Stop(self.active_sessions()));
            }
            if ui.button(t("Select Online")).clicked() {
                self.selected_clients = clients
                    .iter()
                    .filter(|client| client.status == ClientStatus::Online)
                    .map(|client| client.info.id.clone())
                    .collect();
            }
            if ui.button(t("Clear Selection")).clicked() {
                self.selected_clients.clear();
            }
            if ui.button(t("Clear Logs")).clicked() {
                self.logs.clear();
            }
        });
    }

    fn render_clients(&mut self, ui: &mut egui::Ui, clients: &[ClientRow]) {
        crate::theme::clickable_table(ui, "p2p_test_clients_table", true)
            .column(egui_extras::Column::initial(56.0).at_least(48.0))
            .column(egui_extras::Column::initial(90.0).at_least(80.0))
            .column(
                egui_extras::Column::initial(170.0)
                    .at_least(120.0)
                    .clip(true),
            )
            .column(
                egui_extras::Column::initial(150.0)
                    .at_least(110.0)
                    .clip(true),
            )
            .column(
                egui_extras::Column::initial(130.0)
                    .at_least(100.0)
                    .clip(true),
            )
            .column(egui_extras::Column::remainder().at_least(180.0).clip(true))
            .header(crate::theme::TABLE_HEADER_HEIGHT, |mut header| {
                header.col(|ui| table_header(ui, t("Select")));
                header.col(|ui| table_header(ui, t("Status")));
                header.col(|ui| table_header(ui, t("Client ID")));
                header.col(|ui| table_header(ui, t("Host")));
                header.col(|ui| table_header(ui, t("IP")));
                header.col(|ui| table_header(ui, t("P2P Result")));
            })
            .body(|body| {
                body.rows(crate::theme::TABLE_ROW_HEIGHT, clients.len(), |mut row| {
                    let client = &clients[row.index()];
                    let client_id = &client.info.id;
                    let selected = self.selected_clients.contains(client_id);
                    row.set_selected(selected);
                    row.col(|ui| {
                        let mut checked = selected;
                        if ui.checkbox(&mut checked, "").changed() {
                            self.set_selected(client_id, checked);
                        }
                    });
                    row.col(|ui| {
                        let (text, color) =
                            super::client_state::client_status_display(client.status);
                        centered_cell(ui, |ui| {
                            ui.label(egui::RichText::new(text).size(12.0).color(color).strong());
                        });
                    });
                    row.col(|ui| centered_cell(ui, |ui| cell_label(ui, client_id)));
                    row.col(|ui| centered_cell(ui, |ui| cell_label(ui, &client.info.hostname)));
                    row.col(|ui| centered_cell(ui, |ui| cell_label(ui, &client.info.peer_addr)));
                    row.col(|ui| {
                        let text = self
                            .sessions
                            .get(client_id)
                            .map(session_result_text)
                            .unwrap_or_else(|| "-".to_string());
                        centered_cell(ui, |ui| cell_label(ui, text));
                    });
                    if row.response().clicked() {
                        self.set_selected(client_id, !selected);
                    }
                });
            });
    }

    fn render_logs(&mut self, ui: &mut egui::Ui) {
        crate::theme::panel_frame_with_margin(crate::theme::PANEL_MARGIN).show(ui, |ui| {
            ui.label(crate::theme::strong_body_text(t("Test Logs")));
            ui.add_space(crate::theme::SECTION_GAP);
            let output = egui::ScrollArea::vertical()
                .id_salt("p2p_test_logs")
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if self.logs.is_empty() {
                        ui.label(crate::theme::muted_text(t("No logs yet.")));
                    } else {
                        for line in &self.logs {
                            ui.label(crate::theme::muted_text(line).size(12.0));
                        }
                    }
                });
            ui.interact(
                output.inner_rect,
                output.id.with("p2p_test_log_context_menu"),
                egui::Sense::click(),
            )
            .context_menu(|ui| {
                if ui.button(t("Copy")).clicked() {
                    ui.ctx().copy_text(
                        self.logs
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                    ui.close();
                }
                if ui.button(t("Clear")).clicked() {
                    self.logs.clear();
                    ui.close();
                }
            });
        });
    }

    fn render_status_bar(&self, ui: &mut egui::Ui, connected: bool) {
        crate::theme::status_frame().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                status_item(
                    ui,
                    t("Connection"),
                    if connected { t("Online") } else { t("Offline") },
                );
                status_item(ui, t("Selected"), self.selected_clients.len().to_string());
                status_item(ui, t("Running"), self.running_count().to_string());
                status_item(
                    ui,
                    t("Succeeded"),
                    self.status_count(P2pStatus::Succeeded).to_string(),
                );
                status_item(
                    ui,
                    t("Failed"),
                    self.status_count(P2pStatus::Failed).to_string(),
                );
            });
        });
    }

    fn selected_online_clients(&self, clients: &[ClientRow]) -> Vec<String> {
        clients
            .iter()
            .filter(|client| {
                client.status == ClientStatus::Online
                    && self.selected_clients.contains(&client.info.id)
            })
            .map(|client| client.info.id.clone())
            .collect()
    }

    fn set_selected(&mut self, client_id: &str, selected: bool) {
        if selected {
            self.selected_clients.insert(client_id.to_string());
        } else {
            self.selected_clients.remove(client_id);
        }
    }

    fn running_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|session| {
                matches!(
                    session.status,
                    P2pStatus::Starting | P2pStatus::WaitingPeer | P2pStatus::Probing
                )
            })
            .count()
    }

    fn status_count(&self, status: P2pStatus) -> usize {
        self.sessions
            .values()
            .filter(|session| session.status == status)
            .count()
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.logs.push_back(timestamped_log(line));
        while self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
    }

    fn client_label(&self, client_id: &str) -> String {
        self.sessions
            .get(client_id)
            .map(|session| session.label.clone())
            .unwrap_or_else(|| compact_id(client_id))
    }
}

impl P2pClientSession {
    fn new_placeholder(client_id: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            label: compact_id(client_id),
            session_id: 0,
            status: P2pStatus::Starting,
            detail: String::new(),
            local_endpoint: String::new(),
            peer_endpoint: String::new(),
            rtt_ms: None,
            worker: None,
        }
    }
}

pub(super) fn send_start(input_tx: &SyncSender<AdminInput>, client_id: &str) {
    let _ = input_tx.send(AdminInput::P2p(Message::P2pControl {
        target_id: client_id.to_string(),
        session_id: 0,
        nonce: 0,
        action: P2pAction::Start,
        server_udp_addr: String::new(),
        peer_udp_addr: String::new(),
        detail: String::new(),
    }));
}

pub(super) fn send_stop(input_tx: &SyncSender<AdminInput>, client_id: &str, session_id: u64) {
    let _ = input_tx.send(AdminInput::P2p(Message::P2pControl {
        target_id: client_id.to_string(),
        session_id,
        nonce: 0,
        action: P2pAction::Stop,
        server_udp_addr: String::new(),
        peer_udp_addr: String::new(),
        detail: String::new(),
    }));
}

fn start_admin_udp_worker(
    client_id: String,
    session_id: u64,
    nonce: u64,
    advertised_server_udp_addr: String,
    fallback_server_udp_addr: String,
    event_tx: Sender<AdminEvent>,
) -> AdminP2pSession {
    let stop = Arc::new(AtomicBool::new(false));
    let peer_addr = Arc::new(Mutex::new(None));
    let worker_stop = stop.clone();
    let worker_peer_addr = peer_addr.clone();
    thread::spawn(move || {
        admin_p2p_loop(
            client_id,
            session_id,
            nonce,
            advertised_server_udp_addr,
            fallback_server_udp_addr,
            event_tx,
            worker_stop,
            worker_peer_addr,
        );
    });
    AdminP2pSession { stop, peer_addr }
}

fn admin_p2p_loop(
    client_id: String,
    session_id: u64,
    nonce: u64,
    advertised_server_udp_addr: String,
    fallback_server_udp_addr: String,
    event_tx: Sender<AdminEvent>,
    stop: Arc<AtomicBool>,
    peer_addr: Arc<Mutex<Option<SocketAddr>>>,
) {
    let server_addr =
        match resolve_server_addr(&advertised_server_udp_addr, &fallback_server_udp_addr) {
            Ok(addr) => addr,
            Err(error) => {
                send_result(
                    &event_tx,
                    client_id,
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
            send_result(
                &event_tx,
                client_id,
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
        send_result(
            &event_tx,
            client_id,
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
    send_result(
        &event_tx,
        client_id.clone(),
        session_id,
        true,
        false,
        local_endpoint.clone(),
        0,
        format!("admin udp bound local={local_endpoint} server={server_addr}"),
    );

    let started_at = Instant::now();
    let mut last_register = Instant::now() - Duration::from_millis(REGISTER_INTERVAL_MS);
    let mut last_probe = Instant::now() - Duration::from_millis(PROBE_INTERVAL_MS);
    let mut sequence = 1_u64;
    let mut buf = [0_u8; p2p_udp::MAX_PACKET_BYTES];
    let mut packet = Vec::with_capacity(p2p_udp::MAX_PACKET_BYTES);
    let mut success_reported = false;

    while !stop.load(Ordering::Relaxed)
        && started_at.elapsed() < Duration::from_millis(TEST_TIMEOUT_MS)
    {
        if last_register.elapsed() >= Duration::from_millis(REGISTER_INTERVAL_MS) {
            p2p_udp::encode_register(Role::Admin, session_id, nonce, &mut packet);
            let _ = socket.send_to(&packet, server_addr);
            last_register = Instant::now();
        }
        let current_peer = peer_addr.lock().ok().and_then(|addr| *addr);
        if let Some(peer) = current_peer {
            if last_probe.elapsed() >= Duration::from_millis(PROBE_INTERVAL_MS) {
                p2p_udp::encode_probe(
                    Role::Admin,
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
                        Role::Admin,
                        session_id,
                        nonce,
                        sequence,
                        sent_epoch_ms,
                        &mut packet,
                    );
                    let _ = socket.send_to(&packet, from);
                    if !success_reported {
                        success_reported = true;
                        send_result(
                            &event_tx,
                            client_id.clone(),
                            session_id,
                            true,
                            true,
                            from.to_string(),
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
                        send_result(
                            &event_tx,
                            client_id.clone(),
                            session_id,
                            true,
                            true,
                            from.to_string(),
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
                send_result(
                    &event_tx,
                    client_id,
                    session_id,
                    false,
                    true,
                    local_endpoint,
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
        send_result(
            &event_tx,
            client_id,
            session_id,
            false,
            true,
            local_endpoint,
            0,
            "p2p test stopped".to_string(),
        );
    } else {
        send_result(
            &event_tx,
            client_id,
            session_id,
            false,
            true,
            local_endpoint,
            0,
            "p2p direct probe timed out".to_string(),
        );
    }
}

fn send_result(
    event_tx: &Sender<AdminEvent>,
    client_id: String,
    session_id: u64,
    success: bool,
    finished: bool,
    endpoint: impl Into<String>,
    rtt_ms: u32,
    detail: String,
) {
    let _ = event_tx.send(AdminEvent::P2pResult {
        client_id,
        session_id,
        success,
        finished,
        endpoint: endpoint.into(),
        rtt_ms,
        detail,
    });
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

fn status_item(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.label(crate::theme::muted_text(label));
    ui.label(crate::theme::strong_body_text(value.into()));
    ui.separator();
}

fn session_result_text(session: &P2pClientSession) -> String {
    let status = status_label(session.status);
    let mut text = format!("{status}: {}", session.detail);
    if let Some(rtt_ms) = session.rtt_ms {
        text.push_str(&format!(" ({rtt_ms}ms)"));
    }
    text
}

fn status_label(status: P2pStatus) -> &'static str {
    match status {
        P2pStatus::Starting => t("Starting"),
        P2pStatus::WaitingPeer => t("Waiting"),
        P2pStatus::Probing => t("Probing"),
        P2pStatus::Succeeded => t("Succeeded"),
        P2pStatus::Failed => t("Failed"),
        P2pStatus::Stopped => t("Stopped"),
    }
}

fn client_log_label(client: &ClientRow) -> String {
    let hostname = client.info.hostname.trim();
    let username = client.info.username.trim();
    match (hostname.is_empty(), username.is_empty()) {
        (false, false) => format!("{hostname} / {username}"),
        (false, true) => hostname.to_string(),
        (true, false) => username.to_string(),
        (true, true) => {
            let peer_addr = client.info.peer_addr.trim();
            if peer_addr.is_empty() {
                compact_id(&client.info.id)
            } else {
                peer_addr.to_string()
            }
        }
    }
}

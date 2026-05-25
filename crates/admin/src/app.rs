mod about;
mod audio_udp;
mod client_aliases;
mod client_builder;
mod client_groups;
mod client_map;
mod client_registry;
mod client_state;
mod client_table;
mod command_result;
pub(crate) mod event;
mod file_transfer;
mod network;
mod payload;
mod settings;
mod status_bar;
mod toast;
mod ui;
mod video_pipeline;
mod window_dispatch;

pub(crate) use self::client_state::{client_status_display, ClientRow, ClientStatus};
pub(crate) use self::ui::{cell_label, centered_cell, compact_id, table_header, timestamped_log};

use self::{
    audio_udp::{
        initial_stream_id, push_pending_audio_frame, voice_audio_forward_loop, AudioUdpEndpoint,
        AudioUdpSender, AudioUdpSession, PendingAudioFrame,
    },
    client_aliases::{AliasAction, AliasWindow},
    client_builder::ClientBuilderState,
    client_groups::{MoveGroupAction, MoveGroupWindow},
    client_map::ClientMapWindow,
    client_state::{
        client_commands_disabled_text, client_identity_label, client_location_label,
        client_mode_label, client_mode_search_tokens, client_online_notice, client_os_label,
        client_status_text, ClientOnlineToast,
    },
    command_result::{
        command_status_notice, command_title, command_window_identity_title,
        detail_has_result_table, detail_status, kill_target_process_succeeded,
        performance_auto_refresh_due, quiet_user_interaction_command, refresh_command_window,
        render_command_result, render_command_window_status_bar,
        session_command_requires_confirmation, update_command_window, CommandResultRenderState,
        CommandResultStatus, CommandResultWindow, StartupAddForm,
    },
    event::{AdminEvent, AdminEventSink, AdminInput, ReconnectEndpoint},
    file_transfer::{
        file_transfer_message, run_file_upload_transfer, sanitize_log_value,
        send_file_transfer_input, send_upload_cancel, should_log_admin_file_transfer_event,
    },
    network::admin_network_loop,
    payload::{payload_field, video_stream_payload},
    settings::{parse_connection_settings, SettingsAction, SettingsState},
    ui::{
        activity_context_menu, apply_admin_theme, empty_state, panel, prune_activity_logs,
        section_title, COLOR_BAD, COLOR_GOOD, COLOR_WARN, TOOLBAR_CONTROL_HEIGHT,
    },
    video_pipeline::{PendingVideoFrame, VideoDecodeWorkers, VideoFrameCoalescer},
};
use crate::{
    command_menu,
    i18n::{self, t, tf},
    live_control, remote_management,
    runtime::Config,
    tools::p2p_test,
    user_interaction, windowing,
};
use eframe::egui;
use rdl_protocol::{
    AudioSource, ClientInfo, CommandKind, CommandOutputStream, FileTransferAction,
    FileTransferDirection, Message, VideoSource,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const GUI_IDLE_FRAME_INTERVAL_MS: u64 = 250;
const GUI_REALTIME_AUDIO_FRAME_INTERVAL_MS: u64 = 16;
const ADMIN_INPUT_QUEUE_CAPACITY: usize = 8;
const VOICE_AUDIO_OUTBOUND_QUEUE_CAPACITY: usize = 128;
const MAX_GUI_EVENTS_PER_FRAME: usize = 4096;
const CLIENT_ONLINE_TOAST_TTL: Duration = Duration::from_secs(5);
const MAX_CLIENT_ONLINE_TOASTS: usize = 4;
const STATUS_BAR_HEIGHT: f32 = 44.0;
const STATUS_BAR_CONTENT_HEIGHT: f32 = 26.0;
const STATUS_BAR_GAP: f32 = 8.0;
const ROOT_STATUS_BAR_BOTTOM_MARGIN: f32 = 12.0;
pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    eprintln!("{}", config.startup_config_notice());
    run_gui(config)?;
    Ok(())
}

fn run_gui(config: Config) -> eframe::Result {
    disable_macos_automatic_window_tabbing();
    crate::theme::set_theme_kind(crate::theme::ThemeKind::from_config(&config.theme));
    i18n::set_language(i18n::Language::from_config(&config.language));

    let (input_tx, input_rx) = mpsc::sync_channel(ADMIN_INPUT_QUEUE_CAPACITY);
    let (voice_audio_tx, voice_audio_rx) = mpsc::sync_channel(VOICE_AUDIO_OUTBOUND_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel();
    let ui_event_tx = event_tx.clone();
    let network_config = config.clone();
    let repaint_handle = Arc::new(Mutex::new(None));
    let network_repaint_handle = repaint_handle.clone();
    let ignored_file_transfers = Arc::new(Mutex::new(HashSet::new()));
    let network_ignored_file_transfers = ignored_file_transfers.clone();
    let audio_playback_registry = live_control::audio_listen::AudioPlaybackRegistry::default();
    let network_audio_playback_registry = audio_playback_registry.clone();
    let voice_udp_senders = Arc::new(Mutex::new(HashMap::new()));
    let voice_udp_endpoints = Arc::new(Mutex::new(HashMap::new()));
    let voice_audio_udp_senders = voice_udp_senders.clone();
    let voice_audio_udp_endpoints = voice_udp_endpoints.clone();

    thread::spawn(move || {
        voice_audio_forward_loop(
            voice_audio_rx,
            voice_audio_udp_senders,
            voice_audio_udp_endpoints,
        )
    });

    thread::spawn(move || {
        let event_sink = AdminEventSink::new(
            event_tx,
            Some(network_repaint_handle),
            Some(network_audio_playback_registry),
        )
        .with_video_frame_coalescer(Arc::new(VideoFrameCoalescer::default()));
        if let Err(error) = admin_network_loop(
            network_config,
            input_rx,
            event_sink.clone(),
            network_ignored_file_transfers,
        ) {
            event_sink.send(AdminEvent::Log(format!("network stopped: {error}")));
        }
    });

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([780.0, 520.0]);
    let viewport = match rust_desk_light_assets::app_window_icon() {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let window_title = rdl_version::app_version("rust-desk-light admin");

    eframe::run_native(
        &window_title,
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(AdminApp::new(
                cc,
                config,
                input_tx,
                event_rx,
                ui_event_tx,
                repaint_handle,
                ignored_file_transfers,
                audio_playback_registry,
                voice_audio_tx,
                voice_udp_senders,
                voice_udp_endpoints,
            )))
        }),
    )
}

#[cfg(target_os = "macos")]
fn disable_macos_automatic_window_tabbing() {
    if let Some(main_thread) = objc2_foundation::MainThreadMarker::new() {
        objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, main_thread);
    }
}

#[cfg(not(target_os = "macos"))]
fn disable_macos_automatic_window_tabbing() {}

struct AdminApp {
    config: Config,
    input_tx: SyncSender<AdminInput>,
    event_rx: Receiver<AdminEvent>,
    event_tx: Sender<AdminEvent>,
    repaint_handle: Arc<Mutex<Option<egui::Context>>>,
    voice_audio_tx: SyncSender<user_interaction::voice_chat::OutboundCommand>,
    audio_playback_registry: live_control::audio_listen::AudioPlaybackRegistry,
    connected: bool,
    clients: Vec<ClientRow>,
    client_aliases: HashMap<String, String>,
    client_groups: HashMap<String, String>,
    force_deleted_client_ids: HashSet<String>,
    force_delete_client_confirm: Option<String>,
    alias_window: AliasWindow,
    move_group_window: MoveGroupWindow,
    client_filter: String,
    client_builder_open: bool,
    client_builder: ClientBuilderState,
    client_map_window: ClientMapWindow,
    selected_client_id: Option<String>,
    command_windows: Vec<CommandResultWindow>,
    file_manager_windows: Vec<remote_management::file_manager::FileManagerWindow>,
    desktop_windows: Vec<live_control::remote_desktop::RemoteDesktopWindow>,
    camera_windows: Vec<live_control::camera::CameraWindow>,
    video_decode_workers: VideoDecodeWorkers,
    audio_windows: Vec<live_control::audio_listen::AudioListenWindow>,
    audio_udp_sessions: HashMap<String, AudioUdpSession>,
    audio_udp_next_stream_id: u64,
    terminal_windows: Vec<remote_management::remote_terminal::TerminalWindow>,
    proxy_windows: Vec<remote_management::reverse_proxy::ReverseProxyWindow>,
    p2p_test: p2p_test::P2pTestWindow,
    chat_windows: Vec<user_interaction::text_chat::ChatWindow>,
    voice_chat_windows: Vec<user_interaction::voice_chat::VoiceChatWindow>,
    interaction_command_windows: Vec<user_interaction::InteractionCommandWindow>,
    session_command_windows: Vec<crate::session::SessionCommandWindow>,
    execute_windows: Vec<crate::execute::ExecuteWindow>,
    voice_udp_sessions: HashMap<String, AudioUdpSession>,
    voice_udp_senders: Arc<Mutex<HashMap<String, AudioUdpSender>>>,
    voice_udp_endpoints: Arc<Mutex<HashMap<String, AudioUdpEndpoint>>>,
    file_transfer_cancel_flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
    log_lines: Vec<String>,
    client_online_toasts: VecDeque<ClientOnlineToast>,
    client_list_initialized: bool,
    settings: SettingsState,
    about_open: bool,
    applied_theme: Option<(crate::theme::ThemeKind, crate::theme::ResolvedTheme)>,
}

impl AdminApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        input_tx: SyncSender<AdminInput>,
        event_rx: Receiver<AdminEvent>,
        event_tx: Sender<AdminEvent>,
        repaint_handle: Arc<Mutex<Option<egui::Context>>>,
        ignored_file_transfers: Arc<Mutex<HashSet<(String, u64)>>>,
        audio_playback_registry: live_control::audio_listen::AudioPlaybackRegistry,
        voice_audio_tx: SyncSender<user_interaction::voice_chat::OutboundCommand>,
        voice_udp_senders: Arc<Mutex<HashMap<String, AudioUdpSender>>>,
        voice_udp_endpoints: Arc<Mutex<HashMap<String, AudioUdpEndpoint>>>,
    ) -> Self {
        apply_admin_theme(
            &cc.egui_ctx,
            crate::theme::ThemeKind::from_config(&config.theme),
        );
        if let Ok(mut handle) = repaint_handle.lock() {
            *handle = Some(cc.egui_ctx.clone());
        }
        let startup_config_notice = config.startup_config_notice().to_string();
        let client_builder = ClientBuilderState::new(&config);
        let settings = SettingsState::new(&config);
        let log_lines = vec![
            timestamped_log(format!(
                "admin gui started version={}",
                rdl_version::display_version()
            )),
            timestamped_log(startup_config_notice),
        ];
        Self {
            config,
            input_tx,
            event_rx,
            event_tx,
            repaint_handle,
            audio_playback_registry,
            connected: false,
            clients: Vec::new(),
            client_aliases: client_aliases::load_client_aliases(),
            client_groups: client_groups::load_client_groups(),
            force_deleted_client_ids: HashSet::new(),
            force_delete_client_confirm: None,
            alias_window: AliasWindow::default(),
            move_group_window: MoveGroupWindow::default(),
            client_filter: String::new(),
            client_builder_open: false,
            client_builder,
            client_map_window: ClientMapWindow::new(),
            selected_client_id: None,
            command_windows: Vec::new(),
            file_manager_windows: Vec::new(),
            desktop_windows: Vec::new(),
            camera_windows: Vec::new(),
            video_decode_workers: VideoDecodeWorkers::default(),
            audio_windows: Vec::new(),
            audio_udp_sessions: HashMap::new(),
            audio_udp_next_stream_id: initial_stream_id(),
            terminal_windows: Vec::new(),
            proxy_windows: Vec::new(),
            p2p_test: p2p_test::P2pTestWindow::default(),
            chat_windows: Vec::new(),
            voice_chat_windows: Vec::new(),
            voice_audio_tx,
            interaction_command_windows: Vec::new(),
            session_command_windows: Vec::new(),
            execute_windows: Vec::new(),
            voice_udp_sessions: HashMap::new(),
            voice_udp_senders,
            voice_udp_endpoints,
            file_transfer_cancel_flags: Arc::new(Mutex::new(HashMap::new())),
            ignored_file_transfers,
            log_lines,
            client_online_toasts: VecDeque::new(),
            client_list_initialized: false,
            settings,
            about_open: false,
            applied_theme: None,
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut changed = false;
        let mut latest_desktop_frames = HashMap::<String, String>::new();
        let mut latest_camera_frames = HashMap::<String, String>::new();
        let mut latest_desktop_video_frames = HashMap::<String, PendingVideoFrame>::new();
        let mut latest_camera_video_frames = HashMap::<String, PendingVideoFrame>::new();
        let mut pending_audio_frames = HashMap::<(String, u8), VecDeque<PendingAudioFrame>>::new();
        let mut processed_events = 0usize;
        while processed_events < MAX_GUI_EVENTS_PER_FRAME {
            let Ok(event) = self.event_rx.try_recv() else {
                break;
            };
            processed_events += 1;
            changed = true;
            match event {
                AdminEvent::Connected => {
                    self.connected = true;
                    self.settings.finish_reconnect_success();
                    self.push_log("connected to server");
                }
                AdminEvent::Disconnected => {
                    self.connected = false;
                    self.push_log("disconnected from server");
                    self.client_list_initialized = false;
                    self.stop_all_audio_udp_sessions();
                    self.stop_all_voice_udp_sessions();
                    self.clear_voice_udp_senders();
                    remote_management::reverse_proxy::stop_all(&mut self.proxy_windows);
                    self.p2p_test.stop_all_local();
                    for client in &mut self.clients {
                        client.status = ClientStatus::Offline;
                    }
                }
                AdminEvent::ConnectionFailed {
                    ip,
                    port,
                    auth_token,
                    detail,
                } => {
                    self.connected = false;
                    self.settings
                        .open_with_connection_error(ip, port, auth_token, detail);
                }
                AdminEvent::AuthTokenRequired => {
                    let detail = "Server requires an auth token.";
                    self.settings.open_with_connection_error(
                        self.config.ip.clone(),
                        self.config.port,
                        self.config.auth_token.clone(),
                        detail,
                    );
                }
                AdminEvent::AuthTokenRejected(detail) => {
                    self.settings.open_with_connection_error(
                        self.config.ip.clone(),
                        self.config.port,
                        self.config.auth_token.clone(),
                        detail,
                    );
                }
                AdminEvent::Clients(clients) => {
                    self.merge_clients(clients);
                    if self.selected_client_id.is_none() {
                        self.selected_client_id =
                            self.clients.first().map(|client| client.info.id.clone());
                    }
                }
                AdminEvent::Ack {
                    client_id,
                    command,
                    accepted,
                    detail,
                } => {
                    if accepted
                        && command == CommandKind::Camera
                        && detail.starts_with("camera_frame\n")
                    {
                        latest_camera_frames.insert(client_id, detail);
                    } else {
                        self.handle_command_ack(client_id, command, accepted, detail);
                    }
                }
                AdminEvent::DesktopFrame { client_id, payload } => {
                    latest_desktop_frames.insert(client_id, payload);
                }
                AdminEvent::DecodedDesktopFrame {
                    client_id,
                    result,
                    decode_ms,
                } => match result {
                    Ok(frame) => live_control::remote_desktop::handle_decoded_frame(
                        &mut self.desktop_windows,
                        &client_id,
                        frame,
                        decode_ms,
                    ),
                    Err(message) => self.handle_desktop_ack(
                        &client_id,
                        true,
                        format!("remote_desktop_error\nmessage={message}"),
                    ),
                },
                AdminEvent::DecodedCameraFrame {
                    client_id,
                    result,
                    decode_ms,
                } => match result {
                    Ok(frame) => live_control::camera::handle_decoded_frame(
                        &mut self.camera_windows,
                        &client_id,
                        frame,
                        decode_ms,
                    ),
                    Err(message) => self.handle_camera_ack(
                        &client_id,
                        true,
                        format!("camera_error\nmessage={message}"),
                    ),
                },
                AdminEvent::VideoFrame {
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
                    let frame = PendingVideoFrame {
                        seq,
                        source_width,
                        source_height,
                        image_width,
                        image_height,
                        format,
                        bytes,
                    };
                    match source {
                        VideoSource::RemoteDesktop => {
                            latest_desktop_video_frames.insert(client_id, frame);
                        }
                        VideoSource::Camera => {
                            latest_camera_video_frames.insert(client_id, frame);
                        }
                    }
                }
                AdminEvent::VideoFrameReady {
                    client_id,
                    source,
                    coalescer,
                } => {
                    let Some(frame) = coalescer.take(&client_id, &source) else {
                        continue;
                    };
                    match source {
                        VideoSource::RemoteDesktop => {
                            latest_desktop_video_frames.insert(client_id, frame);
                        }
                        VideoSource::Camera => {
                            latest_camera_video_frames.insert(client_id, frame);
                        }
                    }
                }
                AdminEvent::AudioFrame {
                    client_id,
                    source,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                } => push_pending_audio_frame(
                    &mut pending_audio_frames,
                    client_id,
                    PendingAudioFrame {
                        source,
                        seq,
                        sample_rate,
                        channels,
                        format,
                        bytes,
                    },
                ),
                AdminEvent::CommandOutput {
                    client_id,
                    command,
                    stream_id,
                    sequence,
                    stream,
                    chunk,
                    current_dir,
                    finished,
                    success,
                } => self.handle_command_output(
                    client_id,
                    command,
                    stream_id,
                    sequence,
                    stream,
                    chunk,
                    current_dir,
                    finished,
                    success,
                ),
                AdminEvent::FileTransfer(message) => {
                    if let Message::FileTransfer {
                        target_id,
                        transfer_id,
                        direction,
                        action,
                        total_bytes,
                        transferred_bytes,
                        message: status_message,
                        ..
                    } = &message
                    {
                        if should_log_admin_file_transfer_event(*action, status_message) {
                            debug_log!(
                                "debug event=admin_file_transfer_recv client={} id={} direction={} action={} bytes={}/{} message={}",
                                target_id,
                                transfer_id,
                                direction.as_str(),
                                action.as_str(),
                                transferred_bytes,
                                total_bytes,
                                sanitize_log_value(status_message)
                            );
                        }
                        if *action == FileTransferAction::Error {
                            if let Ok(flags) = self.file_transfer_cancel_flags.lock() {
                                if let Some(flag) = flags.get(transfer_id) {
                                    flag.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                        if self.should_ignore_file_transfer(target_id, *transfer_id) {
                            if matches!(
                                *action,
                                FileTransferAction::Complete | FileTransferAction::Error
                            ) {
                                self.unignore_file_transfer(target_id, *transfer_id);
                            }
                            continue;
                        }
                        if let Some(result) = crate::session::handle_update_upload_transfer(
                            &mut self.session_command_windows,
                            &message,
                        ) {
                            if let crate::session::SessionFileTransferResult::SendCommand(
                                outbound,
                            ) = result
                            {
                                self.send_session_command(outbound);
                            }
                            continue;
                        }
                        let client_id = target_id.clone();
                        let (hostname, username) = self.client_window_identity(&client_id);
                        remote_management::file_manager::handle_transfer(
                            &mut self.file_manager_windows,
                            &client_id,
                            hostname,
                            username,
                            message,
                        );
                    }
                }
                AdminEvent::ProxyOpenResult {
                    client_id,
                    stream_id,
                    accepted,
                    detail,
                } => {
                    remote_management::reverse_proxy::handle_open_result(
                        &mut self.proxy_windows,
                        &client_id,
                        stream_id,
                        accepted,
                        detail,
                    );
                }
                AdminEvent::ProxyData {
                    client_id,
                    stream_id,
                    bytes,
                } => {
                    remote_management::reverse_proxy::handle_data(
                        &mut self.proxy_windows,
                        &client_id,
                        stream_id,
                        bytes,
                    );
                }
                AdminEvent::ProxyClose {
                    client_id,
                    stream_id,
                    reason,
                } => {
                    remote_management::reverse_proxy::handle_close(
                        &mut self.proxy_windows,
                        &client_id,
                        stream_id,
                        reason,
                    );
                }
                AdminEvent::P2pControl {
                    target_id,
                    session_id,
                    nonce,
                    action,
                    server_udp_addr,
                    peer_udp_addr,
                    detail,
                } => {
                    let fallback_server_udp_addr =
                        format!("{}:{}", self.config.ip, self.config.port);
                    self.p2p_test.handle_control(
                        target_id,
                        session_id,
                        nonce,
                        action,
                        server_udp_addr,
                        peer_udp_addr,
                        detail,
                        fallback_server_udp_addr,
                        self.event_tx.clone(),
                    );
                }
                AdminEvent::P2pResult {
                    client_id,
                    session_id,
                    success,
                    finished,
                    endpoint,
                    rtt_ms,
                    detail,
                } => self.p2p_test.handle_result(
                    client_id, session_id, success, finished, endpoint, rtt_ms, detail,
                ),
                AdminEvent::Log(line) => self.push_log(line),
            }
        }
        for (client_id, payload) in latest_desktop_frames {
            self.handle_desktop_ack(&client_id, true, payload);
        }
        for (client_id, payload) in latest_camera_frames {
            self.spawn_camera_frame_decode(client_id, payload);
        }
        for (client_id, frame) in latest_desktop_video_frames {
            self.spawn_video_frame_decode(client_id, VideoSource::RemoteDesktop, frame);
        }
        for (client_id, frame) in latest_camera_video_frames {
            self.spawn_video_frame_decode(client_id, VideoSource::Camera, frame);
        }
        for ((client_id, _), frames) in pending_audio_frames {
            for frame in frames {
                self.handle_pending_audio_frame(&client_id, frame);
            }
        }
        changed
    }

    fn handle_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::SaveConnection { ip, port, token } => {
                self.save_settings_connection(ip, port, token);
            }
            SettingsAction::SavePreferences { theme, language } => {
                self.save_settings_preferences(theme, language);
            }
        }
    }

    fn save_settings_connection(&mut self, ip: String, port: String, token: String) {
        let (ip, port, token) = match parse_connection_settings(&ip, &port, &token) {
            Ok(value) => value,
            Err(error) => {
                self.settings.set_error(error);
                return;
            }
        };

        match self.config.save_server_connection(&ip, port, &token) {
            Ok(()) => {
                self.settings.sync_connection(&self.config);
                self.settings.set_reconnect_pending();
                self.push_log(format!("saved server connection {ip}:{port} from settings"));
                let _ = self.input_tx.send(AdminInput::Reconnect {
                    reason: "server connection updated from settings".to_string(),
                    endpoint: Some(ReconnectEndpoint {
                        ip: ip.clone(),
                        port,
                        auth_token: token.clone(),
                    }),
                });
            }
            Err(error) => {
                self.settings
                    .set_error(tf("Save failed: {error}", &[("error", &error.to_string())]));
            }
        }
    }

    fn save_settings_preferences(&mut self, theme: String, language: String) {
        match self.config.save_ui_preferences(&theme, &language) {
            Ok(()) => {
                i18n::set_language(i18n::Language::from_config(&language));
                self.settings.sync_preferences(&self.config);
                if let Ok(handle) = self.repaint_handle.lock() {
                    if let Some(ctx) = handle.as_ref() {
                        let theme = crate::theme::ThemeKind::from_config(&theme);
                        let resolved_theme = apply_admin_theme(ctx, theme);
                        self.applied_theme = Some((theme, resolved_theme));
                        ctx.request_repaint();
                    }
                }
                self.settings.set_notice(t("Settings saved."));
                self.push_log(format!(
                    "saved admin preferences theme={theme} language={language}"
                ));
            }
            Err(error) => {
                self.settings.set_error(tf(
                    "Save preferences failed: {error}",
                    &[("error", &error.to_string())],
                ));
            }
        }
    }

    fn render_settings_window(&mut self, ctx: &egui::Context) {
        if let Some(action) = settings::render_settings_window(ctx, &mut self.settings) {
            self.handle_settings_action(action);
        }
    }

    fn render_p2p_test_window(&mut self, ctx: &egui::Context) {
        let clients = self.clients.clone();
        let aliases = self.client_aliases.clone();
        let Some(action) = self
            .p2p_test
            .render(ctx, &clients, &aliases, self.connected)
        else {
            return;
        };
        match action {
            p2p_test::P2pWindowAction::Start(client_ids) => {
                for client_id in client_ids {
                    let Some(client) = self
                        .clients
                        .iter()
                        .find(|client| client.info.id == client_id)
                        .cloned()
                    else {
                        continue;
                    };
                    let label = self.client_display_label(&client);
                    self.p2p_test.mark_starting(&client, label.clone());
                    p2p_test::send_start(&self.input_tx, &client.info.id);
                    self.push_log(format!("p2p test requested for {label}"));
                }
            }
            p2p_test::P2pWindowAction::Stop(sessions) => {
                for (client_id, session_id) in sessions {
                    p2p_test::send_stop(&self.input_tx, &client_id, session_id);
                    let label = self
                        .clients
                        .iter()
                        .find(|client| client.info.id == client_id)
                        .map(|client| self.client_display_label(client))
                        .unwrap_or_else(|| client_id.clone());
                    self.push_log(format!("p2p test stopped for {label}"));
                }
                self.p2p_test.stop_all_local();
            }
        }
    }

    fn handle_pending_audio_frame(&mut self, client_id: &str, frame: PendingAudioFrame) {
        match frame.source {
            AudioSource::AudioListen => match live_control::audio_listen::decode_audio_frame(
                frame.seq,
                frame.sample_rate,
                frame.channels,
                frame.format,
                frame.bytes,
            ) {
                Ok(frame) => {
                    live_control::audio_listen::handle_audio_frame(
                        &mut self.audio_windows,
                        client_id,
                        frame,
                    );
                }
                Err(message) => self.handle_audio_ack(
                    client_id,
                    true,
                    format!("audio_listen_error\nmessage={message}"),
                ),
            },
            AudioSource::VoiceChat => match user_interaction::voice_chat::decode_audio_frame(
                frame.seq,
                frame.sample_rate,
                frame.channels,
                frame.format,
                frame.bytes,
            ) {
                Ok(frame) => {
                    user_interaction::voice_chat::handle_audio_frame(
                        &mut self.voice_chat_windows,
                        client_id,
                        frame,
                    );
                }
                Err(message) => self.handle_voice_chat_ack(
                    client_id,
                    true,
                    format!("voice_chat_error\nmessage={message}"),
                ),
            },
        }
    }

    fn realtime_audio_active(&self) -> bool {
        live_control::audio_listen::has_active_windows(&self.audio_windows)
            || user_interaction::voice_chat::has_active_windows(&self.voice_chat_windows)
    }

    fn ignore_file_transfer(&self, client_id: &str, transfer_id: u64) {
        if let Ok(mut ignored) = self.ignored_file_transfers.lock() {
            ignored.insert((client_id.to_string(), transfer_id));
        }
        debug_log!(
            "debug event=admin_file_transfer_ignore_add client={client_id} id={transfer_id}"
        );
    }

    fn unignore_file_transfer(&self, client_id: &str, transfer_id: u64) {
        if let Ok(mut ignored) = self.ignored_file_transfers.lock() {
            ignored.remove(&(client_id.to_string(), transfer_id));
        }
        debug_log!(
            "debug event=admin_file_transfer_ignore_remove client={client_id} id={transfer_id}"
        );
    }

    fn should_ignore_file_transfer(&self, client_id: &str, transfer_id: u64) -> bool {
        self.ignored_file_transfers
            .lock()
            .map(|ignored| ignored.contains(&(client_id.to_string(), transfer_id)))
            .unwrap_or(false)
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let line = timestamped_log(line);
        eprintln!("{line}");
        self.log_lines.push(line);
        prune_activity_logs(&mut self.log_lines);
    }

    fn open_force_delete_client_confirm(&mut self, client_id: &str) {
        self.force_delete_client_confirm = Some(client_id.to_string());
    }

    fn force_delete_client(&mut self, client_id: &str) {
        let label = self.client_display_name(client_id);
        self.force_deleted_client_ids.insert(client_id.to_string());
        self.remove_client_row(client_id);
        let alias_removed = self.client_aliases.remove(client_id).is_some();
        let group_removed = self.client_groups.remove(client_id).is_some();
        if alias_removed {
            if let Err(error) = client_aliases::save_client_aliases(&self.client_aliases) {
                self.push_log(format!("force delete client alias cleanup failed: {error}"));
            }
        }
        if group_removed {
            if let Err(error) = client_groups::save_client_groups(&self.client_groups) {
                self.push_log(format!("force delete client group cleanup failed: {error}"));
            }
        }
        self.push_log(format!("force deleted offline client {label}"));
    }

    fn render_force_delete_client_confirm(&mut self, ctx: &egui::Context) {
        let Some(client_id) = self.force_delete_client_confirm.clone() else {
            return;
        };
        let label = self.client_display_name(&client_id);

        egui::Window::new(t("Confirm Delete Client"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(t(
                        "Remove this offline client from the admin list?",
                    ))
                    .size(12.0)
                    .color(crate::theme::palette().muted),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::theme::muted_text(t("Client")));
                    ui.label(crate::theme::body_text(&label));
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(t(
                        "This only removes the saved admin row. It cannot delete the remote client identity until the client reconnects.",
                    ))
                    .size(12.0)
                    .color(crate::theme::palette().muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(t("Delete Client"))
                                .color(COLOR_BAD)
                                .strong(),
                        ))
                        .clicked()
                    {
                        self.force_delete_client(&client_id);
                        self.force_delete_client_confirm = None;
                    }
                    if ui.button(t("Cancel")).clicked() {
                        self.force_delete_client_confirm = None;
                    }
                });
            });
    }

    fn send_command(&mut self, client_id: &str, command: CommandKind) {
        if command == CommandKind::MoveToGroup {
            self.open_move_group_window(client_id);
            return;
        }

        if !self
            .client_status_for(client_id)
            .map(ClientStatus::can_receive_commands)
            .unwrap_or(false)
        {
            if command == CommandKind::DeleteClient {
                self.open_force_delete_client_confirm(client_id);
                return;
            }
            self.push_log(format!(
                "blocked command={} to {}: client is offline",
                command.as_str(),
                client_id
            ));
            return;
        }

        if command_requires_client_ui(&command) && !self.client_gui_available(client_id) {
            self.push_log(format!(
                "blocked command={} to {}: client has no client UI",
                command.as_str(),
                client_id
            ));
            return;
        }

        if command == CommandKind::ClientConfig {
            self.open_session_command_window(client_id, command.clone());
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: client_id.to_string(),
                command,
                payload: "action=show".to_string(),
            });
            self.push_log(format!("requested client config snapshot from {client_id}"));
            return;
        }
        if session_command_requires_confirmation(&command) {
            self.open_session_command_window(client_id, command);
            return;
        }
        if command == CommandKind::TextChat {
            self.open_chat_window(client_id);
            return;
        }
        if command == CommandKind::VoiceChat {
            self.open_voice_chat_window(client_id);
            return;
        }
        if command == CommandKind::FileManager {
            self.open_file_manager_window(client_id);
            return;
        }
        if command == CommandKind::RemoteTerminal {
            self.open_terminal_window(client_id);
            return;
        }
        if command == CommandKind::Proxy {
            self.open_proxy_window(client_id);
            return;
        }
        if command == CommandKind::RemoteDesktop {
            self.open_desktop_window(client_id);
            return;
        }
        if command == CommandKind::Camera {
            self.open_camera_window(client_id);
            return;
        }
        if command == CommandKind::AudioListen {
            self.open_audio_window(client_id);
            return;
        }
        if matches!(
            command,
            CommandKind::ExecuteFile
                | CommandKind::ExecuteCode
                | CommandKind::ExecuteStaticCommand
                | CommandKind::CreateTask
        ) {
            self.open_execute_window(client_id, command);
            return;
        }
        if matches!(
            command,
            CommandKind::MessageBox | CommandKind::BalloonTip | CommandKind::OpenTextInNotepad
        ) {
            self.open_interaction_command_window(client_id, command);
            return;
        }
        let _ = self.input_tx.send(AdminInput::Command {
            target_id: client_id.to_string(),
            command: command.clone(),
            payload: String::new(),
        });
        self.open_command_window(client_id, command.clone());
        self.push_log(format!(
            ">> command={} to {}",
            command.as_str(),
            self.client_display_name(client_id)
        ));
    }

    fn open_chat_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::text_chat::open_window(
            &mut self.chat_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_voice_chat_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::voice_chat::open_window(
            &mut self.voice_chat_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_file_manager_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        remote_management::file_manager::open_window(
            &mut self.file_manager_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_terminal_window(&mut self, client_id: &str) {
        let (hostname, username, os) = self.client_window_environment(client_id);
        remote_management::remote_terminal::open_window(
            &mut self.terminal_windows,
            client_id,
            hostname,
            username,
            os,
        );
    }

    fn open_proxy_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        remote_management::reverse_proxy::open_window(
            &mut self.proxy_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_desktop_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::remote_desktop::open_window(
            &mut self.desktop_windows,
            client_id,
            hostname,
            username,
        );
    }

    fn open_camera_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::camera::open_window(&mut self.camera_windows, client_id, hostname, username);
    }

    fn open_audio_window(&mut self, client_id: &str) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::audio_listen::open_window(
            &mut self.audio_windows,
            client_id,
            hostname,
            username,
            self.audio_playback_registry.clone(),
        );
    }

    fn open_session_command_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        let client_os = self.client_os(client_id);
        crate::session::open_window(
            &mut self.session_command_windows,
            client_id,
            hostname,
            username,
            client_os,
            command,
            &self.config.ip,
            self.config.port,
            &self.config.auth_token,
        );
    }

    fn open_interaction_command_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::open_window(
            &mut self.interaction_command_windows,
            client_id,
            hostname,
            username,
            command,
        );
    }

    fn open_execute_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        crate::execute::open_window(
            &mut self.execute_windows,
            client_id,
            hostname,
            username,
            command,
        );
    }

    fn open_alias_window(&mut self, client_id: &str) {
        let default_alias = self.default_client_alias(client_id);
        let current_alias = self
            .client_aliases
            .get(client_id)
            .cloned()
            .unwrap_or_else(|| default_alias.clone());
        self.alias_window.open(
            client_id,
            current_alias.clone(),
            default_alias,
            &current_alias,
        );
    }

    fn apply_alias_action(&mut self, action: AliasAction) {
        match action {
            AliasAction::Save { client_id, alias } => {
                let alias = client_aliases::clean_alias(&alias);
                if alias.is_empty() {
                    self.alias_window.set_error(t("Alias cannot be empty"));
                } else if alias == self.default_client_alias(&client_id) {
                    self.client_aliases.remove(&client_id);
                    match client_aliases::save_client_aliases(&self.client_aliases) {
                        Ok(()) => {
                            self.alias_window.close();
                            self.push_log(format!("client {client_id} alias restored to default"));
                        }
                        Err(error) => self
                            .alias_window
                            .set_error(format!("{}: {error}", t("Save alias failed"))),
                    }
                } else {
                    self.client_aliases.insert(client_id.clone(), alias.clone());
                    match client_aliases::save_client_aliases(&self.client_aliases) {
                        Ok(()) => {
                            self.alias_window.close();
                            self.push_log(format!("client {client_id} alias set to {alias}"));
                        }
                        Err(error) => self
                            .alias_window
                            .set_error(format!("{}: {error}", t("Save alias failed"))),
                    }
                }
            }
            AliasAction::RestoreDefault { client_id } => {
                self.client_aliases.remove(&client_id);
                match client_aliases::save_client_aliases(&self.client_aliases) {
                    Ok(()) => {
                        self.alias_window.close();
                        self.push_log(format!("client {client_id} alias restored to default"));
                    }
                    Err(error) => self
                        .alias_window
                        .set_error(format!("{}: {error}", t("Save alias failed"))),
                }
            }
        }
    }

    fn default_client_alias(&self, client_id: &str) -> String {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| client_identity_label(&row.info))
            .unwrap_or_else(|| client_id.to_string())
    }

    fn client_display_name(&self, client_id: &str) -> String {
        let name = self
            .client_aliases
            .get(client_id)
            .cloned()
            .unwrap_or_else(|| self.default_client_alias(client_id));
        format!("[{}]", name)
    }

    fn open_move_group_window(&mut self, client_id: &str) {
        let label = self
            .clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| client_identity_label(&row.info))
            .unwrap_or_else(|| client_id.to_string());
        let group = self.client_group(client_id).to_string();
        self.move_group_window.open(client_id, label, &group);
    }

    fn apply_move_group_action(&mut self, action: MoveGroupAction) {
        match action {
            MoveGroupAction::Save { client_id, group } => {
                let group = client_groups::clean_group_name(&group);
                if group.is_empty() {
                    self.client_groups.remove(&client_id);
                } else {
                    self.client_groups.insert(client_id.clone(), group.clone());
                }
                match client_groups::save_client_groups(&self.client_groups) {
                    Ok(()) => {
                        self.move_group_window.close();
                        if group.is_empty() {
                            self.push_log(format!("client {client_id} group cleared"));
                        } else {
                            self.push_log(format!("client {client_id} moved to group {group}"));
                        }
                    }
                    Err(error) => self
                        .move_group_window
                        .set_error(format!("{}: {error}", t("Save group failed"))),
                }
            }
            MoveGroupAction::Clear { client_id } => {
                self.client_groups.remove(&client_id);
                match client_groups::save_client_groups(&self.client_groups) {
                    Ok(()) => {
                        self.move_group_window.close();
                        self.push_log(format!("client {client_id} group cleared"));
                    }
                    Err(error) => self
                        .move_group_window
                        .set_error(format!("{}: {error}", t("Save group failed"))),
                }
            }
        }
    }

    fn open_command_window(&mut self, client_id: &str, command: CommandKind) {
        let (hostname, username) = self.client_window_identity(client_id);
        self.command_windows.push(CommandResultWindow {
            id: self.next_command_window_id(),
            client_id: client_id.to_string(),
            hostname,
            username,
            command,
            status: CommandResultStatus::Pending,
            detail: t("Waiting for client result...").to_string(),
            status_notice: None,
            open: true,
            close_requested: Arc::new(AtomicBool::new(false)),
            refresh_requested: Arc::new(AtomicBool::new(false)),
            auto_refresh_enabled: Arc::new(AtomicBool::new(false)),
            last_auto_refresh_at: None,
            process_kill_confirm: Arc::new(Mutex::new(None)),
            process_kill_requested: Arc::new(Mutex::new(None)),
            startup_delete_confirm: Arc::new(Mutex::new(None)),
            startup_detail: Arc::new(Mutex::new(None)),
            startup_action_requested: Arc::new(Mutex::new(None)),
            registry_key_requested: Arc::new(Mutex::new(None)),
            registry_expanded_keys: Arc::new(Mutex::new(HashSet::new())),
            startup_add_form: Arc::new(Mutex::new(StartupAddForm::default())),
            table_filter: Arc::new(Mutex::new(String::new())),
            table_sort: Arc::new(Mutex::new(None)),
            table_selected_row: Arc::new(Mutex::new(None)),
        });
    }

    fn next_command_window_id(&self) -> u64 {
        self.command_windows
            .iter()
            .map(|window| window.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    fn client_window_identity(&self, client_id: &str) -> (String, String) {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| (self.client_display_label(row), String::new()))
            .unwrap_or_else(|| (client_id.to_string(), String::new()))
    }

    fn client_os(&self, client_id: &str) -> String {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| row.info.os.clone())
            .unwrap_or_default()
    }

    fn client_group(&self, client_id: &str) -> &str {
        self.client_groups
            .get(client_id)
            .map(String::as_str)
            .unwrap_or("")
    }

    fn client_gui_available(&self, client_id: &str) -> bool {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| row.info.gui_available)
            .unwrap_or(false)
    }

    fn client_window_environment(&self, client_id: &str) -> (String, String, String) {
        self.clients
            .iter()
            .find(|row| row.info.id == client_id)
            .map(|row| {
                (
                    self.client_display_label(row),
                    String::new(),
                    row.info.os.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    client_id.to_string(),
                    String::new(),
                    "unknown-os".to_string(),
                )
            })
    }

    fn handle_command_ack(
        &mut self,
        client_id: String,
        command: CommandKind,
        accepted: bool,
        detail: String,
    ) {
        if accepted && detail == "forwarded" {
            user_interaction::handle_ack(
                &mut self.interaction_command_windows,
                &client_id,
                &command,
                accepted,
                &detail,
            );
            return;
        }

        self.push_log(format!(
            "<< command={} from {} accepted={}",
            command.as_str(),
            self.client_display_name(&client_id),
            accepted
        ));

        if command == CommandKind::TextChat {
            self.handle_chat_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::VoiceChat {
            self.handle_voice_chat_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::FileManager {
            self.handle_file_manager_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::RemoteTerminal {
            self.handle_terminal_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::RemoteDesktop {
            self.handle_desktop_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::Camera {
            self.handle_camera_ack(&client_id, accepted, detail);
            return;
        }
        if command == CommandKind::AudioListen {
            self.handle_audio_ack(&client_id, accepted, detail);
            return;
        }
        if crate::execute::handle_ack(
            &mut self.execute_windows,
            &client_id,
            &command,
            accepted,
            &detail,
        ) {
            return;
        }
        if command == CommandKind::ClientConfig
            && crate::session::handle_client_config_ack(
                &mut self.session_command_windows,
                &client_id,
                accepted,
                &detail,
            )
        {
            self.push_log(format!("client config snapshot received from {client_id}"));
            return;
        }
        if command == CommandKind::UpdateClient
            && crate::session::handle_update_client_ack(
                &mut self.session_command_windows,
                &client_id,
                accepted,
                &detail,
            )
        {
            self.push_log(format!(
                "update client result client={} accepted={} detail={}",
                client_id,
                accepted,
                sanitize_log_value(&detail)
            ));
            return;
        }
        if session_command_requires_confirmation(&command) {
            self.push_log(format!(
                "result client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                sanitize_log_value(&detail)
            ));
            if accepted
                && command == CommandKind::DeleteClient
                && detail_status(&detail).as_deref() == Some("scheduled")
            {
                self.remove_client_row(&client_id);
            }
            return;
        }
        if user_interaction::handle_ack(
            &mut self.interaction_command_windows,
            &client_id,
            &command,
            accepted,
            &detail,
        ) {
            self.push_log(format!(
                "result client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                sanitize_log_value(&detail)
            ));
            return;
        }
        if quiet_user_interaction_command(&command) {
            self.push_log(format!(
                "result client={} command={} accepted={} detail={}",
                client_id,
                command.as_str(),
                accepted,
                sanitize_log_value(&detail)
            ));
            return;
        }

        let (hostname, username) = self.client_window_identity(&client_id);
        if let Some(window) = self.command_windows.iter_mut().rev().find(|window| {
            window.client_id == client_id
                && window.command == command
                && matches!(window.status, CommandResultStatus::Pending)
        }) {
            if window.command == CommandKind::PerformanceMonitor
                && window.close_requested.load(Ordering::Relaxed)
            {
                window.status = if accepted {
                    CommandResultStatus::Accepted
                } else {
                    CommandResultStatus::Failed
                };
                window.detail = detail;
                window.hostname = hostname;
                window.username = username;
                window.status_notice = None;
                window.open = false;
                window.auto_refresh_enabled.store(false, Ordering::Relaxed);
                window.last_auto_refresh_at = None;
                return;
            }
            update_command_window(window, accepted, detail, hostname, username);
            return;
        }

        if let Some(window) = self
            .command_windows
            .iter_mut()
            .rev()
            .find(|window| window.client_id == client_id && window.command == command)
        {
            update_command_window(window, accepted, detail, hostname, username);
            return;
        }

        if command == CommandKind::KillTargetProcess {
            let succeeded = accepted && kill_target_process_succeeded(&detail);
            if let Some(window) = self.command_windows.iter_mut().rev().find(|window| {
                window.client_id == client_id && window.command == CommandKind::ProcessManager
            }) {
                window.status = if succeeded {
                    CommandResultStatus::Accepted
                } else {
                    CommandResultStatus::Failed
                };
                window.status_notice = Some(kill_target_process_notice(succeeded, &detail));
                window.open = true;
            }
            self.push_log(format!(
                "kill target process result client={} accepted={} detail={}",
                client_id, accepted, detail
            ));
            if succeeded {
                self.refresh_process_window(&client_id);
                self.refresh_window_manager_window(&client_id);
            }
            return;
        }

        self.command_windows.push(CommandResultWindow {
            id: self.next_command_window_id(),
            client_id,
            hostname,
            username,
            command,
            status: if accepted {
                CommandResultStatus::Accepted
            } else {
                CommandResultStatus::Failed
            },
            detail,
            status_notice: None,
            open: true,
            close_requested: Arc::new(AtomicBool::new(false)),
            refresh_requested: Arc::new(AtomicBool::new(false)),
            auto_refresh_enabled: Arc::new(AtomicBool::new(false)),
            last_auto_refresh_at: None,
            process_kill_confirm: Arc::new(Mutex::new(None)),
            process_kill_requested: Arc::new(Mutex::new(None)),
            startup_delete_confirm: Arc::new(Mutex::new(None)),
            startup_detail: Arc::new(Mutex::new(None)),
            startup_action_requested: Arc::new(Mutex::new(None)),
            registry_key_requested: Arc::new(Mutex::new(None)),
            registry_expanded_keys: Arc::new(Mutex::new(HashSet::new())),
            startup_add_form: Arc::new(Mutex::new(StartupAddForm::default())),
            table_filter: Arc::new(Mutex::new(String::new())),
            table_sort: Arc::new(Mutex::new(None)),
            table_selected_row: Arc::new(Mutex::new(None)),
        });
    }

    fn remove_client_row(&mut self, client_id: &str) {
        self.clients.retain(|row| row.info.id != client_id);
        if self.selected_client_id.as_deref() == Some(client_id) {
            self.selected_client_id = self.clients.first().map(|row| row.info.id.clone());
        }
    }

    fn handle_chat_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::text_chat::handle_ack(
            &mut self.chat_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn handle_voice_chat_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let mut accepted = accepted;
        let mut detail = detail;
        if accepted && detail.starts_with("voice_chat_accepted") {
            if let Err(error) = self.set_voice_udp_sender(client_id, &detail) {
                self.remove_voice_udp_sender(client_id);
                self.stop_voice_udp_session(client_id);
                accepted = false;
                detail = format!("voice_chat_error\nmessage={error}");
            }
        } else if !accepted
            || detail.starts_with("voice_chat_ended")
            || detail.starts_with("voice_chat_error")
            || detail.starts_with("voice_chat_declined")
        {
            self.remove_voice_udp_sender(client_id);
            self.stop_voice_udp_session(client_id);
        }
        let (hostname, username) = self.client_window_identity(client_id);
        user_interaction::voice_chat::handle_ack(
            &mut self.voice_chat_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
            self.voice_audio_tx.clone(),
        );
    }

    fn handle_file_manager_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        remote_management::file_manager::handle_ack(
            &mut self.file_manager_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn handle_terminal_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username, os) = self.client_window_environment(client_id);
        remote_management::remote_terminal::handle_ack(
            &mut self.terminal_windows,
            client_id,
            hostname,
            username,
            os,
            accepted,
            detail,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_command_output(
        &mut self,
        client_id: String,
        command: CommandKind,
        stream_id: u64,
        sequence: u64,
        stream: CommandOutputStream,
        chunk: String,
        current_dir: String,
        finished: bool,
        success: bool,
    ) {
        if command != CommandKind::RemoteTerminal {
            self.push_log(format!(
                "ignored command output client={} command={}",
                client_id,
                command.as_str()
            ));
            return;
        }
        let (hostname, username, os) = self.client_window_environment(&client_id);
        remote_management::remote_terminal::handle_output(
            &mut self.terminal_windows,
            &client_id,
            hostname,
            username,
            os,
            stream_id,
            sequence,
            stream,
            chunk,
            current_dir,
            finished,
            success,
        );
    }

    fn handle_desktop_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::remote_desktop::handle_ack(
            &mut self.desktop_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn handle_camera_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::camera::handle_ack(
            &mut self.camera_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn handle_audio_ack(&mut self, client_id: &str, accepted: bool, detail: String) {
        let (hostname, username) = self.client_window_identity(client_id);
        live_control::audio_listen::handle_ack(
            &mut self.audio_windows,
            client_id,
            hostname,
            username,
            accepted,
            detail,
        );
    }

    fn refresh_process_window(&mut self, client_id: &str) {
        self.refresh_command_result_window(
            client_id,
            CommandKind::ProcessManager,
            t("Refreshing..."),
        );
    }

    fn refresh_window_manager_window(&mut self, client_id: &str) {
        self.refresh_command_result_window(
            client_id,
            CommandKind::WindowManager,
            t("Refreshing..."),
        );
    }

    fn refresh_command_result_window(
        &mut self,
        client_id: &str,
        command: CommandKind,
        pending_detail: &str,
    ) {
        let Some(window) = self
            .command_windows
            .iter_mut()
            .rev()
            .find(|window| window.client_id == client_id && window.command == command)
        else {
            return;
        };

        let _ = self.input_tx.send(AdminInput::Command {
            target_id: client_id.to_string(),
            command: command.clone(),
            payload: String::new(),
        });
        window.status = CommandResultStatus::Pending;
        window.status_notice = Some(pending_detail.to_string());
        if !detail_has_result_table(&window.detail) {
            window.detail = pending_detail.to_string();
        }
        window.open = true;
        self.push_log(format!(
            "refresh command={} to {client_id}",
            command.as_str()
        ));
    }

    fn render_menu_bar(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button(format!("🌐 {}", t("Client Map"))).clicked() {
                    self.client_map_window.open();
                }
                if ui.button(format!("🛠 {}", t("Client Builder"))).clicked() {
                    self.client_builder_open = true;
                }
                ui.menu_button(format!("🔧 {}", t("Tools")), |ui| {
                    if ui.button(t("P2P Test")).clicked() {
                        self.p2p_test.open();
                        ui.close();
                    }
                });
                if let Some(client_id) = self.selected_client_id.clone() {
                    ui.separator();
                    let status = self
                        .client_status_for(&client_id)
                        .unwrap_or(ClientStatus::Offline);
                    if status.can_receive_commands() {
                        command_menu::render_toolbar_actions(
                            ui,
                            &client_id,
                            &mut |client_id, command| {
                                self.send_command(client_id, command);
                            },
                        );
                    } else {
                        let (_, color) = client_status_display(status);
                        ui.label(
                            egui::RichText::new(client_commands_disabled_text(status))
                                .size(12.0)
                                .color(color)
                                .strong(),
                        )
                        .on_hover_text(t(
                            "Remote commands become available when the client reconnects",
                        ));
                    }
                } else {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(t("Select a client for actions"))
                            .size(12.0)
                            .color(crate::theme::palette().muted),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(format!("⚙ {}", t("Setting"))).clicked() {
                        self.settings.open();
                    }
                    ui.separator();
                });
            });
        });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, t("Activity"));
            ui.add_space(6.0);
            let output = egui::ScrollArea::vertical()
                .id_salt("admin_activity_scroll_area")
                .stick_to_bottom(true)
                .max_height(120.0)
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(ui.available_width());
                        for line in &self.log_lines {
                            ui.label(
                                egui::RichText::new(line)
                                    .size(12.0)
                                    .color(crate::theme::palette().muted),
                            );
                        }
                    });
                });
            activity_context_menu(ui, output.inner_rect, output.id, &mut self.log_lines);
        });
    }

    fn render_command_windows(&mut self, ctx: &egui::Context) {
        let mut pending_logs = Vec::new();
        let now = Instant::now();
        for window in &mut self.command_windows {
            if window.close_requested.load(Ordering::Relaxed) {
                window.open = false;
                window.auto_refresh_enabled.store(false, Ordering::Relaxed);
                window.last_auto_refresh_at = None;
            }
            if window.refresh_requested.swap(false, Ordering::Relaxed) {
                refresh_command_window(
                    &self.input_tx,
                    window,
                    t("Refreshing..."),
                    "refresh",
                    now,
                    &mut pending_logs,
                );
            }
            if performance_auto_refresh_due(window, now) {
                refresh_command_window(
                    &self.input_tx,
                    window,
                    t("Refreshing..."),
                    "auto_refresh",
                    now,
                    &mut pending_logs,
                );
            }
            if window.command == CommandKind::StartupManager {
                let startup_payload = window
                    .startup_action_requested
                    .lock()
                    .ok()
                    .and_then(|mut value| value.take());
                if let Some(payload) = startup_payload {
                    let action = payload_field(&payload, "action")
                        .unwrap_or_else(|| "startup_action".to_string());
                    let _ = self.input_tx.send(AdminInput::Command {
                        target_id: window.client_id.clone(),
                        command: CommandKind::StartupManager,
                        payload,
                    });
                    window.status = CommandResultStatus::Pending;
                    window.status_notice = Some(t("Waiting for client result...").to_string());
                    window.open = true;
                    pending_logs.push(format!(
                        "startup manager {} on {}",
                        action, window.client_id
                    ));
                }
            }
            if window.command == CommandKind::RegistryManager {
                let registry_payload = window
                    .registry_key_requested
                    .lock()
                    .ok()
                    .and_then(|mut value| value.take());
                if let Some(payload) = registry_payload {
                    let _ = self.input_tx.send(AdminInput::Command {
                        target_id: window.client_id.clone(),
                        command: CommandKind::RegistryManager,
                        payload,
                    });
                    window.status = CommandResultStatus::Pending;
                    window.status_notice = Some(t("Waiting for client result...").to_string());
                    window.open = true;
                    pending_logs.push(format!("registry key request on {}", window.client_id));
                }
            }
            let process_id = window
                .process_kill_requested
                .lock()
                .ok()
                .and_then(|mut value| value.take());
            if let Some(process_id) = process_id {
                let _ = self.input_tx.send(AdminInput::Command {
                    target_id: window.client_id.clone(),
                    command: CommandKind::KillTargetProcess,
                    payload: process_id.clone(),
                });
                window.status = CommandResultStatus::Pending;
                window.status_notice = Some(t("Waiting for client result...").to_string());
                window.open = true;
                pending_logs.push(format!(
                    "kill target process pid={} on {}",
                    process_id, window.client_id
                ));
            }
        }
        for line in pending_logs {
            self.push_log(line);
        }

        for window in &mut self.command_windows {
            if !window.open {
                continue;
            }
            let title = format!(
                "{} - {}",
                command_title(&window.command),
                command_window_identity_title(&window.hostname, &window.username)
            );
            let viewport_id = egui::ViewportId::from_hash_of(("command_result", window.id));
            let builder = windowing::child_viewport_builder(title, [760.0, 460.0], [260.0, 180.0]);

            let command = window.command.clone();
            let status = window.status;
            let detail = window.detail.clone();
            let explicit_status_notice = window.status_notice.clone();
            let close_requested = window.close_requested.clone();
            let refresh_requested = window.refresh_requested.clone();
            let auto_refresh_enabled = window.auto_refresh_enabled.clone();
            let process_kill_confirm = window.process_kill_confirm.clone();
            let process_kill_requested = window.process_kill_requested.clone();
            let startup_delete_confirm = window.startup_delete_confirm.clone();
            let startup_detail = window.startup_detail.clone();
            let startup_action_requested = window.startup_action_requested.clone();
            let registry_key_requested = window.registry_key_requested.clone();
            let registry_expanded_keys = window.registry_expanded_keys.clone();
            let startup_add_form = window.startup_add_form.clone();
            let table_filter = window.table_filter.clone();
            let table_sort = window.table_sort.clone();
            let table_selected_row = window.table_selected_row.clone();
            let status_notice =
                explicit_status_notice.or_else(|| command_status_notice(&command, status, &detail));

            ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
                if ui.ctx().input(|input| input.viewport().close_requested()) {
                    close_requested.store(true, Ordering::Relaxed);
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
                                egui::ScrollArea::both()
                                    .id_salt(("command_result_scroll", viewport_id))
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let mut detail = detail.clone();
                                        let render_state = CommandResultRenderState {
                                            table_filter: &table_filter,
                                            table_sort: &table_sort,
                                            table_selected_row: &table_selected_row,
                                            refresh_requested: &refresh_requested,
                                            auto_refresh_enabled: &auto_refresh_enabled,
                                            refresh_in_flight: matches!(
                                                status,
                                                CommandResultStatus::Pending
                                            ),
                                            process_kill_confirm: &process_kill_confirm,
                                            process_kill_requested: &process_kill_requested,
                                            startup_delete_confirm: &startup_delete_confirm,
                                            startup_detail: &startup_detail,
                                            startup_action_requested: &startup_action_requested,
                                            registry_key_requested: &registry_key_requested,
                                            registry_expanded_keys: &registry_expanded_keys,
                                            startup_add_form: &startup_add_form,
                                        };
                                        render_command_result(
                                            ui,
                                            &command,
                                            &mut detail,
                                            render_state,
                                        );
                                    });
                            },
                        );
                        ui.add_space(STATUS_BAR_GAP);
                        render_command_window_status_bar(ui, &status, status_notice.as_deref());
                    });
            });
        }
        self.command_windows
            .retain(|window| window.open || matches!(window.status, CommandResultStatus::Pending));
    }

    fn render_chat_windows(&mut self, ctx: &egui::Context) {
        for outbound in user_interaction::text_chat::render_windows(ctx, &mut self.chat_windows) {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: CommandKind::TextChat,
                payload: outbound.text,
            });
            self.push_log(format!("sent text_chat to {}", outbound.client_id));
        }
    }

    fn render_voice_chat_windows(&mut self, ctx: &egui::Context) {
        for outbound in
            user_interaction::voice_chat::render_windows(ctx, &mut self.voice_chat_windows)
        {
            match outbound {
                user_interaction::voice_chat::OutboundCommand::Command {
                    client_id,
                    mut payload,
                } => {
                    if payload_field(&payload, "action").as_deref() == Some("invite") {
                        let Some(stream_id) = self.start_voice_udp_session(&client_id) else {
                            self.handle_voice_chat_ack(
                                &client_id,
                                false,
                                "voice_chat_error\nmessage=voice udp setup failed".to_string(),
                            );
                            continue;
                        };
                        let client_receive_stream_id = self.next_audio_udp_stream_id();
                        let client_receive_endpoint = AudioUdpEndpoint {
                            host: self.config.ip.clone(),
                            port: self.config.port,
                            stream_id: client_receive_stream_id,
                        };
                        if let Err(error) =
                            self.set_voice_udp_sender_endpoint(&client_id, &client_receive_endpoint)
                        {
                            self.stop_voice_udp_session(&client_id);
                            self.handle_voice_chat_ack(
                                &client_id,
                                false,
                                format!("voice_chat_error\nmessage={error}"),
                            );
                            continue;
                        }
                        payload.push_str(&format!(
                            "\ntransport=udp\nudp_host={}\nudp_port={}\nudp_stream={stream_id}\nudp_return_stream={client_receive_stream_id}",
                            self.config.ip, self.config.port
                        ));
                    }
                    let _ = self.input_tx.send(AdminInput::Command {
                        target_id: client_id.clone(),
                        command: CommandKind::VoiceChat,
                        payload,
                    });
                    self.push_log(format!("sent voice_chat invite to {client_id}"));
                }
                user_interaction::voice_chat::OutboundCommand::AudioControl {
                    client_id,
                    payload,
                } => {
                    match payload_field(&payload, "action").as_deref() {
                        Some("stop") | Some("end") => {
                            self.remove_voice_udp_sender(&client_id);
                            self.stop_voice_udp_session(&client_id);
                        }
                        _ => {}
                    }
                    let _ = self.input_tx.send(AdminInput::AudioControl {
                        target_id: client_id,
                        source: AudioSource::VoiceChat,
                        payload,
                    });
                }
                user_interaction::voice_chat::OutboundCommand::AudioFrame {
                    client_id,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                } => {
                    let mut remove_sender = false;
                    if let Ok(mut senders) = self.voice_udp_senders.lock() {
                        if let Some(sender) = senders.get_mut(&client_id) {
                            if sender
                                .send_frame(&client_id, seq, sample_rate, channels, &format, &bytes)
                                .is_err()
                            {
                                remove_sender = true;
                            }
                        }
                        if remove_sender {
                            senders.remove(&client_id);
                        }
                    }
                }
            }
        }
    }

    fn render_interaction_command_windows(&mut self, ctx: &egui::Context) {
        for outbound in user_interaction::render_windows(ctx, &mut self.interaction_command_windows)
        {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: outbound.command.clone(),
                payload: outbound.payload,
            });
            self.push_log(format!(
                ">> command={} to {}",
                outbound.command.as_str(),
                self.client_display_name(&outbound.client_id)
            ));
        }
    }

    fn render_session_command_windows(&mut self, ctx: &egui::Context) {
        for action in crate::session::render_windows(ctx, &mut self.session_command_windows) {
            match action {
                crate::session::OutboundSessionAction::Command(outbound) => {
                    self.send_session_command(outbound);
                }
                crate::session::OutboundSessionAction::UpdateUpload(request) => {
                    self.handle_session_update_upload(request);
                }
            }
        }
    }

    fn send_session_command(&mut self, outbound: crate::session::OutboundSessionCommand) {
        let _ = self.input_tx.send(AdminInput::Command {
            target_id: outbound.client_id.clone(),
            command: outbound.command.clone(),
            payload: outbound.payload,
        });
        self.push_log(format!(
            ">> command={} to {}",
            outbound.command.as_str(),
            self.client_display_name(&outbound.client_id)
        ));
    }

    fn handle_session_update_upload(&mut self, request: crate::session::UpdateUploadRequest) {
        self.unignore_file_transfer(&request.client_id, request.transfer_id);
        debug_log!(
            "debug event=admin_update_upload_request client={} id={} local_path={} remote_path={}",
            request.client_id,
            request.transfer_id,
            request.local_path,
            request.remote_path
        );
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let input_tx = self.input_tx.clone();
        let sink = AdminEventSink::new(
            self.event_tx.clone(),
            Some(self.repaint_handle.clone()),
            None,
        );
        let client_id = request.client_id.clone();
        let transfer_id = request.transfer_id;
        let local_path = request.local_path.clone();
        let remote_path = request.remote_path.clone();
        thread::spawn(move || {
            let result = run_file_upload_transfer(
                &input_tx,
                &client_id,
                transfer_id,
                &local_path,
                &remote_path,
                cancel_flag,
            );
            if let Err(error) = result {
                let _ = send_upload_cancel(&input_tx, &client_id, transfer_id, &remote_path);
                if error.kind() == io::ErrorKind::Interrupted {
                    return;
                }
                sink.send(AdminEvent::FileTransfer(file_transfer_message(
                    client_id,
                    transfer_id,
                    FileTransferDirection::Upload,
                    FileTransferAction::Error,
                    remote_path.clone(),
                    String::new(),
                    0,
                    0,
                    0,
                    0,
                    Vec::new(),
                    format!("upload failed: {error}"),
                )));
            }
        });
        self.push_log(format!(
            "queued update upload id={} to {}",
            request.transfer_id, request.client_id
        ));
    }

    fn render_execute_windows(&mut self, ctx: &egui::Context) {
        for outbound in crate::execute::render_windows(ctx, &mut self.execute_windows) {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: outbound.command.clone(),
                payload: outbound.payload,
            });
            self.push_log(format!(
                ">> command={} to {}",
                outbound.command.as_str(),
                self.client_display_name(&outbound.client_id)
            ));
        }
    }

    fn render_file_manager_windows(&mut self, ctx: &egui::Context) {
        for outbound in
            remote_management::file_manager::render_windows(ctx, &mut self.file_manager_windows)
        {
            if let Some(request) =
                remote_management::file_manager::parse_transfer_request(&outbound.payload)
            {
                self.handle_file_transfer_request(outbound.client_id.clone(), request);
            } else {
                let input_tx = self.input_tx.clone();
                let client_id = outbound.client_id.clone();
                let action = payload_field(&outbound.payload, "action")
                    .unwrap_or_else(|| "list".to_string());
                let path = payload_field(&outbound.payload, "path").unwrap_or_default();
                debug_log!(
                    "debug event=admin_file_manager_send client={} action={} path={}",
                    outbound.client_id,
                    action,
                    path
                );
                thread::spawn(move || {
                    let _ = input_tx.send(AdminInput::Command {
                        target_id: client_id,
                        command: CommandKind::FileManager,
                        payload: outbound.payload,
                    });
                });
                self.push_log(format!("sent file_manager to {}", outbound.client_id));
            }
        }
    }

    fn handle_file_transfer_request(
        &mut self,
        client_id: String,
        request: remote_management::file_manager::FileTransferRequest,
    ) {
        match request {
            remote_management::file_manager::FileTransferRequest::Upload {
                transfer_id,
                local_path,
                remote_path,
            } => {
                self.unignore_file_transfer(&client_id, transfer_id);
                debug_log!(
                    "debug event=admin_file_transfer_request client={} id={} direction=upload local_path={} remote_path={}",
                    client_id, transfer_id, local_path, remote_path
                );
                let cancel_flag = Arc::new(AtomicBool::new(false));
                if let Ok(mut flags) = self.file_transfer_cancel_flags.lock() {
                    flags.insert(transfer_id, cancel_flag.clone());
                }
                let input_tx = self.input_tx.clone();
                let flags = self.file_transfer_cancel_flags.clone();
                let sink = AdminEventSink::new(
                    self.event_tx.clone(),
                    Some(self.repaint_handle.clone()),
                    None,
                );
                let worker_client_id = client_id.clone();
                thread::spawn(move || {
                    let result = run_file_upload_transfer(
                        &input_tx,
                        &worker_client_id,
                        transfer_id,
                        &local_path,
                        &remote_path,
                        cancel_flag,
                    );
                    if let Ok(mut flags) = flags.lock() {
                        flags.remove(&transfer_id);
                    }
                    if let Err(error) = result {
                        let _ = send_upload_cancel(
                            &input_tx,
                            &worker_client_id,
                            transfer_id,
                            &remote_path,
                        );
                        if error.kind() == io::ErrorKind::Interrupted {
                            return;
                        }
                        sink.send(AdminEvent::FileTransfer(file_transfer_message(
                            worker_client_id,
                            transfer_id,
                            FileTransferDirection::Upload,
                            FileTransferAction::Error,
                            remote_path.clone(),
                            String::new(),
                            0,
                            0,
                            0,
                            0,
                            Vec::new(),
                            format!("upload failed: {error}"),
                        )));
                    }
                });
                self.push_log(format!(
                    "queued file upload id={transfer_id} to {client_id}"
                ));
            }
            remote_management::file_manager::FileTransferRequest::Download {
                transfer_id,
                remote_path,
                local_dir,
            } => {
                self.unignore_file_transfer(&client_id, transfer_id);
                debug_log!(
                    "debug event=admin_file_transfer_request client={} id={} direction=download remote_path={} local_dir={}",
                    client_id, transfer_id, remote_path, local_dir
                );
                let input_tx = self.input_tx.clone();
                let download_message = file_transfer_message(
                    client_id.clone(),
                    transfer_id,
                    FileTransferDirection::Download,
                    FileTransferAction::Start,
                    remote_path,
                    String::new(),
                    0,
                    0,
                    0,
                    0,
                    Vec::new(),
                    local_dir,
                );
                thread::spawn(move || {
                    let _ = send_file_transfer_input(&input_tx, download_message);
                });
                self.push_log(format!(
                    "queued file download id={transfer_id} from {client_id}"
                ));
            }
            remote_management::file_manager::FileTransferRequest::Cancel {
                transfer_id,
                direction,
                remote_path,
            } => {
                let should_reconnect_after_cancel = direction == FileTransferDirection::Download;
                self.ignore_file_transfer(&client_id, transfer_id);
                debug_log!(
                    "debug event=admin_file_transfer_request client={} id={} direction={} action=cancel remote_path={}",
                    client_id,
                    transfer_id,
                    direction.as_str(),
                    remote_path
                );
                if let Ok(flags) = self.file_transfer_cancel_flags.lock() {
                    if let Some(flag) = flags.get(&transfer_id) {
                        flag.store(true, Ordering::Relaxed);
                    }
                }
                let input_tx = self.input_tx.clone();
                let cancel_message = file_transfer_message(
                    client_id.clone(),
                    transfer_id,
                    direction,
                    FileTransferAction::Cancel,
                    remote_path,
                    String::new(),
                    0,
                    0,
                    0,
                    0,
                    Vec::new(),
                    "cancel requested".to_string(),
                );
                thread::spawn(move || {
                    let _ = send_file_transfer_input(&input_tx, cancel_message);
                    if should_reconnect_after_cancel {
                        let _ = input_tx.send(AdminInput::Reconnect {
                            reason: format!("cancelled download transfer id={transfer_id}"),
                            endpoint: None,
                        });
                    }
                });
                self.push_log(format!(
                    "cancel file transfer id={transfer_id} on {client_id}"
                ));
            }
        }
    }

    fn render_desktop_windows(&mut self, ctx: &egui::Context) {
        for outbound in live_control::remote_desktop::render_windows(ctx, &mut self.desktop_windows)
        {
            let message = if outbound.input {
                AdminInput::DesktopInput {
                    target_id: outbound.client_id.clone(),
                    payload: outbound.payload,
                }
            } else if video_stream_payload(&outbound.payload) {
                AdminInput::VideoControl {
                    target_id: outbound.client_id.clone(),
                    source: VideoSource::RemoteDesktop,
                    payload: outbound.payload,
                }
            } else {
                AdminInput::DesktopControl {
                    target_id: outbound.client_id.clone(),
                    payload: outbound.payload,
                }
            };
            let _ = self.input_tx.send(message);
        }
    }

    fn render_camera_windows(&mut self, ctx: &egui::Context) {
        for outbound in live_control::camera::render_windows(ctx, &mut self.camera_windows) {
            let message = if video_stream_payload(&outbound.payload) {
                AdminInput::VideoControl {
                    target_id: outbound.client_id.clone(),
                    source: VideoSource::Camera,
                    payload: outbound.payload,
                }
            } else {
                AdminInput::Command {
                    target_id: outbound.client_id.clone(),
                    command: CommandKind::Camera,
                    payload: outbound.payload,
                }
            };
            let _ = self.input_tx.send(message);
        }
    }

    fn render_audio_windows(&mut self, ctx: &egui::Context) {
        for mut outbound in live_control::audio_listen::render_windows(ctx, &mut self.audio_windows)
        {
            match payload_field(&outbound.payload, "action").as_deref() {
                Some("start") => {
                    if let Some(stream_id) = self.start_audio_udp_session(&outbound.client_id) {
                        outbound.payload.push_str(&format!(
                            "\ntransport=udp\nudp_host={}\nudp_port={}\nudp_stream={stream_id}",
                            self.config.ip, self.config.port
                        ));
                    }
                }
                Some("stop") => self.stop_audio_udp_session(&outbound.client_id),
                _ => {}
            }
            let message = if video_stream_payload(&outbound.payload) {
                AdminInput::AudioControl {
                    target_id: outbound.client_id.clone(),
                    source: AudioSource::AudioListen,
                    payload: outbound.payload,
                }
            } else {
                AdminInput::Command {
                    target_id: outbound.client_id.clone(),
                    command: CommandKind::AudioListen,
                    payload: outbound.payload,
                }
            };
            let _ = self.input_tx.send(message);
        }
    }

    fn render_terminal_windows(&mut self, ctx: &egui::Context) {
        for outbound in
            remote_management::remote_terminal::render_windows(ctx, &mut self.terminal_windows)
        {
            let _ = self.input_tx.send(AdminInput::Command {
                target_id: outbound.client_id.clone(),
                command: CommandKind::RemoteTerminal,
                payload: outbound.command,
            });
            self.push_log(format!("sent remote_terminal to {}", outbound.client_id));
        }
    }

    fn render_proxy_windows(&mut self, ctx: &egui::Context) {
        remote_management::reverse_proxy::render_windows(
            ctx,
            &mut self.proxy_windows,
            &self.input_tx,
        );
    }
}

impl eframe::App for AdminApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let theme = crate::theme::ThemeKind::from_config(&self.config.theme);
        let resolved_theme = crate::theme::resolve_theme(ui.ctx(), theme);
        if self.applied_theme != Some((theme, resolved_theme)) {
            let resolved_theme = apply_admin_theme(ui.ctx(), theme);
            self.applied_theme = Some((theme, resolved_theme));
        }

        let changed = self.drain_events();

        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, crate::theme::palette().bg);
        let horizontal_margin = if ui.available_width() < 880.0 { 10 } else { 18 };
        let content_height = (ui.available_height()
            - STATUS_BAR_HEIGHT
            - STATUS_BAR_GAP
            - ROOT_STATUS_BAR_BOTTOM_MARGIN)
            .max(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), content_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Frame::default()
                            .inner_margin(egui::Margin::symmetric(horizontal_margin, 18))
                            .show(ui, |ui| {
                                ui.with_layout(
                                    egui::Layout::top_down_justified(egui::Align::Min),
                                    |ui| {
                                        self.render_menu_bar(ui);
                                        ui.add_space(12.0);
                                        self.render_overview(ui);
                                        ui.add_space(12.0);
                                        self.render_clients(ui);
                                        ui.add_space(12.0);
                                        self.render_activity(ui);
                                    },
                                );
                            });
                    });
            },
        );
        ui.add_space(STATUS_BAR_GAP);
        egui::Frame::default()
            .inner_margin(egui::Margin::symmetric(horizontal_margin, 0))
            .show(ui, |ui| {
                self.render_status_bar(ui);
            });
        ui.add_space(ROOT_STATUS_BAR_BOTTOM_MARGIN);
        self.render_child_windows(ui.ctx());
        if let Some(log_line) =
            self.client_builder
                .render(ui.ctx(), &mut self.client_builder_open, &self.config)
        {
            self.push_log(log_line);
        }
        self.client_map_window.render(
            ui.ctx(),
            &self.clients,
            &self.client_aliases,
            &mut self.selected_client_id,
            &mut self.client_filter,
        );
        self.render_client_online_toasts(ui.ctx());

        if changed {
            ui.ctx().request_repaint();
        } else {
            let interval_ms = if self.realtime_audio_active() {
                GUI_REALTIME_AUDIO_FRAME_INTERVAL_MS
            } else {
                GUI_IDLE_FRAME_INTERVAL_MS
            };
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(interval_ms));
        }
    }
}

fn command_requires_client_ui(command: &CommandKind) -> bool {
    matches!(command, CommandKind::TextChat | CommandKind::VoiceChat)
}

fn kill_target_process_notice(succeeded: bool, detail: &str) -> String {
    let title = if succeeded {
        t("Kill process completed")
    } else {
        t("Kill process failed")
    };
    let message = compact_command_detail(detail);
    if message.is_empty() {
        title.to_string()
    } else {
        format!("{title}: {message}")
    }
}

fn compact_command_detail(detail: &str) -> String {
    detail
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.ends_with(':'))
        .collect::<Vec<_>>()
        .join(" ")
}

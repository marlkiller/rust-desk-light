#[cfg(feature = "gui")]
use crate::app_event::ClientInput;
use crate::app_event::{ClientEvent, ClientEventSink};
#[cfg(feature = "gui")]
use crate::app_ui::{
    activity_context_menu, apply_client_theme, detail_row, panel, prune_activity_logs,
    section_title, status_pill, timestamped_log, COLOR_BG, COLOR_MUTED, COLOR_TEXT,
};
use crate::client_network::client_network_loop;
use crate::runtime::{gui_available, load_client_identity, Config};
#[cfg(feature = "gui")]
use crate::runtime::{hostname, username, LocalIdentity};
#[cfg(feature = "gui")]
use crate::user_interaction;
#[cfg(feature = "gui")]
use eframe::egui;
use std::io;
use std::sync::mpsc;
#[cfg(feature = "gui")]
use std::sync::mpsc::Sender;
#[cfg(feature = "gui")]
use std::sync::Mutex;
#[cfg(feature = "gui")]
use std::sync::{mpsc::Receiver, Arc};
use std::thread;

#[cfg(feature = "gui")]
const GUI_FRAME_INTERVAL_MS: u64 = 16;

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let process_lock = crate::runtime::acquire_client_process_lock()?;
    debug_log!(
        "debug event=client_process_lock path={}",
        process_lock.path().display()
    );

    let startup_notice_printed = if gui_available() {
        #[cfg(feature = "gui")]
        {
            eprintln!("{}", config.startup_config_notice());
            match run_gui(config.clone()) {
                Ok(()) => return Ok(()),
                Err(error) => eprintln!("GUI startup failed: {error}; falling back to terminal"),
            }
            true
        }
        #[cfg(not(feature = "gui"))]
        {
            false
        }
    } else {
        false
    };

    run_terminal(config, !startup_notice_printed)?;
    Ok(())
}

#[cfg(feature = "gui")]
fn run_gui(config: Config) -> eframe::Result {
    disable_macos_automatic_window_tabbing();

    let identity = load_client_identity();
    let (event_tx, event_rx) = mpsc::channel();
    let (input_tx, input_rx) = mpsc::channel();
    let app_config = config.clone();
    let network_identity = identity.clone();
    let repaint_handle = Arc::new(Mutex::new(None));
    let network_repaint_handle = repaint_handle.clone();

    thread::spawn(move || {
        let event_sink = ClientEventSink::new(event_tx, Some(network_repaint_handle));
        if let Err(error) = client_network_loop(
            app_config,
            network_identity,
            true,
            event_sink.clone(),
            input_rx,
        ) {
            event_sink.send(ClientEvent::Log(format!("network stopped: {error}")));
        }
    });

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([780.0, 520.0])
        .with_min_inner_size([680.0, 440.0]);
    let viewport = match rust_desk_light_assets::app_window_icon() {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let window_title = rdl_version::app_version("rust-desk-light client");

    eframe::run_native(
        &window_title,
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(ClientApp::new(
                cc,
                config,
                identity,
                event_rx,
                input_tx,
                repaint_handle,
            )))
        }),
    )
}

#[cfg(all(feature = "gui", target_os = "macos"))]
fn disable_macos_automatic_window_tabbing() {
    if let Some(main_thread) = objc2_foundation::MainThreadMarker::new() {
        objc2_app_kit::NSWindow::setAllowsAutomaticWindowTabbing(false, main_thread);
    }
}

#[cfg(all(feature = "gui", not(target_os = "macos")))]
fn disable_macos_automatic_window_tabbing() {}

fn run_terminal(config: Config, print_startup_notice: bool) -> io::Result<()> {
    let identity = load_client_identity();
    let (event_tx, event_rx) = mpsc::channel();
    let (_input_tx, input_rx) = mpsc::channel();
    #[cfg(feature = "gui")]
    let terminal_mode = "terminal fallback";
    #[cfg(not(feature = "gui"))]
    let terminal_mode = "terminal mode";
    println!(
        "rust-desk-light client {} {}",
        rdl_version::display_version(),
        terminal_mode
    );
    if print_startup_notice {
        println!("{}", config.startup_config_notice());
    }
    println!("client id: {}", identity.id);
    println!("fingerprint: {}", identity.fingerprint);
    println!("waiting for admin commands; press Ctrl+C to exit");

    thread::spawn(move || {
        let event_sink = ClientEventSink::new(event_tx, None);
        if let Err(error) =
            client_network_loop(config, identity, false, event_sink.clone(), input_rx)
        {
            event_sink.send(ClientEvent::Log(format!("network stopped: {error}")));
        }
    });

    for event in event_rx {
        match event {
            ClientEvent::Connected => println!("connected"),
            ClientEvent::Disconnected => println!("disconnected"),
            ClientEvent::Command { command, payload } => {
                println!(">> command={} payload={payload}", command.as_str());
            }
            ClientEvent::ChatMessage { text } => println!("text_chat={text}"),
            ClientEvent::VoiceChatInvite => println!("voice_chat=incoming"),
            ClientEvent::VoiceChatConnected => println!("voice_chat=connected"),
            ClientEvent::VoiceChatEnded { message } => println!("voice_chat=ended {message}"),
            ClientEvent::VoiceChatFailed { message } => println!("voice_chat=failed {message}"),
            ClientEvent::Log(line) => println!("{line}"),
        }
    }

    Ok(())
}

#[cfg(feature = "gui")]
struct ClientApp {
    config: Config,
    identity: LocalIdentity,
    input_tx: Sender<ClientInput>,
    event_rx: Receiver<ClientEvent>,
    connected: bool,
    log_lines: Vec<String>,
    chat_window: Option<user_interaction::text_chat::ChatWindow>,
    voice_chat_window: Option<user_interaction::voice_chat::VoiceChatWindow>,
}

#[cfg(feature = "gui")]
impl ClientApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        identity: LocalIdentity,
        event_rx: Receiver<ClientEvent>,
        input_tx: Sender<ClientInput>,
        repaint_handle: Arc<Mutex<Option<egui::Context>>>,
    ) -> Self {
        apply_client_theme(&cc.egui_ctx);
        if let Ok(mut handle) = repaint_handle.lock() {
            *handle = Some(cc.egui_ctx.clone());
        }
        Self {
            log_lines: vec![
                timestamped_log(format!(
                    "client gui started version={}",
                    rdl_version::display_version()
                )),
                timestamped_log(config.startup_config_notice()),
            ],
            config,
            identity,
            input_tx,
            event_rx,
            connected: false,
            chat_window: None,
            voice_chat_window: None,
        }
    }

    fn drain_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.event_rx.try_recv() {
            changed = true;
            match event {
                ClientEvent::Connected => {
                    self.connected = true;
                    self.push_log("connected to server");
                }
                ClientEvent::Disconnected => {
                    self.connected = false;
                    self.push_log("disconnected from server");
                }
                ClientEvent::Command { command, payload } => {
                    self.push_log(format!(
                        ">> command={} payload={payload}",
                        command.as_str()
                    ));
                }
                ClientEvent::ChatMessage { text } => {
                    user_interaction::text_chat::receive_admin_message(&mut self.chat_window, text);
                }
                ClientEvent::VoiceChatInvite => {
                    user_interaction::voice_chat::receive_invite(&mut self.voice_chat_window);
                    self.push_log("incoming voice_chat invite");
                }
                ClientEvent::VoiceChatConnected => {
                    user_interaction::voice_chat::mark_live(&mut self.voice_chat_window);
                    self.push_log("voice_chat connected");
                }
                ClientEvent::VoiceChatEnded { message } => {
                    user_interaction::voice_chat::mark_ended(
                        &mut self.voice_chat_window,
                        message.clone(),
                    );
                    self.push_log(format!("voice_chat ended: {message}"));
                }
                ClientEvent::VoiceChatFailed { message } => {
                    user_interaction::voice_chat::mark_failed(
                        &mut self.voice_chat_window,
                        message.clone(),
                    );
                    self.push_log(format!("voice_chat failed: {message}"));
                }
                ClientEvent::Log(line) => self.push_log(line),
            }
        }
        changed
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let line = timestamped_log(line);
        eprintln!("{line}");
        self.log_lines.push(line);
        prune_activity_logs(&mut self.log_lines);
    }

    fn render_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Rust Desk Light")
                        .size(22.0)
                        .color(COLOR_TEXT)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "Client Agent | {}",
                        rdl_version::display_version()
                    ))
                    .size(13.0)
                    .color(COLOR_MUTED),
                );
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_pill(ui, self.connected);
            });
        });
    }

    fn render_status(&self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Status");
            ui.add_space(10.0);
            egui::Grid::new("client_status_grid")
                .num_columns(2)
                .spacing([18.0, 10.0])
                .show(ui, |ui| {
                    detail_row(
                        ui,
                        "Connection",
                        if self.connected {
                            "Online"
                        } else {
                            "Connecting / Offline"
                        },
                    );
                    detail_row(ui, "Client ID", &self.identity.id);
                    detail_row(ui, "Fingerprint", &self.identity.fingerprint);
                    detail_row(
                        ui,
                        "Server",
                        &format!("{}:{}", self.config.ip, self.config.port),
                    );
                    detail_row(ui, "Config Mode", self.config.config_mode_label());
                    detail_row(ui, "Config Detail", &self.config.config_mode_detail());
                    detail_row(ui, "Version", &rdl_version::display_version());
                    detail_row(ui, "Host", &hostname());
                    detail_row(
                        ui,
                        "Runtime",
                        &format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
                    );
                    detail_row(ui, "User", &username());
                });
        });
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        panel(ui, |ui| {
            section_title(ui, "Activity");
            ui.add_space(8.0);
            let output = egui::ScrollArea::vertical()
                .id_salt("client_activity_scroll_area")
                .stick_to_bottom(true)
                .max_height(180.0)
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(ui.available_width());
                        for line in &self.log_lines {
                            ui.label(egui::RichText::new(line).size(12.0).color(COLOR_MUTED));
                        }
                    });
                });
            activity_context_menu(ui, output.inner_rect, output.id, &mut self.log_lines);
        });
    }
}

#[cfg(feature = "gui")]
impl eframe::App for ClientApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let changed = self.drain_events();

        ui.painter().rect_filled(ui.max_rect(), 0.0, COLOR_BG);
        ui.add_space(18.0);
        ui.vertical_centered_justified(|ui| {
            ui.set_max_width(700.0);
            self.render_header(ui);
            ui.add_space(14.0);
            self.render_status(ui);
            ui.add_space(12.0);
            self.render_activity(ui);
        });
        for text in user_interaction::text_chat::render_window(ui.ctx(), &mut self.chat_window) {
            let _ = self.input_tx.send(ClientInput::ChatReply { text });
        }
        for action in
            user_interaction::voice_chat::render_window(ui.ctx(), &mut self.voice_chat_window)
        {
            match action {
                user_interaction::voice_chat::VoiceChatAction::Accept => {
                    user_interaction::voice_chat::mark_connecting(&mut self.voice_chat_window);
                    let _ = self.input_tx.send(ClientInput::VoiceChatAccept);
                }
                user_interaction::voice_chat::VoiceChatAction::Decline => {
                    let _ = self.input_tx.send(ClientInput::VoiceChatDecline);
                }
                user_interaction::voice_chat::VoiceChatAction::End => {
                    let _ = self.input_tx.send(ClientInput::VoiceChatEnd);
                }
                user_interaction::voice_chat::VoiceChatAction::MicMuted(muted) => {
                    let _ = self.input_tx.send(ClientInput::VoiceChatMicMuted { muted });
                }
                user_interaction::voice_chat::VoiceChatAction::SpeakerMuted(muted) => {
                    let _ = self
                        .input_tx
                        .send(ClientInput::VoiceChatSpeakerMuted { muted });
                }
            }
        }

        if changed {
            ui.ctx().request_repaint();
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(GUI_FRAME_INTERVAL_MS));
        }
    }
}

use crate::{
    i18n::{t, tf},
    theme::{COLOR_BAD, COLOR_GOOD, COLOR_WARN},
    windowing,
};
use chrono::{Local, TimeZone};
use eframe::egui;
use egui_extras::{Column, Size, StripBuilder};
use rdl_protocol::{FileTransferAction, FileTransferDirection, Message};
use rfd::FileDialog;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

const TOOLBAR_CONTROL_HEIGHT: f32 = crate::theme::CONTROL_HEIGHT;
const TRANSFER_COLUMN_WIDTH: f32 = 100.0;
const TRANSFER_BUTTON_WIDTH: f32 = 84.0;
const TRANSFER_TABLE_HEIGHT: f32 = 150.0;
const TRANSFER_REQUEST_MARKER: &str = "file_transfer_request";
const QUICK_JUMPS: [(&str, &str); 4] = [
    ("User", "~"),
    ("Desktop", "~/Desktop"),
    ("Downloads", "~/Downloads"),
    ("Documents", "~/Documents"),
];

static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct FileManagerWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    current_path: Arc<Mutex<String>>,
    path_input: Arc<Mutex<String>>,
    entries: Arc<Mutex<Vec<FileEntry>>>,
    selected_name: FileSelection,
    local_entries: Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: FileSelection,
    status: Arc<Mutex<FileStatus>>,
    notice: Arc<Mutex<String>>,
    local_path: Arc<Mutex<String>>,
    rename_to: Arc<Mutex<String>>,
    new_folder_name: Arc<Mutex<String>>,
    local_rename_to: Arc<Mutex<String>>,
    local_new_folder_name: Arc<Mutex<String>>,
    pending_delete: Arc<Mutex<Option<String>>>,
    pending_rename: Arc<Mutex<Option<String>>>,
    pending_new_folder: Arc<Mutex<bool>>,
    pending_local_delete: Arc<Mutex<Option<String>>>,
    pending_local_rename: Arc<Mutex<Option<String>>>,
    pending_local_new_folder: Arc<Mutex<bool>>,
    outbound: Arc<Mutex<Vec<String>>>,
    transfers: Arc<Mutex<Vec<FileTransferRow>>>,
    open: bool,
    close_when_transfers_finish: bool,
    close_requested: Arc<AtomicBool>,
}

#[derive(Clone)]
struct FileEntry {
    kind: String,
    name: String,
    size: String,
    modified: String,
}

type FileSelection = Arc<Mutex<FileSelectionState>>;

#[derive(Clone, Default)]
struct FileSelectionState {
    names: Vec<String>,
}

#[derive(Clone, Copy)]
enum FileStatus {
    Ready,
    Pending,
    Done,
    Failed,
}

#[derive(Clone)]
struct FileTransferRow {
    transfer_id: u64,
    direction: FileTransferDirection,
    name: String,
    source: String,
    destination: String,
    remote_path: String,
    local_root: String,
    total_bytes: u64,
    transferred_bytes: u64,
    status: FileTransferStatus,
    message: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FileTransferStatus {
    Scanning,
    Running,
    Cancelling,
    Done,
    Failed,
    Cancelled,
}

pub(crate) struct OutboundCommand {
    pub(crate) client_id: String,
    pub(crate) payload: String,
}

pub(crate) enum FileTransferRequest {
    Upload {
        transfer_id: u64,
        local_path: String,
        remote_path: String,
    },
    Download {
        transfer_id: u64,
        remote_path: String,
        local_dir: String,
    },
    Cancel {
        transfer_id: u64,
        direction: FileTransferDirection,
        remote_path: String,
    },
}

pub(crate) fn open_window(
    windows: &mut Vec<FileManagerWindow>,
    client_id: &str,
    hostname: String,
    username: String,
) {
    if let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    {
        window.open = true;
        window.close_when_transfers_finish = false;
        window.hostname = hostname;
        window.username = username;
        window.close_requested.store(false, Ordering::Relaxed);
        queue_action(window, "list", "");
        return;
    }

    let local_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .display()
        .to_string();
    let mut window = FileManagerWindow {
        client_id: client_id.to_string(),
        hostname,
        username,
        current_path: Arc::new(Mutex::new(String::new())),
        path_input: Arc::new(Mutex::new(String::new())),
        entries: Arc::new(Mutex::new(Vec::new())),
        selected_name: Arc::new(Mutex::new(FileSelectionState::default())),
        local_entries: Arc::new(Mutex::new(read_local_entries(&local_dir))),
        selected_local_name: Arc::new(Mutex::new(FileSelectionState::default())),
        status: Arc::new(Mutex::new(FileStatus::Ready)),
        notice: Arc::new(Mutex::new("Ready".to_string())),
        local_path: Arc::new(Mutex::new(local_dir)),
        rename_to: Arc::new(Mutex::new(String::new())),
        new_folder_name: Arc::new(Mutex::new(String::new())),
        local_rename_to: Arc::new(Mutex::new(String::new())),
        local_new_folder_name: Arc::new(Mutex::new(String::new())),
        pending_delete: Arc::new(Mutex::new(None)),
        pending_rename: Arc::new(Mutex::new(None)),
        pending_new_folder: Arc::new(Mutex::new(false)),
        pending_local_delete: Arc::new(Mutex::new(None)),
        pending_local_rename: Arc::new(Mutex::new(None)),
        pending_local_new_folder: Arc::new(Mutex::new(false)),
        outbound: Arc::new(Mutex::new(Vec::new())),
        transfers: Arc::new(Mutex::new(Vec::new())),
        open: true,
        close_when_transfers_finish: false,
        close_requested: Arc::new(AtomicBool::new(false)),
    };
    queue_action(&mut window, "list", "");
    windows.push(window);
}

pub(crate) fn handle_ack(
    windows: &mut Vec<FileManagerWindow>,
    client_id: &str,
    hostname: String,
    username: String,
    accepted: bool,
    detail: String,
) {
    open_if_missing(windows, client_id, hostname, username);
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };

    let response = FileResponse::parse(&detail);
    if accepted && response.kind == "download" {
        let result = save_download(window, &response);
        set_status(
            window,
            if result.is_ok() {
                FileStatus::Done
            } else {
                FileStatus::Failed
            },
            &result.unwrap_or_else(|error| error),
        );
        refresh_local_entries(window);
        queue_action(window, "list", &response.cwd);
        return;
    }

    if !accepted || response.kind == "error" {
        set_status(
            window,
            FileStatus::Failed,
            response
                .message
                .as_deref()
                .unwrap_or("file manager command failed"),
        );
        if !response.cwd.is_empty() {
            set_path(window, &response.cwd);
        }
        return;
    }

    if !response.cwd.is_empty() {
        set_path(window, &response.cwd);
    }
    if let Ok(mut entries) = window.entries.lock() {
        *entries = response.entries;
    }
    clear_selection(&window.selected_name);
    set_status(window, FileStatus::Done, "Directory loaded");
}

pub(crate) fn handle_transfer(
    windows: &mut [FileManagerWindow],
    client_id: &str,
    _hostname: String,
    _username: String,
    message: Message,
) {
    let Some(window) = windows
        .iter_mut()
        .find(|window| window.client_id == client_id)
    else {
        return;
    };

    let Message::FileTransfer {
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
        message,
        ..
    } = message
    else {
        return;
    };
    if !transfer_row_exists(&window.transfers, transfer_id) {
        return;
    }

    match (direction, action) {
        (FileTransferDirection::Download, FileTransferAction::Directory) => {
            if let Err(error) = create_download_directory(window, transfer_id, &relative_path) {
                update_transfer_status(
                    &window.transfers,
                    transfer_id,
                    FileTransferStatus::Failed,
                    Some(total_bytes),
                    Some(transferred_bytes),
                    &error,
                );
                set_status(window, FileStatus::Failed, &error);
            }
        }
        (FileTransferDirection::Download, FileTransferAction::Chunk) => {
            if let Err(error) = write_download_chunk(
                window,
                transfer_id,
                &relative_path,
                file_size,
                offset,
                &bytes,
            ) {
                update_transfer_status(
                    &window.transfers,
                    transfer_id,
                    FileTransferStatus::Failed,
                    Some(total_bytes),
                    Some(transferred_bytes),
                    &error,
                );
                set_status(window, FileStatus::Failed, &error);
                return;
            }
            let completed = total_bytes > 0 && transferred_bytes >= total_bytes;
            let status = if completed {
                FileTransferStatus::Done
            } else {
                FileTransferStatus::Running
            };
            let status_message = if completed && message.trim().is_empty() {
                "download complete"
            } else {
                &message
            };
            update_transfer_status(
                &window.transfers,
                transfer_id,
                status,
                Some(total_bytes),
                Some(transferred_bytes),
                status_message,
            );
            if completed {
                refresh_local_entries_if_download_target_visible(window, transfer_id);
                if window.open {
                    set_status(window, FileStatus::Done, "Download complete");
                }
            }
        }
        (_, FileTransferAction::Progress) => {
            let message_lower = message.to_ascii_lowercase();
            let status = if message_lower.contains("cancel") {
                FileTransferStatus::Cancelling
            } else if message_lower.contains("scanning") {
                FileTransferStatus::Scanning
            } else {
                FileTransferStatus::Running
            };
            update_transfer_status(
                &window.transfers,
                transfer_id,
                status,
                Some(total_bytes),
                Some(transferred_bytes),
                &message,
            );
        }
        (_, FileTransferAction::Complete) => {
            let cancelled = message.to_ascii_lowercase().contains("cancel");
            let status = if cancelled {
                FileTransferStatus::Cancelled
            } else {
                FileTransferStatus::Done
            };
            update_transfer_status(
                &window.transfers,
                transfer_id,
                status,
                Some(total_bytes),
                Some(transferred_bytes),
                &message,
            );
            if direction == FileTransferDirection::Download {
                if cancelled {
                    if window.open {
                        set_status(window, FileStatus::Done, "Transfer cancelled");
                    }
                } else {
                    refresh_local_entries_if_download_target_visible(window, transfer_id);
                    if window.open {
                        set_status(window, FileStatus::Done, "Download complete");
                    }
                }
            } else if window.open {
                if cancelled {
                    set_status(window, FileStatus::Done, "Transfer cancelled");
                } else {
                    let current = window
                        .current_path
                        .lock()
                        .map(|value| value.clone())
                        .unwrap_or_default();
                    queue_action(window, "list", &current);
                    set_status(window, FileStatus::Done, "Upload complete");
                }
            }
        }
        (_, FileTransferAction::Error) => {
            let detail = if message.trim().is_empty() {
                format!("file transfer failed: {path}")
            } else {
                message
            };
            update_transfer_status(
                &window.transfers,
                transfer_id,
                FileTransferStatus::Failed,
                Some(total_bytes),
                Some(transferred_bytes),
                &detail,
            );
            if window.open {
                set_status(window, FileStatus::Failed, &detail);
            }
        }
        _ => {}
    }
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<FileManagerWindow>,
) -> Vec<OutboundCommand> {
    let mut outbound = Vec::new();
    for window in windows.iter_mut() {
        if window.close_requested.swap(false, Ordering::Relaxed) {
            if has_active_transfers(&window.transfers) {
                queue_cancel_active_transfers(&window.outbound, &window.transfers);
                window.open = false;
                window.close_when_transfers_finish = true;
                set_status(
                    window,
                    FileStatus::Pending,
                    t("Closing: stopping active file transfers"),
                );
            } else {
                window.open = false;
                window.close_when_transfers_finish = false;
            }
        }

        let client_id = window.client_id.clone();
        if window.open {
            let title = format!(
                "{} - {}",
                t("File Manager"),
                identity_title(&window.hostname, &window.username)
            );
            let viewport_id = egui::ViewportId::from_hash_of(("admin_file_manager", &client_id));
            let builder = windowing::child_viewport_builder(title, [1080.0, 620.0], [820.0, 460.0]);

            let current_path = window.current_path.clone();
            let path_input = window.path_input.clone();
            let entries = window.entries.clone();
            let selected_name = window.selected_name.clone();
            let local_entries = window.local_entries.clone();
            let selected_local_name = window.selected_local_name.clone();
            let status = window.status.clone();
            let notice = window.notice.clone();
            let local_path = window.local_path.clone();
            let rename_to = window.rename_to.clone();
            let new_folder_name = window.new_folder_name.clone();
            let local_rename_to = window.local_rename_to.clone();
            let local_new_folder_name = window.local_new_folder_name.clone();
            let pending_delete = window.pending_delete.clone();
            let pending_rename = window.pending_rename.clone();
            let pending_new_folder = window.pending_new_folder.clone();
            let pending_local_delete = window.pending_local_delete.clone();
            let pending_local_rename = window.pending_local_rename.clone();
            let pending_local_new_folder = window.pending_local_new_folder.clone();
            let outbound_queue = window.outbound.clone();
            let transfers = window.transfers.clone();
            let close_requested = window.close_requested.clone();
            let entries_id = client_id.clone();

            ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
                if ui.ctx().input(|input| input.viewport().close_requested()) {
                    close_requested.store(true, Ordering::Relaxed);
                }
                egui::CentralPanel::default()
                    .frame(crate::theme::page_frame())
                    .show_inside(ui, |ui| {
                        windowing::render_child_window_controls(ui);
                        let content_height =
                            (ui.available_height() - TRANSFER_TABLE_HEIGHT - 64.0).max(260.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), content_height),
                            egui::Layout::left_to_right(egui::Align::Min),
                            |ui| {
                                StripBuilder::new(ui)
                                    .size(Size::remainder())
                                    .size(Size::exact(TRANSFER_COLUMN_WIDTH))
                                    .size(Size::remainder())
                                    .horizontal(|mut strip| {
                                        strip.cell(|ui| {
                                            render_remote_panel(
                                                ui,
                                                &entries_id,
                                                &current_path,
                                                &path_input,
                                                &entries,
                                                &selected_name,
                                                &rename_to,
                                                &new_folder_name,
                                                &pending_delete,
                                                &pending_rename,
                                                &pending_new_folder,
                                                &outbound_queue,
                                                &status,
                                                &local_path,
                                                &notice,
                                                &transfers,
                                            );
                                        });
                                        strip.cell(|ui| {
                                            render_transfer_buttons(
                                                ui,
                                                &current_path,
                                                &entries,
                                                &selected_name,
                                                &local_path,
                                                &local_entries,
                                                &selected_local_name,
                                                &outbound_queue,
                                                &status,
                                                &notice,
                                                &transfers,
                                            );
                                        });
                                        strip.cell(|ui| {
                                            render_local_panel(
                                                ui,
                                                &entries_id,
                                                &local_path,
                                                &local_entries,
                                                &selected_local_name,
                                                &local_rename_to,
                                                &local_new_folder_name,
                                                &pending_local_delete,
                                                &pending_local_rename,
                                                &pending_local_new_folder,
                                                &current_path,
                                                &outbound_queue,
                                                &status,
                                                &notice,
                                                &transfers,
                                            );
                                        });
                                    });
                            },
                        );
                        ui.add_space(8.0);
                        render_transfer_table(ui, &transfers, &outbound_queue);
                        ui.add_space(8.0);
                        render_status_bar(ui, &status, &notice);
                        render_pending_dialogs(
                            ui,
                            &pending_delete,
                            &pending_rename,
                            &pending_new_folder,
                            &rename_to,
                            &current_path,
                            &new_folder_name,
                            &local_path,
                            &local_entries,
                            &selected_local_name,
                            &pending_local_delete,
                            &pending_local_rename,
                            &pending_local_new_folder,
                            &local_rename_to,
                            &local_new_folder_name,
                            &status,
                            &notice,
                            &outbound_queue,
                        );
                    });
            });
        }

        while let Some(payload) = window
            .outbound
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop())
        {
            if parse_transfer_request(&payload).is_none() {
                set_status(window, FileStatus::Pending, t("Waiting for client result"));
            } else {
                set_status(window, FileStatus::Done, t("File transfer queued"));
            }
            outbound.push(OutboundCommand {
                client_id: client_id.clone(),
                payload,
            });
        }
    }
    windows.retain(|window| {
        window.open
            || (window.close_when_transfers_finish && has_pending_outbound(&window.outbound))
    });
    outbound
}

fn open_if_missing(
    windows: &mut Vec<FileManagerWindow>,
    client_id: &str,
    hostname: String,
    username: String,
) {
    if windows.iter().any(|window| window.client_id == client_id) {
        return;
    }
    open_window(windows, client_id, hostname, username);
}

#[allow(clippy::too_many_arguments)]
fn render_remote_panel(
    ui: &mut egui::Ui,
    entries_id: &str,
    current_path: &Arc<Mutex<String>>,
    path_input: &Arc<Mutex<String>>,
    entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_name: &FileSelection,
    rename_to: &Arc<Mutex<String>>,
    new_folder_name: &Arc<Mutex<String>>,
    pending_delete: &Arc<Mutex<Option<String>>>,
    pending_rename: &Arc<Mutex<Option<String>>>,
    pending_new_folder: &Arc<Mutex<bool>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    local_path: &Arc<Mutex<String>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    crate::theme::panel_frame_with_margin(8.0).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(t("Remote"))
                    .size(13.0)
                    .color(crate::theme::palette().text)
                    .strong(),
            );
            render_remote_toolbar(ui, current_path, path_input, outbound, status);
            ui.add_space(6.0);
            render_entries_table(
                ui,
                entries_id,
                current_path,
                entries,
                selected_name,
                rename_to,
                pending_delete,
                pending_rename,
                pending_new_folder,
                new_folder_name,
                outbound,
                status,
                local_path,
                notice,
                transfers,
            );
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn render_local_panel(
    ui: &mut egui::Ui,
    entries_id: &str,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
    local_rename_to: &Arc<Mutex<String>>,
    local_new_folder_name: &Arc<Mutex<String>>,
    pending_local_delete: &Arc<Mutex<Option<String>>>,
    pending_local_rename: &Arc<Mutex<Option<String>>>,
    pending_local_new_folder: &Arc<Mutex<bool>>,
    current_path: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    crate::theme::panel_frame_with_margin(8.0).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(t("Local"))
                    .size(13.0)
                    .color(crate::theme::palette().text)
                    .strong(),
            );
            render_local_toolbar(ui, local_path, local_entries, selected_local_name);
            ui.add_space(6.0);
            render_local_entries_table(
                ui,
                entries_id,
                local_path,
                local_entries,
                selected_local_name,
                local_rename_to,
                local_new_folder_name,
                pending_local_delete,
                pending_local_rename,
                pending_local_new_folder,
                current_path,
                outbound,
                status,
                notice,
                transfers,
            );
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn render_transfer_buttons(
    ui: &mut egui::Ui,
    current_path: &Arc<Mutex<String>>,
    remote_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_remote: &FileSelection,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local: &FileSelection,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    ui.vertical_centered(|ui| {
        ui.set_min_size(ui.available_size());
        ui.add_space(150.0);
        if ui
            .add_enabled(
                !is_pending(status),
                egui::Button::new("Down ->")
                    .min_size(egui::vec2(TRANSFER_BUTTON_WIDTH, TOOLBAR_CONTROL_HEIGHT)),
            )
            .on_hover_text(t("Download selected remote file or folder"))
            .clicked()
        {
            let entries = selected_remote_entries(remote_entries, selected_remote);
            if entries.is_empty() {
                set_status_arc(
                    status,
                    notice,
                    FileStatus::Failed,
                    t("Select a remote file or folder"),
                );
            } else {
                let count = entries.len();
                for entry in &entries {
                    queue_download_remote(current_path, local_path, outbound, transfers, entry);
                }
                set_status_arc(
                    status,
                    notice,
                    FileStatus::Done,
                    &tf(
                        if count == 1 {
                            "{count} download queued"
                        } else {
                            "{count} downloads queued"
                        },
                        &[("count", &count.to_string())],
                    ),
                );
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        }
        ui.add_space(8.0);
        if ui
            .add_enabled(
                !is_pending(status),
                egui::Button::new("<- Up")
                    .min_size(egui::vec2(TRANSFER_BUTTON_WIDTH, TOOLBAR_CONTROL_HEIGHT)),
            )
            .on_hover_text(t("Upload selected local file or folder"))
            .clicked()
        {
            upload_selected_local(
                current_path,
                local_path,
                local_entries,
                selected_local,
                outbound,
                status,
                notice,
                transfers,
            );
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    });
}

fn render_remote_toolbar(
    ui: &mut egui::Ui,
    current_path: &Arc<Mutex<String>>,
    path_input: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        let busy = is_pending(status);
        if ui.add_enabled(!busy, egui::Button::new(t("Up"))).clicked() {
            let path = current_path
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            queue_payload(outbound, &request("list", &parent_path(&path), ""));
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
        if ui
            .add_enabled(!busy, egui::Button::new(t("Refresh")))
            .clicked()
        {
            let path = current_path
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            queue_payload(outbound, &request("list", &path, ""));
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
        ui.add_enabled_ui(!busy, |ui| {
            ui.menu_button(t("Jump"), |ui| {
                for (label, path) in QUICK_JUMPS {
                    if ui.button(label).clicked() {
                        queue_payload(outbound, &request("list", path, ""));
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        ui.close();
                    }
                }
            });
        });
        let mut path = path_input
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let response = ui.add_sized(
            [
                (ui.available_width() - 230.0).max(90.0),
                TOOLBAR_CONTROL_HEIGHT,
            ],
            egui::TextEdit::singleline(&mut path)
                .hint_text(t("Remote path"))
                .vertical_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(mut value) = path_input.lock() {
                *value = path.clone();
            }
        }
        let go = ui.add_enabled(!busy, egui::Button::new(t("Go"))).clicked()
            || (!busy
                && response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if go {
            queue_payload(outbound, &request("list", &path, ""));
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    });
}

fn render_local_toolbar(
    ui: &mut egui::Ui,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
        if ui.button(t("Up")).clicked() {
            let path = local_path
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            set_local_dir(
                local_path,
                local_entries,
                selected_local_name,
                &parent_path(&path),
            );
        }
        if ui.button(t("Refresh")).clicked() {
            refresh_local_entries_arc(local_path, local_entries, selected_local_name);
        }
        if ui.button(t("Choose...")).clicked() {
            let current = local_path
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            if let Some(path) = pick_local_folder(&current, t("Choose local folder")) {
                set_local_dir(
                    local_path,
                    local_entries,
                    selected_local_name,
                    &path.to_string_lossy(),
                );
            }
        }
        ui.menu_button(t("Jump"), |ui| {
            for (label, path) in local_quick_jump_paths() {
                let enabled = path.is_dir();
                let path_text = path.display().to_string();
                if ui
                    .add_enabled(enabled, egui::Button::new(label))
                    .on_hover_text(&path_text)
                    .clicked()
                {
                    set_local_dir(local_path, local_entries, selected_local_name, &path_text);
                    ui.close();
                }
            }
        });
        let mut path = local_path
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let response = ui.add_sized(
            [
                (ui.available_width() - 42.0).max(90.0),
                TOOLBAR_CONTROL_HEIGHT,
            ],
            egui::TextEdit::singleline(&mut path)
                .hint_text(t("Local path"))
                .vertical_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(mut value) = local_path.lock() {
                *value = path.clone();
            }
        }
        let go = ui.button(t("Go")).clicked()
            || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
        if go {
            set_local_dir(local_path, local_entries, selected_local_name, &path);
        }
    });
}

fn render_entries_table(
    ui: &mut egui::Ui,
    entries_id: &str,
    current_path: &Arc<Mutex<String>>,
    entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_name: &FileSelection,
    rename_to: &Arc<Mutex<String>>,
    pending_delete: &Arc<Mutex<Option<String>>>,
    pending_rename: &Arc<Mutex<Option<String>>>,
    pending_new_folder: &Arc<Mutex<bool>>,
    new_folder_name: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    local_path: &Arc<Mutex<String>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    let entries = entries
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let selected = selected_names(selected_name);
    let ctx = ui.ctx().clone();
    file_table(
        ui,
        ("remote_file_table_resizable", entries_id),
        &entries,
        &selected,
        |row_response, entry, checked| {
            if let Some(checked) = checked {
                set_entry_checked(selected_name, entry, checked);
            } else if row_response.clicked() || row_response.secondary_clicked() {
                select_entry(selected_name, entry);
            }
            if row_response.double_clicked() && entry.kind == "dir" && !is_pending(status) {
                let path = join_remote(current_path, &entry.name);
                queue_payload(outbound, &request("list", &path, ""));
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            row_response.context_menu(|ui| {
                if ui.button(t("Open")).clicked() && entry.kind == "dir" {
                    let path = join_remote(current_path, &entry.name);
                    queue_payload(outbound, &request("list", &path, ""));
                    ui.close();
                }
                if ui.button(t("Copy Full Path")).clicked() {
                    ui.ctx().copy_text(join_remote(current_path, &entry.name));
                    ui.close();
                }
                if ui.button(t("Download...")).clicked() {
                    choose_download_remote(
                        current_path,
                        local_path,
                        outbound,
                        status,
                        notice,
                        transfers,
                        entry,
                    );
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    ui.close();
                }
                if ui.button(t("Upload File...")).clicked() {
                    choose_upload_local(
                        LocalPickMode::File,
                        current_path,
                        local_path,
                        outbound,
                        status,
                        notice,
                        transfers,
                    );
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    ui.close();
                }
                if ui.button(t("Upload Folder...")).clicked() {
                    choose_upload_local(
                        LocalPickMode::Folder,
                        current_path,
                        local_path,
                        outbound,
                        status,
                        notice,
                        transfers,
                    );
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    ui.close();
                }
                if ui.button(t("Delete")).clicked() {
                    let path = join_remote(current_path, &entry.name);
                    begin_delete(pending_delete, &path);
                    ui.close();
                }
                ui.separator();
                if ui.button(t("New Folder")).clicked() {
                    begin_new_folder(pending_new_folder, new_folder_name);
                    ui.close();
                }
                if ui.button(t("Rename")).clicked() {
                    let path = join_remote(current_path, &entry.name);
                    begin_rename(pending_rename, rename_to, &path, &entry.name);
                    ui.close();
                }
            });
        },
    );
    render_remote_blank_context_menu(
        ui,
        selected_name,
        current_path,
        local_path,
        pending_new_folder,
        new_folder_name,
        outbound,
        status,
        notice,
        transfers,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_local_entries_table(
    ui: &mut egui::Ui,
    entries_id: &str,
    local_path: &Arc<Mutex<String>>,
    entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_name: &FileSelection,
    local_rename_to: &Arc<Mutex<String>>,
    local_new_folder_name: &Arc<Mutex<String>>,
    pending_local_delete: &Arc<Mutex<Option<String>>>,
    pending_local_rename: &Arc<Mutex<Option<String>>>,
    pending_local_new_folder: &Arc<Mutex<bool>>,
    current_path: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    let entries_snapshot = entries
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let selected = selected_names(selected_name);
    file_table(
        ui,
        ("local_file_table_resizable", entries_id),
        &entries_snapshot,
        &selected,
        |row_response, entry, checked| {
            if let Some(checked) = checked {
                set_entry_checked(selected_name, entry, checked);
            } else if row_response.clicked() || row_response.secondary_clicked() {
                select_entry(selected_name, entry);
            }
            if row_response.double_clicked() && entry.kind == "dir" {
                let path = join_local(local_path, &entry.name);
                set_local_dir(local_path, entries, selected_name, &path);
            }
            row_response.context_menu(|ui| {
                if ui.button(t("Open")).clicked() && entry.kind == "dir" {
                    let path = join_local(local_path, &entry.name);
                    set_local_dir(local_path, entries, selected_name, &path);
                    ui.close();
                }
                if ui.button(t("Copy Full Path")).clicked() {
                    ui.ctx().copy_text(join_local(local_path, &entry.name));
                    ui.close();
                }
                if ui.button(t("Upload")).clicked() {
                    select_entry(selected_name, entry);
                    upload_selected_local(
                        current_path,
                        local_path,
                        entries,
                        selected_name,
                        outbound,
                        status,
                        notice,
                        transfers,
                    );
                    ui.close();
                }
                ui.separator();
                if ui.button(t("New Folder")).clicked() {
                    begin_local_new_folder(pending_local_new_folder, local_new_folder_name);
                    ui.close();
                }
                if ui.button(t("Delete")).clicked() {
                    begin_local_delete(pending_local_delete, local_path, &entry.name);
                    ui.close();
                }
                if ui.button(t("Rename")).clicked() {
                    begin_local_rename(
                        pending_local_rename,
                        local_rename_to,
                        local_path,
                        &entry.name,
                    );
                    ui.close();
                }
            });
        },
    );
    render_local_blank_context_menu(
        ui,
        selected_name,
        local_path,
        pending_local_new_folder,
        local_new_folder_name,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_remote_blank_context_menu(
    ui: &mut egui::Ui,
    selected_name: &FileSelection,
    current_path: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    pending_new_folder: &Arc<Mutex<bool>>,
    new_folder_name: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    let Some(blank_response) = file_table_blank_response(ui) else {
        return;
    };
    if blank_response.clicked() || blank_response.secondary_clicked() {
        clear_selection(selected_name);
    }
    let ctx = blank_response.ctx.clone();
    blank_response.context_menu(|ui| {
        if ui.button(t("New Folder")).clicked() {
            begin_new_folder(pending_new_folder, new_folder_name);
            ui.close();
        }
        if ui.button(t("Upload File...")).clicked() {
            choose_upload_local(
                LocalPickMode::File,
                current_path,
                local_path,
                outbound,
                status,
                notice,
                transfers,
            );
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            ui.close();
        }
        if ui.button(t("Upload Folder...")).clicked() {
            choose_upload_local(
                LocalPickMode::Folder,
                current_path,
                local_path,
                outbound,
                status,
                notice,
                transfers,
            );
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            ui.close();
        }
        ui.separator();
        if ui.button(t("Copy Current Folder Path")).clicked() {
            ui.ctx().copy_text(current_directory_path(current_path));
            ui.close();
        }
    });
}

fn render_local_blank_context_menu(
    ui: &mut egui::Ui,
    selected_name: &FileSelection,
    local_path: &Arc<Mutex<String>>,
    pending_local_new_folder: &Arc<Mutex<bool>>,
    local_new_folder_name: &Arc<Mutex<String>>,
) {
    let Some(blank_response) = file_table_blank_response(ui) else {
        return;
    };
    if blank_response.clicked() || blank_response.secondary_clicked() {
        clear_selection(selected_name);
    }
    blank_response.context_menu(|ui| {
        if ui.button(t("New Folder")).clicked() {
            begin_local_new_folder(pending_local_new_folder, local_new_folder_name);
            ui.close();
        }
        ui.separator();
        if ui.button(t("Copy Current Folder Path")).clicked() {
            ui.ctx().copy_text(current_directory_path(local_path));
            ui.close();
        }
    });
}

fn file_table_blank_response(ui: &mut egui::Ui) -> Option<egui::Response> {
    let size = ui.available_size();
    if size.x <= 1.0 || size.y <= 1.0 {
        return None;
    }
    Some(ui.allocate_response(size, egui::Sense::click()))
}

fn file_table<R>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    entries: &[FileEntry],
    selected: &[String],
    mut row_handler: impl FnMut(&egui::Response, &FileEntry, Option<bool>) -> R,
) {
    let available_width = ui.available_width().max(360.0);
    let select_width = 28.0;
    let type_width = 44.0;
    let size_width = 88.0;
    let modified_width = 132.0;
    let name_width =
        (available_width - select_width - type_width - size_width - modified_width - 24.0)
            .max(140.0);
    let table = crate::theme::clickable_table(ui, id, true)
        .column(Column::exact(select_width))
        .column(Column::initial(type_width).at_least(38.0).clip(true))
        .column(Column::initial(name_width).at_least(140.0).clip(true))
        .column(Column::initial(size_width).at_least(72.0).clip(true))
        .column(Column::initial(modified_width).at_least(108.0).clip(true));
    table
        .header(24.0, |mut header| {
            header.col(|ui| table_header_label(ui, ""));
            header.col(|ui| table_header_label(ui, t("Type")));
            header.col(|ui| table_header_label(ui, t("Name")));
            header.col(|ui| table_header_label(ui, t("Size")));
            header.col(|ui| table_header_label(ui, t("Modified")));
        })
        .body(|mut body| {
            for entry in entries {
                let is_selected = selected.iter().any(|name| name == &entry.name);
                body.row(24.0, |mut row| {
                    row.set_selected(is_selected);
                    let mut checked_change = None;
                    row.col(|ui| {
                        let mut checked = is_selected;
                        if ui.checkbox(&mut checked, "").changed() {
                            checked_change = Some(checked);
                        }
                    });
                    row.col(|ui| table_text(ui, &entry.kind));
                    row.col(|ui| table_text(ui, &entry.name));
                    row.col(|ui| table_text(ui, &format_file_size(&entry.size)));
                    row.col(|ui| table_text(ui, &format_modified_time(&entry.modified)));
                    let row_response = row.response();
                    if row_response.hovered() {
                        row_response
                            .ctx
                            .set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    row_handler(&row_response, entry, checked_change);
                });
            }
        });
}

fn render_transfer_table(
    ui: &mut egui::Ui,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
    outbound: &Arc<Mutex<Vec<String>>>,
) {
    let rows = transfers
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    crate::theme::panel_frame_with_margin(8.0).show(ui, |ui| {
        ui.set_min_height(TRANSFER_TABLE_HEIGHT - 18.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t("Transfers"))
                    .size(13.0)
                    .color(crate::theme::palette().text)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(t("Right click a row to delete"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
            });
        });
        ui.add_space(6.0);
        if rows.is_empty() {
            ui.label(
                egui::RichText::new(t("No file transfers"))
                    .size(12.0)
                    .color(crate::theme::palette().muted),
            );
            return;
        }

        let available_width = ui.available_width().max(720.0);
        let id_width = 64.0;
        let direction_width = 86.0;
        let progress_width = 150.0;
        let status_width = 112.0;
        let message_width = 180.0;
        let name_width = (available_width
            - id_width
            - direction_width
            - progress_width
            - status_width
            - message_width
            - 30.0)
            .max(180.0);

        crate::theme::clickable_table(ui, "file_transfer_table_resizable", true)
            .column(Column::initial(id_width).at_least(48.0).clip(true))
            .column(Column::initial(direction_width).at_least(76.0).clip(true))
            .column(Column::initial(name_width).at_least(160.0).clip(true))
            .column(Column::initial(progress_width).at_least(110.0).clip(true))
            .column(Column::initial(status_width).at_least(92.0).clip(true))
            .column(Column::initial(message_width).at_least(120.0).clip(true))
            .header(24.0, |mut header| {
                header.col(|ui| table_header_label(ui, t("ID")));
                header.col(|ui| table_header_label(ui, t("Direction")));
                header.col(|ui| table_header_label(ui, t("Item")));
                header.col(|ui| table_header_label(ui, t("Progress")));
                header.col(|ui| table_header_label(ui, t("Status")));
                header.col(|ui| table_header_label(ui, t("Message")));
            })
            .body(|mut body| {
                for row_data in rows {
                    body.row(24.0, |mut row| {
                        row.col(|ui| table_text(ui, &row_data.transfer_id.to_string()));
                        row.col(|ui| table_text(ui, transfer_direction_label(row_data.direction)));
                        row.col(|ui| {
                            let response = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&row_data.name)
                                        .size(12.0)
                                        .color(crate::theme::palette().text),
                                )
                                .selectable(false)
                                .sense(egui::Sense::hover()),
                            );
                            response.on_hover_text(format!(
                                "{}\n{}",
                                row_data.source, row_data.destination
                            ));
                        });
                        row.col(|ui| table_text(ui, &transfer_progress_label(&row_data)));
                        row.col(|ui| {
                            let color = transfer_status_color(row_data.status);
                            ui.label(
                                egui::RichText::new(transfer_status_label(row_data.status))
                                    .size(12.0)
                                    .color(color)
                                    .strong(),
                            );
                        });
                        row.col(|ui| table_text(ui, &row_data.message));
                        let response = row.response();
                        if response.hovered() {
                            response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        response.context_menu(|ui| {
                            if ui.button(t("Delete")).clicked() {
                                if transfer_can_stop(row_data.status) {
                                    queue_cancel_transfer(outbound, transfers, &row_data);
                                }
                                if let Ok(mut rows) = transfers.lock() {
                                    rows.retain(|row| row.transfer_id != row_data.transfer_id);
                                }
                                ui.close();
                            }
                        });
                    });
                }
            });
    });
}

fn queue_cancel_transfer(
    outbound: &Arc<Mutex<Vec<String>>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
    row: &FileTransferRow,
) {
    queue_payload(
        outbound,
        &transfer_request_payload(
            "cancel",
            row.transfer_id,
            row.direction,
            &row.remote_path,
            None,
        ),
    );
    update_transfer_status(
        transfers,
        row.transfer_id,
        FileTransferStatus::Cancelling,
        None,
        None,
        "cancel requested",
    );
}

fn queue_cancel_active_transfers(
    outbound: &Arc<Mutex<Vec<String>>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    let rows = transfers
        .lock()
        .map(|rows| rows.clone())
        .unwrap_or_default();
    for row in rows.iter().filter(|row| transfer_can_stop(row.status)) {
        queue_cancel_transfer(outbound, transfers, row);
    }
}

fn has_active_transfers(transfers: &Arc<Mutex<Vec<FileTransferRow>>>) -> bool {
    transfers
        .lock()
        .map(|rows| rows.iter().any(|row| transfer_can_stop(row.status)))
        .unwrap_or(false)
}

fn has_pending_outbound(outbound: &Arc<Mutex<Vec<String>>>) -> bool {
    outbound
        .lock()
        .map(|queue| !queue.is_empty())
        .unwrap_or(false)
}

fn transfer_row_exists(transfers: &Arc<Mutex<Vec<FileTransferRow>>>, transfer_id: u64) -> bool {
    transfers
        .lock()
        .map(|rows| rows.iter().any(|row| row.transfer_id == transfer_id))
        .unwrap_or(false)
}

fn table_header_label(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.0)
                .color(crate::theme::palette().text)
                .strong(),
        )
        .selectable(false)
        .sense(egui::Sense::hover()),
    );
}

fn table_text(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.0)
                .color(crate::theme::palette().text),
        )
        .selectable(false)
        .sense(egui::Sense::hover()),
    );
}

fn format_file_size(size: &str) -> String {
    let size = size.trim();
    if size.is_empty() {
        return String::new();
    }
    let Ok(bytes) = size.parse::<u64>() else {
        return size.to_string();
    };
    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit_index = 0usize;
    while value >= 1024.0 && unit_index + 1 < units.len() {
        value /= 1024.0;
        unit_index += 1;
    }

    if value >= 100.0 || (value.fract()).abs() < 0.05 {
        format!("{value:.0} {}", units[unit_index])
    } else {
        format!("{value:.1} {}", units[unit_index])
    }
}

fn transfer_direction_label(direction: FileTransferDirection) -> &'static str {
    match direction {
        FileTransferDirection::Upload => t("Upload"),
        FileTransferDirection::Download => t("Download"),
    }
}

fn transfer_status_label(status: FileTransferStatus) -> &'static str {
    match status {
        FileTransferStatus::Scanning => t("Scanning"),
        FileTransferStatus::Running => t("Running"),
        FileTransferStatus::Cancelling => t("Cancelling"),
        FileTransferStatus::Done => t("Done"),
        FileTransferStatus::Failed => t("Failed"),
        FileTransferStatus::Cancelled => t("Cancelled"),
    }
}

fn transfer_status_color(status: FileTransferStatus) -> egui::Color32 {
    match status {
        FileTransferStatus::Scanning => COLOR_WARN,
        FileTransferStatus::Running => COLOR_WARN,
        FileTransferStatus::Cancelling => COLOR_WARN,
        FileTransferStatus::Done => COLOR_GOOD,
        FileTransferStatus::Failed => COLOR_BAD,
        FileTransferStatus::Cancelled => crate::theme::palette().muted,
    }
}

fn transfer_can_stop(status: FileTransferStatus) -> bool {
    matches!(
        status,
        FileTransferStatus::Scanning | FileTransferStatus::Running
    )
}

fn transfer_progress_label(row: &FileTransferRow) -> String {
    if row.status == FileTransferStatus::Scanning && row.total_bytes == 0 {
        return t("Scanning").to_string();
    }
    if row.total_bytes > 0 {
        let percent =
            ((row.transferred_bytes as f64 / row.total_bytes as f64) * 100.0).clamp(0.0, 100.0);
        format!(
            "{percent:.1}% ({}/{})",
            format_file_size(&row.transferred_bytes.to_string()),
            format_file_size(&row.total_bytes.to_string())
        )
    } else if row.transferred_bytes > 0 {
        format_file_size(&row.transferred_bytes.to_string())
    } else {
        "-".to_string()
    }
}

fn format_modified_time(modified: &str) -> String {
    let modified = modified.trim();
    if modified.is_empty() {
        return String::new();
    }
    let Ok(seconds) = modified.parse::<i64>() else {
        return modified.to_string();
    };
    let Some(datetime) = Local.timestamp_opt(seconds, 0).single() else {
        return modified.to_string();
    };
    datetime.format("%Y-%m-%d %H:%M").to_string()
}

fn render_status_bar(
    ui: &mut egui::Ui,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let status = status
        .lock()
        .map(|value| *value)
        .unwrap_or(FileStatus::Ready);
    let notice = notice.lock().map(|value| value.clone()).unwrap_or_default();
    let (label, color) = match status {
        FileStatus::Ready => (t("Ready"), crate::theme::palette().muted),
        FileStatus::Pending => (t("Pending"), COLOR_WARN),
        FileStatus::Done => (t("Done"), COLOR_GOOD),
        FileStatus::Failed => (t("Failed"), COLOR_BAD),
    };
    crate::theme::status_frame().show(ui, |ui| {
        ui.set_min_height(26.0);
        crate::theme::render_status_line(ui, label, color, &notice, |_| {});
    });
}

fn render_pending_dialogs(
    ui: &mut egui::Ui,
    pending_delete: &Arc<Mutex<Option<String>>>,
    pending_rename: &Arc<Mutex<Option<String>>>,
    pending_new_folder: &Arc<Mutex<bool>>,
    rename_to: &Arc<Mutex<String>>,
    current_path: &Arc<Mutex<String>>,
    new_folder_name: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
    pending_local_delete: &Arc<Mutex<Option<String>>>,
    pending_local_rename: &Arc<Mutex<Option<String>>>,
    pending_local_new_folder: &Arc<Mutex<bool>>,
    local_rename_to: &Arc<Mutex<String>>,
    local_new_folder_name: &Arc<Mutex<String>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
) {
    let delete_path = pending_delete.lock().ok().and_then(|value| value.clone());
    if let Some(remote_path) = delete_path {
        egui::Window::new(t("Confirm Delete"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(t("Delete this remote item?"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
                ui.label(
                    egui::RichText::new(&remote_path)
                        .size(12.0)
                        .color(crate::theme::palette().text),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(t("Delete")).color(COLOR_BAD).strong(),
                        ))
                        .clicked()
                    {
                        queue_payload(outbound, &request("delete", &remote_path, ""));
                        if let Ok(mut value) = pending_delete.lock() {
                            *value = None;
                        }
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if ui.button(t("Cancel")).clicked() {
                        if let Ok(mut value) = pending_delete.lock() {
                            *value = None;
                        }
                    }
                });
            });
    }

    let rename_path = pending_rename.lock().ok().and_then(|value| value.clone());
    if let Some(remote_path) = rename_path {
        egui::Window::new(t("Rename Item"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(t("Rename remote item"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
                ui.label(
                    egui::RichText::new(&remote_path)
                        .size(12.0)
                        .color(crate::theme::palette().text),
                );
                ui.add_space(8.0);
                let mut name = rename_to
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name)
                        .hint_text(t("New name"))
                        .vertical_align(egui::Align::Center),
                );
                if response.changed() {
                    if let Ok(mut value) = rename_to.lock() {
                        *value = name.clone();
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(t("Rename")).clicked() {
                        queue_payload(outbound, &request("rename", &remote_path, &name));
                        if let Ok(mut value) = pending_rename.lock() {
                            *value = None;
                        }
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if ui.button(t("Cancel")).clicked() {
                        if let Ok(mut value) = pending_rename.lock() {
                            *value = None;
                        }
                    }
                });
            });
    }

    let show_new_folder = pending_new_folder
        .lock()
        .map(|value| *value)
        .unwrap_or(false);
    if show_new_folder {
        egui::Window::new(t("New Remote Folder"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(t("Create folder in current remote directory"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
                ui.add_space(8.0);
                let mut name = new_folder_name
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name)
                        .hint_text(t("Folder name"))
                        .vertical_align(egui::Align::Center),
                );
                if response.changed() {
                    if let Ok(mut value) = new_folder_name.lock() {
                        *value = name.clone();
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let create = ui.button(t("Create")).clicked()
                        || (response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                    if create {
                        create_folder(current_path, new_folder_name, outbound, status, notice);
                        if let Ok(mut value) = pending_new_folder.lock() {
                            *value = false;
                        }
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if ui.button(t("Cancel")).clicked() {
                        if let Ok(mut value) = pending_new_folder.lock() {
                            *value = false;
                        }
                    }
                });
            });
    }

    let local_delete_path = pending_local_delete
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if let Some(path) = local_delete_path {
        egui::Window::new(t("Confirm Local Delete"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(t("Delete this local item?"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
                ui.label(
                    egui::RichText::new(&path)
                        .size(12.0)
                        .color(crate::theme::palette().text),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(t("Delete")).color(COLOR_BAD).strong(),
                        ))
                        .clicked()
                    {
                        delete_local_path(
                            &path,
                            local_path,
                            local_entries,
                            selected_local_name,
                            status,
                            notice,
                        );
                        if let Ok(mut value) = pending_local_delete.lock() {
                            *value = None;
                        }
                    }
                    if ui.button(t("Cancel")).clicked() {
                        if let Ok(mut value) = pending_local_delete.lock() {
                            *value = None;
                        }
                    }
                });
            });
    }

    let local_rename_path = pending_local_rename
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if let Some(path) = local_rename_path {
        egui::Window::new(t("Rename Local Item"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(t("Rename local item"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
                ui.label(
                    egui::RichText::new(&path)
                        .size(12.0)
                        .color(crate::theme::palette().text),
                );
                ui.add_space(8.0);
                let mut name = local_rename_to
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name)
                        .hint_text(t("New name"))
                        .vertical_align(egui::Align::Center),
                );
                if response.changed() {
                    if let Ok(mut value) = local_rename_to.lock() {
                        *value = name.clone();
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(t("Rename")).clicked() {
                        rename_local_path(
                            &path,
                            &name,
                            local_path,
                            local_entries,
                            selected_local_name,
                            status,
                            notice,
                        );
                        if let Ok(mut value) = pending_local_rename.lock() {
                            *value = None;
                        }
                    }
                    if ui.button(t("Cancel")).clicked() {
                        if let Ok(mut value) = pending_local_rename.lock() {
                            *value = None;
                        }
                    }
                });
            });
    }

    let show_local_new_folder = pending_local_new_folder
        .lock()
        .map(|value| *value)
        .unwrap_or(false);
    if show_local_new_folder {
        egui::Window::new(t("New Local Folder"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(t("Create folder in current local directory"))
                        .size(12.0)
                        .color(crate::theme::palette().muted),
                );
                ui.add_space(8.0);
                let mut name = local_new_folder_name
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name)
                        .hint_text(t("Folder name"))
                        .vertical_align(egui::Align::Center),
                );
                if response.changed() {
                    if let Ok(mut value) = local_new_folder_name.lock() {
                        *value = name.clone();
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let create = ui.button(t("Create")).clicked()
                        || (response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                    if create {
                        create_local_folder(
                            local_path,
                            local_entries,
                            selected_local_name,
                            local_new_folder_name,
                            status,
                            notice,
                        );
                        if let Ok(mut value) = pending_local_new_folder.lock() {
                            *value = false;
                        }
                    }
                    if ui.button(t("Cancel")).clicked() {
                        if let Ok(mut value) = pending_local_new_folder.lock() {
                            *value = false;
                        }
                    }
                });
            });
    }
}

fn queue_action(window: &mut FileManagerWindow, action: &str, path: &str) {
    queue_payload(&window.outbound, &request(action, path, ""));
}

fn queue_payload(outbound: &Arc<Mutex<Vec<String>>>, payload: &str) {
    if let Ok(mut queue) = outbound.lock() {
        queue.insert(0, payload.to_string());
    }
}

fn pick_local_folder(current_dir: &str, title: &str) -> Option<PathBuf> {
    local_file_dialog(current_dir, title).pick_folder()
}

fn pick_local_file(current_dir: &str, title: &str) -> Option<PathBuf> {
    local_file_dialog(current_dir, title).pick_file()
}

fn local_file_dialog(current_dir: &str, title: &str) -> FileDialog {
    let mut dialog = FileDialog::new().set_title(title);
    let dir = PathBuf::from(current_dir.trim());
    if dir.is_dir() {
        dialog = dialog.set_directory(dir);
    }
    dialog
}

fn local_quick_jump_paths() -> Vec<(&'static str, PathBuf)> {
    let Some(home) = user_home_dir() else {
        return Vec::new();
    };
    vec![
        ("User", home.clone()),
        ("Desktop", home.join("Desktop")),
        ("Downloads", home.join("Downloads")),
        ("Documents", home.join("Documents")),
    ]
}

fn user_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                if drive.is_empty() || path.is_empty() {
                    return None;
                }
                let mut combined = drive;
                combined.push(path);
                Some(PathBuf::from(combined))
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
    }

    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
    }
}

#[derive(Clone, Copy)]
enum LocalPickMode {
    File,
    Folder,
}

fn choose_download_remote(
    current_path: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
    entry: &FileEntry,
) {
    let current = local_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let Some(local_dir) = pick_local_folder(&current, "Choose download folder") else {
        set_status_arc(status, notice, FileStatus::Ready, "Download cancelled");
        return;
    };
    let local_dir = local_dir.to_string_lossy().to_string();
    queue_download_remote_to_dir(current_path, &local_dir, outbound, transfers, entry);
    set_status_arc(status, notice, FileStatus::Done, "Download queued");
}

fn queue_download_remote(
    current_path: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
    entry: &FileEntry,
) {
    let local_dir = local_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    queue_download_remote_to_dir(current_path, &local_dir, outbound, transfers, entry);
}

fn queue_download_remote_to_dir(
    current_path: &Arc<Mutex<String>>,
    local_dir: &str,
    outbound: &Arc<Mutex<Vec<String>>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
    entry: &FileEntry,
) {
    let transfer_id = NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed);
    let remote_path = join_remote(current_path, &entry.name);
    let total_bytes = if entry.kind == "file" {
        entry.size.trim().parse::<u64>().unwrap_or(0)
    } else {
        0
    };
    let destination = PathBuf::from(local_dir.trim())
        .join(&entry.name)
        .display()
        .to_string();
    add_transfer_row(
        transfers,
        FileTransferRow {
            transfer_id,
            direction: FileTransferDirection::Download,
            name: entry.name.clone(),
            source: remote_path.clone(),
            destination,
            remote_path: remote_path.clone(),
            local_root: local_dir.to_string(),
            total_bytes,
            transferred_bytes: 0,
            status: FileTransferStatus::Running,
            message: "queued".to_string(),
        },
    );
    queue_payload(
        outbound,
        &transfer_request_payload(
            "download",
            transfer_id,
            FileTransferDirection::Download,
            &remote_path,
            Some(("local_dir", local_dir)),
        ),
    );
}

fn add_transfer_row(transfers: &Arc<Mutex<Vec<FileTransferRow>>>, row: FileTransferRow) {
    if let Ok(mut rows) = transfers.lock() {
        rows.retain(|existing| existing.transfer_id != row.transfer_id);
        rows.insert(0, row);
    }
}

fn update_transfer_status(
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
    transfer_id: u64,
    status: FileTransferStatus,
    total_bytes: Option<u64>,
    transferred_bytes: Option<u64>,
    message: &str,
) {
    let Ok(mut rows) = transfers.lock() else {
        return;
    };
    let Some(row) = rows.iter_mut().find(|row| row.transfer_id == transfer_id) else {
        return;
    };
    let keep_failed = row.status == FileTransferStatus::Failed
        && matches!(
            status,
            FileTransferStatus::Done | FileTransferStatus::Cancelled
        );
    let keep_terminal = transfer_status_is_terminal(row.status)
        && matches!(
            status,
            FileTransferStatus::Scanning
                | FileTransferStatus::Running
                | FileTransferStatus::Cancelling
        );
    if !keep_failed && !keep_terminal {
        row.status = status;
    }
    if let Some(total_bytes) = total_bytes {
        if total_bytes > 0 {
            row.total_bytes = row.total_bytes.max(total_bytes);
        }
    }
    if let Some(transferred_bytes) = transferred_bytes {
        row.transferred_bytes = row.transferred_bytes.max(transferred_bytes);
    }
    if !message.trim().is_empty() && !keep_failed && !keep_terminal {
        row.message = message.to_string();
    }
}

fn transfer_status_is_terminal(status: FileTransferStatus) -> bool {
    matches!(
        status,
        FileTransferStatus::Done | FileTransferStatus::Failed | FileTransferStatus::Cancelled
    )
}

fn transfer_request_payload(
    action: &str,
    transfer_id: u64,
    direction: FileTransferDirection,
    remote_path: &str,
    extra: Option<(&str, &str)>,
) -> String {
    let mut payload = format!(
        "{TRANSFER_REQUEST_MARKER}\naction={action}\ntransfer_id={transfer_id}\ndirection={}\nremote_path={remote_path}",
        direction.as_str()
    );
    if let Some((key, value)) = extra {
        payload.push('\n');
        payload.push_str(key);
        payload.push('=');
        payload.push_str(value);
    }
    payload
}

pub(crate) fn parse_transfer_request(payload: &str) -> Option<FileTransferRequest> {
    let mut lines = payload.lines();
    if lines.next()?.trim() != TRANSFER_REQUEST_MARKER {
        return None;
    }
    let mut action = String::new();
    let mut transfer_id = None;
    let mut direction = None;
    let mut remote_path = String::new();
    let mut local_path = String::new();
    let mut local_dir = String::new();
    for line in lines {
        if let Some(rest) = line.strip_prefix("action=") {
            action = rest.trim().to_ascii_lowercase();
        } else if let Some(rest) = line.strip_prefix("transfer_id=") {
            transfer_id = rest.trim().parse::<u64>().ok();
        } else if let Some(rest) = line.strip_prefix("direction=") {
            direction = match rest.trim() {
                "upload" => Some(FileTransferDirection::Upload),
                "download" => Some(FileTransferDirection::Download),
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("remote_path=") {
            remote_path = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("local_path=") {
            local_path = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("local_dir=") {
            local_dir = rest.to_string();
        }
    }
    let transfer_id = transfer_id?;
    match action.as_str() {
        "upload" if !local_path.is_empty() && !remote_path.is_empty() => {
            Some(FileTransferRequest::Upload {
                transfer_id,
                local_path,
                remote_path,
            })
        }
        "download" if !local_dir.is_empty() && !remote_path.is_empty() => {
            Some(FileTransferRequest::Download {
                transfer_id,
                remote_path,
                local_dir,
            })
        }
        "cancel" if !remote_path.is_empty() => Some(FileTransferRequest::Cancel {
            transfer_id,
            direction: direction?,
            remote_path,
        }),
        _ => None,
    }
}

fn request(action: &str, path: &str, value: &str) -> String {
    format!("action={action}\npath={path}\nvalue={value}")
}

fn set_path(window: &mut FileManagerWindow, path: &str) {
    if let Ok(mut value) = window.current_path.lock() {
        *value = path.to_string();
    }
    if let Ok(mut value) = window.path_input.lock() {
        *value = path.to_string();
    }
}

fn set_status(window: &mut FileManagerWindow, status: FileStatus, text: &str) {
    set_status_arc(&window.status, &window.notice, status, text);
}

fn set_status_arc(
    status_state: &Arc<Mutex<FileStatus>>,
    notice_state: &Arc<Mutex<String>>,
    status: FileStatus,
    text: &str,
) {
    if let Ok(mut value) = status_state.lock() {
        *value = status;
    }
    if let Ok(mut value) = notice_state.lock() {
        *value = text.to_string();
    }
}

fn create_folder(
    current_path: &Arc<Mutex<String>>,
    new_folder_name: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let name = new_folder_name
        .lock()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        set_status_arc(status, notice, FileStatus::Failed, "Enter a folder name");
        return;
    }
    if name.contains(['\\', '/', '\n', '\t']) {
        set_status_arc(status, notice, FileStatus::Failed, "Invalid folder name");
        return;
    }
    let path = current_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    queue_payload(outbound, &request("mkdir", &path, &name));
    if let Ok(mut value) = new_folder_name.lock() {
        value.clear();
    }
}

fn create_local_folder(
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
    local_new_folder_name: &Arc<Mutex<String>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let name = local_new_folder_name
        .lock()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            "Enter a local folder name",
        );
        return;
    }
    if name.contains(['\\', '/', '\n', '\t']) {
        set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            "Invalid local folder name",
        );
        return;
    }
    let dir = local_path
        .lock()
        .map(|value| PathBuf::from(value.trim()))
        .unwrap_or_else(|_| PathBuf::from("."));
    match fs::create_dir_all(dir.join(&name)) {
        Ok(()) => {
            if let Ok(mut value) = local_new_folder_name.lock() {
                value.clear();
            }
            refresh_local_entries_arc(local_path, local_entries, selected_local_name);
            set_status_arc(status, notice, FileStatus::Done, "Local folder created");
        }
        Err(error) => set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            &format!("Create local folder failed: {error}"),
        ),
    }
}

fn delete_local_path(
    path: &str,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let path = PathBuf::from(path);
    let result = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path),
        Ok(_) => fs::remove_file(&path),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => {
            refresh_local_entries_arc(local_path, local_entries, selected_local_name);
            set_status_arc(status, notice, FileStatus::Done, "Local item deleted");
        }
        Err(error) => set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            &format!("Delete local item failed: {error}"),
        ),
    }
}

fn rename_local_path(
    path: &str,
    new_name: &str,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name.contains(['\\', '/', '\n', '\t']) {
        set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            "Invalid local item name",
        );
        return;
    }
    let path = PathBuf::from(path);
    let Some(parent) = path.parent() else {
        set_status_arc(status, notice, FileStatus::Failed, "Invalid local path");
        return;
    };
    match fs::rename(&path, parent.join(new_name)) {
        Ok(()) => {
            refresh_local_entries_arc(local_path, local_entries, selected_local_name);
            set_status_arc(status, notice, FileStatus::Done, "Local item renamed");
        }
        Err(error) => set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            &format!("Rename local item failed: {error}"),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn choose_upload_local(
    mode: LocalPickMode,
    current_path: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    let current = local_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let path = match mode {
        LocalPickMode::File => pick_local_file(&current, "Choose file to upload"),
        LocalPickMode::Folder => pick_local_folder(&current, "Choose folder to upload"),
    };
    let Some(path) = path else {
        set_status_arc(status, notice, FileStatus::Ready, "Upload cancelled");
        return;
    };
    queue_upload_local_path(current_path, &path, outbound, status, notice, transfers);
}

fn upload_selected_local(
    current_path: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local: &FileSelection,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    let entries = selected_entries(local_entries, selected_local);
    if entries.is_empty() {
        set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            "Select a local file or folder",
        );
        return;
    }

    let count = entries.len();
    for entry in entries {
        let local_file = join_local(local_path, &entry.name);
        queue_upload_local_path(
            current_path,
            &PathBuf::from(local_file),
            outbound,
            status,
            notice,
            transfers,
        );
    }
    set_status_arc(
        status,
        notice,
        FileStatus::Done,
        &format!("{count} upload{} queued", plural_suffix(count)),
    );
}

fn queue_upload_local_path(
    current_path: &Arc<Mutex<String>>,
    local_file: &Path,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
    transfers: &Arc<Mutex<Vec<FileTransferRow>>>,
) {
    let Some(name) = local_file.file_name().and_then(|value| value.to_str()) else {
        set_status_arc(status, notice, FileStatus::Failed, "Invalid local path");
        return;
    };
    let metadata = match fs::metadata(local_file) {
        Ok(metadata) => metadata,
        Err(error) => {
            set_status_arc(
                status,
                notice,
                FileStatus::Failed,
                &format!("Read local item failed: {error}"),
            );
            return;
        }
    };
    if !metadata.is_file() && !metadata.is_dir() {
        set_status_arc(
            status,
            notice,
            FileStatus::Failed,
            "Select a local file or folder",
        );
        return;
    }

    let transfer_id = NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed);
    let local_file = local_file.to_string_lossy().to_string();
    let remote_path = join_remote(current_path, name);
    let total_bytes = if metadata.is_file() {
        metadata.len()
    } else {
        0
    };
    add_transfer_row(
        transfers,
        FileTransferRow {
            transfer_id,
            direction: FileTransferDirection::Upload,
            name: name.to_string(),
            source: local_file.clone(),
            destination: remote_path.clone(),
            remote_path: remote_path.clone(),
            local_root: String::new(),
            total_bytes,
            transferred_bytes: 0,
            status: FileTransferStatus::Running,
            message: "queued".to_string(),
        },
    );
    queue_payload(
        outbound,
        &transfer_request_payload(
            "upload",
            transfer_id,
            FileTransferDirection::Upload,
            &remote_path,
            Some(("local_path", &local_file)),
        ),
    );
    set_status_arc(status, notice, FileStatus::Done, "Upload queued");
}

fn selected_remote_entries(
    entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_name: &FileSelection,
) -> Vec<FileEntry> {
    selected_entries(entries, selected_name)
}

fn is_pending(status: &Arc<Mutex<FileStatus>>) -> bool {
    status
        .lock()
        .map(|value| matches!(*value, FileStatus::Pending))
        .unwrap_or(false)
}

fn select_entry(selected_name: &FileSelection, entry: &FileEntry) {
    if let Ok(mut selected) = selected_name.lock() {
        selected.names.clear();
        selected.names.push(entry.name.clone());
    }
}

fn clear_selection(selected_name: &FileSelection) {
    if let Ok(mut selected) = selected_name.lock() {
        selected.names.clear();
    }
}

fn selected_names(selected_name: &FileSelection) -> Vec<String> {
    selected_name
        .lock()
        .map(|selected| selected.names.clone())
        .unwrap_or_default()
}

fn set_entry_checked(selected_name: &FileSelection, entry: &FileEntry, checked: bool) {
    if let Ok(mut selected) = selected_name.lock() {
        if checked {
            if !selected.names.iter().any(|name| name == &entry.name) {
                selected.names.push(entry.name.clone());
            }
        } else {
            selected.names.retain(|name| name != &entry.name);
        }
    }
}

fn selected_entries(
    entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_name: &FileSelection,
) -> Vec<FileEntry> {
    let names = selected_names(selected_name);
    if names.is_empty() {
        return Vec::new();
    }
    entries
        .lock()
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| {
                    names.iter().any(|name| name == &entry.name)
                        && matches!(entry.kind.as_str(), "file" | "dir")
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn current_directory_path(path: &Arc<Mutex<String>>) -> String {
    path.lock().map(|value| value.clone()).unwrap_or_default()
}

fn refresh_local_entries(window: &FileManagerWindow) {
    refresh_local_entries_arc(
        &window.local_path,
        &window.local_entries,
        &window.selected_local_name,
    );
}

fn refresh_local_entries_if_download_target_visible(window: &FileManagerWindow, transfer_id: u64) {
    let target = transfer_local_root(window, transfer_id);
    let current = window
        .local_path
        .lock()
        .map(|value| PathBuf::from(value.trim()))
        .unwrap_or_else(|_| PathBuf::from("."));
    if same_local_dir(&target, &current) {
        refresh_local_entries(window);
    }
}

fn same_local_dir(left: &Path, right: &Path) -> bool {
    let normalized_left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let normalized_right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    normalized_left == normalized_right
}

fn refresh_local_entries_arc(
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
) {
    let path = local_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let entries = read_local_entries(&path);
    if let Ok(mut value) = local_entries.lock() {
        *value = entries;
    }
    clear_selection(selected_local_name);
}

fn create_download_directory(
    window: &FileManagerWindow,
    transfer_id: u64,
    relative_path: &str,
) -> Result<(), String> {
    let root = transfer_local_root(window, transfer_id);
    let target = safe_local_join(&root, relative_path)
        .ok_or_else(|| "download directory failed: invalid path".to_string())?;
    fs::create_dir_all(target).map_err(|error| format!("download directory failed: {error}"))
}

fn write_download_chunk(
    window: &FileManagerWindow,
    transfer_id: u64,
    relative_path: &str,
    file_size: u64,
    offset: u64,
    bytes: &[u8],
) -> Result<(), String> {
    let root = transfer_local_root(window, transfer_id);
    let target = safe_local_join(&root, relative_path)
        .ok_or_else(|| "download chunk failed: invalid path".to_string())?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("download mkdir failed: {error}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(offset == 0)
        .open(&target)
        .map_err(|error| format!("download open failed: {error}"))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("download seek failed: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("download write failed: {error}"))?;
    if file_size > 0 && offset.saturating_add(bytes.len() as u64) >= file_size {
        let _ = file.set_len(file_size);
    }
    Ok(())
}

fn transfer_local_root(window: &FileManagerWindow, transfer_id: u64) -> PathBuf {
    window
        .transfers
        .lock()
        .ok()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.transfer_id == transfer_id)
                .map(|row| row.local_root.clone())
        })
        .filter(|path| !path.trim().is_empty())
        .map(|path| PathBuf::from(path.trim()))
        .unwrap_or_else(|| {
            window
                .local_path
                .lock()
                .map(|value| PathBuf::from(value.trim()))
                .unwrap_or_else(|_| PathBuf::from("."))
        })
}

fn safe_local_join(root: &Path, relative_path: &str) -> Option<PathBuf> {
    let relative_path = relative_path.trim();
    if relative_path.is_empty() {
        return Some(root.to_path_buf());
    }
    if is_remote_absolute_path(relative_path) {
        return None;
    }
    let mut path = root.to_path_buf();
    for part in relative_path.split(is_remote_path_separator) {
        match part {
            "" | "." => {}
            ".." => return None,
            _ if part.contains('\0') => return None,
            _ => path.push(part),
        }
    }
    Some(path)
}

fn set_local_dir(
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &FileSelection,
    path: &str,
) {
    let dir = PathBuf::from(path.trim());
    if !dir.is_dir() {
        return;
    }
    let display = dir.display().to_string();
    if let Ok(mut value) = local_path.lock() {
        *value = display.clone();
    }
    if let Ok(mut value) = local_entries.lock() {
        *value = read_local_entries(&display);
    }
    clear_selection(selected_local_name);
}

fn read_local_entries(path: &str) -> Vec<FileEntry> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(path) else {
        return rows;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let kind = if metadata.is_dir() { "dir" } else { "file" };
        let size = if metadata.is_file() {
            metadata.len().to_string()
        } else {
            String::new()
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_default();
        let name = entry
            .file_name()
            .to_string_lossy()
            .replace(['\t', '\n'], " ");
        rows.push(FileEntry {
            kind: kind.to_string(),
            name,
            size,
            modified,
        });
    }
    rows.sort_by(|left, right| {
        let left_dir = left.kind == "dir";
        let right_dir = right.kind == "dir";
        right_dir.cmp(&left_dir).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    rows
}

fn begin_delete(pending_delete: &Arc<Mutex<Option<String>>>, remote_path: &str) {
    if let Ok(mut value) = pending_delete.lock() {
        *value = Some(remote_path.to_string());
    }
}

fn begin_new_folder(pending_new_folder: &Arc<Mutex<bool>>, new_folder_name: &Arc<Mutex<String>>) {
    if let Ok(mut value) = pending_new_folder.lock() {
        *value = true;
    }
    if let Ok(mut value) = new_folder_name.lock() {
        value.clear();
    }
}

fn begin_local_delete(
    pending_local_delete: &Arc<Mutex<Option<String>>>,
    local_path: &Arc<Mutex<String>>,
    name: &str,
) {
    if let Ok(mut value) = pending_local_delete.lock() {
        *value = Some(join_local(local_path, name));
    }
}

fn begin_local_new_folder(
    pending_local_new_folder: &Arc<Mutex<bool>>,
    local_new_folder_name: &Arc<Mutex<String>>,
) {
    if let Ok(mut value) = pending_local_new_folder.lock() {
        *value = true;
    }
    if let Ok(mut value) = local_new_folder_name.lock() {
        value.clear();
    }
}

fn begin_rename(
    pending_rename: &Arc<Mutex<Option<String>>>,
    rename_to: &Arc<Mutex<String>>,
    remote_path: &str,
    current_name: &str,
) {
    if let Ok(mut value) = pending_rename.lock() {
        *value = Some(remote_path.to_string());
    }
    if let Ok(mut value) = rename_to.lock() {
        *value = current_name.to_string();
    }
}

fn begin_local_rename(
    pending_local_rename: &Arc<Mutex<Option<String>>>,
    local_rename_to: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    current_name: &str,
) {
    if let Ok(mut value) = pending_local_rename.lock() {
        *value = Some(join_local(local_path, current_name));
    }
    if let Ok(mut value) = local_rename_to.lock() {
        *value = current_name.to_string();
    }
}

fn join_remote(current_path: &Arc<Mutex<String>>, name: &str) -> String {
    let current = current_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    if current.ends_with('\\') || current.ends_with('/') {
        format!("{current}{name}")
    } else if current.contains('\\') {
        format!("{current}\\{name}")
    } else {
        format!("{current}/{name}")
    }
}

fn join_local(local_path: &Arc<Mutex<String>>, name: &str) -> String {
    let base = local_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    PathBuf::from(base).join(name).display().to_string()
}

fn parent_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    let trimmed = path.trim_end_matches(is_remote_path_separator);
    if trimmed.is_empty() || is_windows_drive_label(trimmed) {
        return path.to_string();
    }
    let Some(separator_index) = trimmed.rfind(is_remote_path_separator) else {
        return path.to_string();
    };
    if separator_index == 0 {
        return trimmed[..=separator_index].to_string();
    }
    let parent = &trimmed[..separator_index];
    if is_windows_drive_label(parent) {
        return format!("{parent}{}", &trimmed[separator_index..=separator_index]);
    }
    parent.to_string()
}

fn remote_file_name(path: &str) -> String {
    path.trim()
        .trim_end_matches(is_remote_path_separator)
        .rsplit(is_remote_path_separator)
        .find(|part| !part.trim().is_empty())
        .unwrap_or("download.bin")
        .to_string()
}

fn is_remote_path_separator(ch: char) -> bool {
    matches!(ch, '\\' | '/')
}

fn is_remote_absolute_path(path: &str) -> bool {
    path.starts_with(is_remote_path_separator) || has_windows_drive_prefix(path)
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_windows_drive_label(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn save_download(window: &FileManagerWindow, response: &FileResponse) -> Result<String, String> {
    let bytes = decode_hex(response.value.as_deref().unwrap_or(""))?;
    let local_dir = window
        .local_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let dir = if local_dir.trim().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(local_dir.trim())
    };
    if !dir.is_dir() {
        return Err("Local target is not a directory".to_string());
    }
    let name = remote_file_name(&response.path);
    let target = dir.join(name);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    fs::write(&target, bytes).map_err(|error| error.to_string())?;
    Ok(format!("Downloaded to {}", target.display()))
}

struct FileResponse {
    kind: String,
    cwd: String,
    path: String,
    value: Option<String>,
    message: Option<String>,
    entries: Vec<FileEntry>,
}

impl FileResponse {
    fn parse(detail: &str) -> Self {
        let mut lines = detail.lines();
        let kind = lines.next().unwrap_or("error").trim().to_string();
        let mut cwd = String::new();
        let mut path = String::new();
        let mut value = None;
        let mut message = None;
        let mut entries = Vec::new();
        let mut in_entries = false;
        for line in lines {
            if in_entries {
                let parts = line.split('\t').collect::<Vec<_>>();
                if parts.len() >= 4 {
                    entries.push(FileEntry {
                        kind: parts[0].to_string(),
                        name: parts[1].to_string(),
                        size: parts[2].to_string(),
                        modified: parts[3].to_string(),
                    });
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("cwd=") {
                cwd = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("path=") {
                path = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("value=") {
                value = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("message=") {
                message = Some(rest.to_string());
            } else if line.starts_with("entries=") {
                in_entries = true;
            }
        }
        Self {
            kind,
            cwd,
            path,
            value,
            message,
            entries,
        }
    }
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("invalid hex length".to_string());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks(2) {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex data".to_string()),
    }
}

fn identity_title(hostname: &str, username: &str) -> String {
    match (hostname.trim(), username.trim()) {
        ("", "") => "unknown-host".to_string(),
        (host, "") => host.to_string(),
        ("", user) => user.to_string(),
        (host, user) => format!("{host} / {user}"),
    }
}

use eframe::egui;
use egui_extras::{Column, Size, StripBuilder, TableBuilder};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(246, 248, 251);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(222, 228, 236);
const COLOR_PANEL: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(24, 33, 47);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(96, 108, 124);
const COLOR_GOOD: egui::Color32 = egui::Color32::from_rgb(24, 135, 84);
const COLOR_BAD: egui::Color32 = egui::Color32::from_rgb(190, 58, 58);
const COLOR_WARN: egui::Color32 = egui::Color32::from_rgb(179, 116, 28);

pub(crate) struct FileManagerWindow {
    pub(crate) client_id: String,
    hostname: String,
    username: String,
    current_path: Arc<Mutex<String>>,
    path_input: Arc<Mutex<String>>,
    entries: Arc<Mutex<Vec<FileEntry>>>,
    selected_name: Arc<Mutex<Option<String>>>,
    local_entries: Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: Arc<Mutex<Option<String>>>,
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
    open: bool,
    close_requested: Arc<AtomicBool>,
}

#[derive(Clone)]
struct FileEntry {
    kind: String,
    name: String,
    size: String,
    modified: String,
}

#[derive(Clone, Copy)]
enum FileStatus {
    Ready,
    Pending,
    Done,
    Failed,
}

pub(crate) struct OutboundCommand {
    pub(crate) client_id: String,
    pub(crate) payload: String,
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
        selected_name: Arc::new(Mutex::new(None)),
        local_entries: Arc::new(Mutex::new(read_local_entries(&local_dir))),
        selected_local_name: Arc::new(Mutex::new(None)),
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
        open: true,
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
    if let Ok(mut selected) = window.selected_name.lock() {
        *selected = None;
    }
    set_status(window, FileStatus::Done, "Directory loaded");
}

pub(crate) fn render_windows(
    ctx: &egui::Context,
    windows: &mut Vec<FileManagerWindow>,
) -> Vec<OutboundCommand> {
    let mut outbound = Vec::new();
    for window in windows.iter_mut() {
        if window.close_requested.load(Ordering::Relaxed) {
            window.open = false;
        }
        if !window.open {
            continue;
        }

        let client_id = window.client_id.clone();
        let title = format!(
            "File Manager - {}",
            identity_title(&window.hostname, &window.username)
        );
        let viewport_id = egui::ViewportId::from_hash_of(("admin_file_manager", &client_id));
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1080.0, 620.0])
            .with_min_inner_size([820.0, 460.0])
            .with_resizable(true);

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
        let close_requested = window.close_requested.clone();
        let entries_id = client_id.clone();

        ctx.show_viewport_immediate(viewport_id, builder, move |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(COLOR_BG).inner_margin(12.0))
                .show_inside(ui, |ui| {
                    let content_height = (ui.available_height() - 52.0).max(260.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), content_height),
                        egui::Layout::left_to_right(egui::Align::Min),
                        |ui| {
                            StripBuilder::new(ui)
                                .size(Size::remainder())
                                .size(Size::exact(104.0))
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
                                        );
                                    });
                                });
                        },
                    );
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

        let payload = window
            .outbound
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop());
        if let Some(payload) = payload {
            set_status(window, FileStatus::Pending, "Waiting for client result");
            outbound.push(OutboundCommand {
                client_id: client_id.clone(),
                payload,
            });
        }
    }
    windows.retain(|window| window.open);
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
    selected_name: &Arc<Mutex<Option<String>>>,
    rename_to: &Arc<Mutex<String>>,
    new_folder_name: &Arc<Mutex<String>>,
    pending_delete: &Arc<Mutex<Option<String>>>,
    pending_rename: &Arc<Mutex<Option<String>>>,
    pending_new_folder: &Arc<Mutex<bool>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Remote")
                        .size(13.0)
                        .color(COLOR_TEXT)
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
    selected_local_name: &Arc<Mutex<Option<String>>>,
    local_rename_to: &Arc<Mutex<String>>,
    local_new_folder_name: &Arc<Mutex<String>>,
    pending_local_delete: &Arc<Mutex<Option<String>>>,
    pending_local_rename: &Arc<Mutex<Option<String>>>,
    pending_local_new_folder: &Arc<Mutex<bool>>,
    current_path: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Local")
                        .size(13.0)
                        .color(COLOR_TEXT)
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
                );
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn render_transfer_buttons(
    ui: &mut egui::Ui,
    current_path: &Arc<Mutex<String>>,
    remote_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_remote: &Arc<Mutex<Option<String>>>,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local: &Arc<Mutex<Option<String>>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    ui.vertical_centered(|ui| {
        ui.set_min_size(ui.available_size());
        ui.add_space(150.0);
        if ui
            .add_enabled(!is_pending(status), egui::Button::new("Download ->"))
            .clicked()
        {
            if let Some(path) =
                selected_remote_file_path(current_path, remote_entries, selected_remote)
            {
                queue_payload(outbound, &request("download", &path, ""));
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            } else {
                set_status_arc(status, notice, FileStatus::Failed, "Select a remote file");
            }
        }
        ui.add_space(8.0);
        if ui
            .add_enabled(!is_pending(status), egui::Button::new("<- Upload"))
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
        let busy = is_pending(status);
        if ui.add_enabled(!busy, egui::Button::new("Up")).clicked() {
            let path = current_path
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            queue_payload(outbound, &request("list", &parent_path(&path), ""));
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Refresh"))
            .clicked()
        {
            let path = current_path
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            queue_payload(outbound, &request("list", &path, ""));
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
        let mut path = path_input
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let response = ui.add_sized(
            [(ui.available_width() - 230.0).max(90.0), 28.0],
            egui::TextEdit::singleline(&mut path).hint_text("Remote path"),
        );
        if response.changed() {
            if let Ok(mut value) = path_input.lock() {
                *value = path.clone();
            }
        }
        let go = ui.add_enabled(!busy, egui::Button::new("Go")).clicked()
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
    selected_local_name: &Arc<Mutex<Option<String>>>,
) {
    ui.horizontal(|ui| {
        if ui.button("Up").clicked() {
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
        if ui.button("Refresh").clicked() {
            refresh_local_entries_arc(local_path, local_entries, selected_local_name);
        }
        let mut path = local_path
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let response = ui.add_sized(
            [(ui.available_width() - 42.0).max(90.0), 28.0],
            egui::TextEdit::singleline(&mut path).hint_text("Local path"),
        );
        if response.changed() {
            if let Ok(mut value) = local_path.lock() {
                *value = path.clone();
            }
        }
        let go = ui.button("Go").clicked()
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
    selected_name: &Arc<Mutex<Option<String>>>,
    rename_to: &Arc<Mutex<String>>,
    pending_delete: &Arc<Mutex<Option<String>>>,
    pending_rename: &Arc<Mutex<Option<String>>>,
    pending_new_folder: &Arc<Mutex<bool>>,
    new_folder_name: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
) {
    let entries = entries
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let selected = selected_name
        .lock()
        .map(|value| value.clone())
        .unwrap_or(None);
    file_table(
        ui,
        ("remote_file_table_v2", entries_id),
        &entries,
        selected.as_deref(),
        |ui, entry| {
            let row_response = selectable_row_label(
                ui,
                selected.as_deref() == Some(entry.name.as_str()),
                &entry.name,
            );
            if row_response.clicked() {
                select_entry(selected_name, entry);
            }
            if row_response.double_clicked() && entry.kind == "dir" && !is_pending(status) {
                let path = join_remote(current_path, &entry.name);
                queue_payload(outbound, &request("list", &path, ""));
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            row_response.context_menu(|ui| {
                if ui.button("Open").clicked() && entry.kind == "dir" {
                    let path = join_remote(current_path, &entry.name);
                    queue_payload(outbound, &request("list", &path, ""));
                    ui.close();
                }
                if ui.button("Download").clicked() && entry.kind == "file" {
                    let path = join_remote(current_path, &entry.name);
                    queue_payload(outbound, &request("download", &path, ""));
                    ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    let path = join_remote(current_path, &entry.name);
                    begin_delete(pending_delete, &path);
                    ui.close();
                }
                ui.separator();
                if ui.button("New Folder").clicked() {
                    begin_new_folder(pending_new_folder, new_folder_name);
                    ui.close();
                }
                if ui.button("Rename").clicked() {
                    let path = join_remote(current_path, &entry.name);
                    begin_rename(pending_rename, rename_to, &path, &entry.name);
                    ui.close();
                }
            });
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn render_local_entries_table(
    ui: &mut egui::Ui,
    entries_id: &str,
    local_path: &Arc<Mutex<String>>,
    entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_name: &Arc<Mutex<Option<String>>>,
    local_rename_to: &Arc<Mutex<String>>,
    local_new_folder_name: &Arc<Mutex<String>>,
    pending_local_delete: &Arc<Mutex<Option<String>>>,
    pending_local_rename: &Arc<Mutex<Option<String>>>,
    pending_local_new_folder: &Arc<Mutex<bool>>,
    current_path: &Arc<Mutex<String>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let entries_snapshot = entries
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let selected = selected_name
        .lock()
        .map(|value| value.clone())
        .unwrap_or(None);
    file_table(
        ui,
        ("local_file_table_v2", entries_id),
        &entries_snapshot,
        selected.as_deref(),
        |ui, entry| {
            let row_response = selectable_row_label(
                ui,
                selected.as_deref() == Some(entry.name.as_str()),
                &entry.name,
            );
            if row_response.clicked() {
                select_entry(selected_name, entry);
            }
            if row_response.double_clicked() && entry.kind == "dir" {
                let path = join_local(local_path, &entry.name);
                set_local_dir(local_path, entries, selected_name, &path);
            }
            row_response.context_menu(|ui| {
                if ui.button("Open").clicked() && entry.kind == "dir" {
                    let path = join_local(local_path, &entry.name);
                    set_local_dir(local_path, entries, selected_name, &path);
                    ui.close();
                }
                if ui.button("Upload").clicked() && entry.kind == "file" {
                    select_entry(selected_name, entry);
                    upload_selected_local(
                        current_path,
                        local_path,
                        entries,
                        selected_name,
                        outbound,
                        status,
                        notice,
                    );
                    ui.close();
                }
                ui.separator();
                if ui.button("New Folder").clicked() {
                    begin_local_new_folder(pending_local_new_folder, local_new_folder_name);
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    begin_local_delete(pending_local_delete, local_path, &entry.name);
                    ui.close();
                }
                if ui.button("Rename").clicked() {
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
}

fn file_table<R>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    entries: &[FileEntry],
    selected: Option<&str>,
    mut name_cell: impl FnMut(&mut egui::Ui, &FileEntry) -> R,
) {
    let available_width = ui.available_width().max(360.0);
    let type_width = 44.0;
    let size_width = 76.0;
    let modified_width = 104.0;
    let name_width = (available_width - type_width - size_width - modified_width - 24.0).max(140.0);
    let table = TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .resizable(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(type_width))
        .column(Column::exact(name_width))
        .column(Column::exact(size_width))
        .column(Column::exact(modified_width));
    table.reset();
    table
        .header(24.0, |mut header| {
            header.col(|ui| table_header_label(ui, "Type"));
            header.col(|ui| table_header_label(ui, "Name"));
            header.col(|ui| table_header_label(ui, "Size"));
            header.col(|ui| table_header_label(ui, "Modified"));
        })
        .body(|mut body| {
            for entry in entries {
                let is_selected = selected == Some(entry.name.as_str());
                body.row(24.0, |mut row| {
                    row.set_selected(is_selected);
                    row.col(|ui| table_text(ui, &entry.kind));
                    row.col(|ui| {
                        name_cell(ui, entry);
                    });
                    row.col(|ui| table_text(ui, &entry.size));
                    row.col(|ui| table_text(ui, &entry.modified));
                });
            }
        });
}

fn table_header_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(12.0)
            .color(COLOR_TEXT)
            .strong(),
    );
}

fn table_text(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.0).color(COLOR_TEXT));
}

fn selectable_row_label(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    ui.selectable_label(
        selected,
        egui::RichText::new(text).size(12.0).color(COLOR_TEXT),
    )
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
        FileStatus::Ready => ("Ready", COLOR_MUTED),
        FileStatus::Pending => ("Pending", COLOR_WARN),
        FileStatus::Done => ("Done", COLOR_GOOD),
        FileStatus::Failed => ("Failed", COLOR_BAD),
    };
    egui::Frame::default()
        .fill(COLOR_PANEL)
        .stroke(egui::Stroke::new(1.0, COLOR_BORDER))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_min_height(26.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(
                    egui::RichText::new(label)
                        .size(12.0)
                        .color(COLOR_TEXT)
                        .strong(),
                );
                ui.label(egui::RichText::new(notice).size(12.0).color(COLOR_MUTED));
            });
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
    selected_local_name: &Arc<Mutex<Option<String>>>,
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
        egui::Window::new("Confirm Delete")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("Delete this remote item?")
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.label(
                    egui::RichText::new(&remote_path)
                        .size(12.0)
                        .color(COLOR_TEXT),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Delete").color(COLOR_BAD).strong(),
                        ))
                        .clicked()
                    {
                        queue_payload(outbound, &request("delete", &remote_path, ""));
                        if let Ok(mut value) = pending_delete.lock() {
                            *value = None;
                        }
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if ui.button("Cancel").clicked() {
                        if let Ok(mut value) = pending_delete.lock() {
                            *value = None;
                        }
                    }
                });
            });
    }

    let rename_path = pending_rename.lock().ok().and_then(|value| value.clone());
    if let Some(remote_path) = rename_path {
        egui::Window::new("Rename Item")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("Rename remote item")
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.label(
                    egui::RichText::new(&remote_path)
                        .size(12.0)
                        .color(COLOR_TEXT),
                );
                ui.add_space(8.0);
                let mut name = rename_to
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name).hint_text("New name"),
                );
                if response.changed() {
                    if let Ok(mut value) = rename_to.lock() {
                        *value = name.clone();
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        queue_payload(outbound, &request("rename", &remote_path, &name));
                        if let Ok(mut value) = pending_rename.lock() {
                            *value = None;
                        }
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if ui.button("Cancel").clicked() {
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
        egui::Window::new("New Remote Folder")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("Create folder in current remote directory")
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.add_space(8.0);
                let mut name = new_folder_name
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name).hint_text("Folder name"),
                );
                if response.changed() {
                    if let Ok(mut value) = new_folder_name.lock() {
                        *value = name.clone();
                    }
                }
                let create = ui.button("Create").clicked()
                    || (response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                if create {
                    create_folder(current_path, new_folder_name, outbound, status, notice);
                    if let Ok(mut value) = pending_new_folder.lock() {
                        *value = false;
                    }
                    ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                }
                if ui.button("Cancel").clicked() {
                    if let Ok(mut value) = pending_new_folder.lock() {
                        *value = false;
                    }
                }
            });
    }

    let local_delete_path = pending_local_delete
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if let Some(path) = local_delete_path {
        egui::Window::new("Confirm Local Delete")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("Delete this local item?")
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.label(egui::RichText::new(&path).size(12.0).color(COLOR_TEXT));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Delete").color(COLOR_BAD).strong(),
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
                    if ui.button("Cancel").clicked() {
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
        egui::Window::new("Rename Local Item")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("Rename local item")
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.label(egui::RichText::new(&path).size(12.0).color(COLOR_TEXT));
                ui.add_space(8.0);
                let mut name = local_rename_to
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name).hint_text("New name"),
                );
                if response.changed() {
                    if let Ok(mut value) = local_rename_to.lock() {
                        *value = name.clone();
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
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
                    if ui.button("Cancel").clicked() {
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
        egui::Window::new("New Local Folder")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new("Create folder in current local directory")
                        .size(12.0)
                        .color(COLOR_MUTED),
                );
                ui.add_space(8.0);
                let mut name = local_new_folder_name
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                let response = ui.add_sized(
                    [420.0, 28.0],
                    egui::TextEdit::singleline(&mut name).hint_text("Folder name"),
                );
                if response.changed() {
                    if let Ok(mut value) = local_new_folder_name.lock() {
                        *value = name.clone();
                    }
                }
                let create = ui.button("Create").clicked()
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
                if ui.button("Cancel").clicked() {
                    if let Ok(mut value) = pending_local_new_folder.lock() {
                        *value = false;
                    }
                }
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
    selected_local_name: &Arc<Mutex<Option<String>>>,
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
    selected_local_name: &Arc<Mutex<Option<String>>>,
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
    selected_local_name: &Arc<Mutex<Option<String>>>,
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

fn upload_selected_local(
    current_path: &Arc<Mutex<String>>,
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local: &Arc<Mutex<Option<String>>>,
    outbound: &Arc<Mutex<Vec<String>>>,
    status: &Arc<Mutex<FileStatus>>,
    notice: &Arc<Mutex<String>>,
) {
    let Some(name) = selected_local.lock().ok().and_then(|value| value.clone()) else {
        set_status_arc(status, notice, FileStatus::Failed, "Select a local file");
        return;
    };
    let is_file = local_entries
        .lock()
        .map(|entries| {
            entries
                .iter()
                .any(|entry| entry.name == name && entry.kind == "file")
        })
        .unwrap_or(false);
    if !is_file {
        set_status_arc(status, notice, FileStatus::Failed, "Select a local file");
        return;
    }
    let local_file = join_local(local_path, &name);
    match build_upload_payload(current_path, &local_file, &name) {
        Ok(payload) => queue_payload(outbound, &payload),
        Err(error) => set_status_arc(status, notice, FileStatus::Failed, &error),
    }
}

fn selected_remote_file_path(
    current_path: &Arc<Mutex<String>>,
    entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_name: &Arc<Mutex<Option<String>>>,
) -> Option<String> {
    let name = selected_name.lock().ok().and_then(|value| value.clone())?;
    let is_file = entries
        .lock()
        .map(|entries| {
            entries
                .iter()
                .any(|entry| entry.name == name && entry.kind == "file")
        })
        .unwrap_or(false);
    if is_file {
        Some(join_remote(current_path, &name))
    } else {
        None
    }
}

fn is_pending(status: &Arc<Mutex<FileStatus>>) -> bool {
    status
        .lock()
        .map(|value| matches!(*value, FileStatus::Pending))
        .unwrap_or(false)
}

fn select_entry(selected_name: &Arc<Mutex<Option<String>>>, entry: &FileEntry) {
    if let Ok(mut selected) = selected_name.lock() {
        *selected = Some(entry.name.clone());
    }
}

fn refresh_local_entries(window: &FileManagerWindow) {
    refresh_local_entries_arc(
        &window.local_path,
        &window.local_entries,
        &window.selected_local_name,
    );
}

fn refresh_local_entries_arc(
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &Arc<Mutex<Option<String>>>,
) {
    let path = local_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let entries = read_local_entries(&path);
    if let Ok(mut value) = local_entries.lock() {
        *value = entries;
    }
    if let Ok(mut value) = selected_local_name.lock() {
        *value = None;
    }
}

fn set_local_dir(
    local_path: &Arc<Mutex<String>>,
    local_entries: &Arc<Mutex<Vec<FileEntry>>>,
    selected_local_name: &Arc<Mutex<Option<String>>>,
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
    if let Ok(mut value) = selected_local_name.lock() {
        *value = None;
    }
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
    Path::new(path)
        .parent()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn build_upload_payload(
    current_path: &Arc<Mutex<String>>,
    local_path: &str,
    remote_name: &str,
) -> Result<String, String> {
    let local = local_path.trim();
    if local.is_empty() {
        return Err("local path is empty".to_string());
    }
    let bytes = fs::read(local).map_err(|error| error.to_string())?;
    let name = if remote_name.trim().is_empty() {
        Path::new(local)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .ok_or_else(|| "remote name is empty".to_string())?
    } else {
        remote_name.trim().to_string()
    };
    let current = current_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let remote_path = if current.is_empty() {
        name
    } else if current.ends_with('\\') || current.ends_with('/') {
        format!("{current}{name}")
    } else if current.contains('\\') {
        format!("{current}\\{name}")
    } else {
        format!("{current}/{name}")
    };
    Ok(request("upload", &remote_path, &encode_hex(&bytes)))
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
    let name = Path::new(&response.path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "download.bin".to_string());
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

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
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

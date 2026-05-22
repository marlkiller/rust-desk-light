use super::{
    filtered_table_rows, normalized_table_header, parse_result_table, stable_hash, table_row_key,
    table_value, DisplayTableRow, ResultTable, TABLE_BODY_CELL_HEIGHT, TABLE_BODY_TEXT_SIZE,
    TABLE_HEADER_CELL_HEIGHT, TABLE_HEADER_TEXT_SIZE,
};
use crate::{i18n::t, theme::table_cell_label};
use base64::{engine::general_purpose::STANDARD, Engine};
use eframe::egui;
use egui_extras::Column;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub(super) fn merge_details(existing: &str, incoming: &str) -> Option<String> {
    let mut incoming_table = parse_result_table(incoming)?;
    normalize_registry_table_hives(&mut incoming_table);
    let Some(mut existing_table) = parse_result_table(existing) else {
        return Some(serialize_result_table("registry_manager", &incoming_table));
    };
    normalize_registry_table_hives(&mut existing_table);

    if !same_table_headers(&existing_table.headers, &incoming_table.headers) {
        return Some(serialize_result_table("registry_manager", &incoming_table));
    }

    let mut seen = existing_table
        .rows
        .iter()
        .map(|row| row.join("\t"))
        .collect::<HashSet<_>>();
    for row in incoming_table.rows {
        if seen.insert(row.join("\t")) {
            existing_table.rows.push(row);
        }
    }

    Some(serialize_result_table("registry_manager", &existing_table))
}

fn normalize_registry_table_hives(table: &mut ResultTable) {
    let Some(hive_index) = table
        .headers
        .iter()
        .position(|header| normalized_table_header(header) == "hive")
    else {
        return;
    };
    for row in &mut table.rows {
        let Some(hive) = row.get_mut(hive_index) else {
            continue;
        };
        if let Some(canonical) = registry_canonical_hive(hive) {
            *hive = canonical.to_string();
        }
    }
}

pub(super) fn render_result(
    ui: &mut egui::Ui,
    table: &ResultTable,
    table_filter: &Arc<Mutex<String>>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
) {
    let filter = table_filter
        .lock()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let groups = registry_groups(table, &filter);

    if groups.is_empty() {
        ui.label(
            egui::RichText::new(t("No registry values match the current filter."))
                .size(12.0)
                .color(crate::theme::palette().muted),
        );
        return;
    }

    let selected_state = table_selected_row
        .lock()
        .map(|value| value.clone())
        .unwrap_or(None);
    let selected_group_index = registry_selected_group_index(&groups, selected_state.as_deref());

    render_registry_browser(
        ui,
        table,
        &groups,
        selected_group_index,
        &selected_state,
        table_selected_row,
        registry_key_requested,
        registry_expanded_keys,
    );
}

struct RegistryGroup {
    hive: String,
    path: String,
    can_expand: bool,
    rows: Vec<DisplayTableRow>,
}

struct RegistryTreeRoot {
    hive: String,
    group_index: Option<usize>,
    children: Vec<RegistryTreeNode>,
}

struct RegistryTreeNode {
    name: String,
    group_index: Option<usize>,
    children: Vec<RegistryTreeNode>,
}

fn render_registry_browser(
    ui: &mut egui::Ui,
    table: &ResultTable,
    groups: &[RegistryGroup],
    selected_group_index: usize,
    selected_state: &Option<String>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
) {
    let tree_width = (ui.available_width() * 0.34).clamp(260.0, 360.0);
    let tree_scroll_height = (ui.clip_rect().height() - 96.0).clamp(180.0, 420.0);
    ui.horizontal_top(|ui| {
        render_registry_tree_panel(
            ui,
            groups,
            selected_group_index,
            table_selected_row,
            registry_key_requested,
            registry_expanded_keys,
            tree_width,
            tree_scroll_height,
        );
        ui.add_space(8.0);
        if let Some(group) = groups.get(selected_group_index) {
            render_registry_values_panel(ui, table, group, selected_state, table_selected_row);
        }
    });
}

fn render_registry_tree_panel(
    ui: &mut egui::Ui,
    groups: &[RegistryGroup],
    selected_group_index: usize,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
    width: f32,
    scroll_height: f32,
) {
    egui::Frame::default()
        .fill(crate::theme::palette().panel_subtle)
        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 8))
        .show(ui, |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.vertical(|ui| {
                ui.label(registry_tree_label(t("Registry"), true));
                ui.add_space(3.0);
                egui::ScrollArea::both()
                    .id_salt("registry_tree_scroll")
                    .max_height(scroll_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_min_width((width - 24.0).max(180.0));
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(registry_tree_label(t("Computer"), false));
                        for root in registry_tree(groups) {
                            render_registry_tree_root(
                                ui,
                                groups,
                                &root,
                                selected_group_index,
                                table_selected_row,
                                registry_key_requested,
                                registry_expanded_keys,
                            );
                        }
                    });
            });
        });
}

fn render_registry_tree_root(
    ui: &mut egui::Ui,
    groups: &[RegistryGroup],
    root: &RegistryTreeRoot,
    selected_group_index: usize,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
) {
    let path = registry_display_hive(&root.hive);
    let root_open = registry_path_expanded(registry_expanded_keys, &path);
    let can_expand = registry_tree_root_can_expand(groups, root);
    let selected = root
        .group_index
        .map(|index| index == selected_group_index)
        .unwrap_or(false);
    render_registry_tree_row(
        ui,
        registry_expanded_keys,
        table_selected_row,
        registry_key_requested,
        &path,
        &root.hive,
        selected,
        can_expand,
        0,
    );

    if root_open {
        for node in &root.children {
            render_registry_tree_node(
                ui,
                groups,
                node,
                selected_group_index,
                table_selected_row,
                registry_key_requested,
                registry_expanded_keys,
                &root.hive,
                1,
            );
        }
    }
}

fn render_registry_tree_node(
    ui: &mut egui::Ui,
    groups: &[RegistryGroup],
    node: &RegistryTreeNode,
    selected_group_index: usize,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
    id_prefix: &str,
    depth: usize,
) {
    if node.children.is_empty() {
        if let Some(group_index) = node.group_index {
            render_registry_tree_leaf(
                ui,
                groups,
                group_index,
                selected_group_index,
                table_selected_row,
                registry_key_requested,
                registry_expanded_keys,
                depth,
            );
        }
        return;
    }

    let id = format!("{id_prefix}\\{}", node.name);
    let node_open = registry_path_expanded(registry_expanded_keys, &id);
    let can_expand = registry_tree_node_can_expand(groups, node);
    let selected = node
        .group_index
        .map(|index| index == selected_group_index)
        .unwrap_or(false);
    render_registry_tree_row(
        ui,
        registry_expanded_keys,
        table_selected_row,
        registry_key_requested,
        &id,
        &node.name,
        selected,
        can_expand,
        depth,
    );

    if node_open {
        for child in &node.children {
            render_registry_tree_node(
                ui,
                groups,
                child,
                selected_group_index,
                table_selected_row,
                registry_key_requested,
                registry_expanded_keys,
                &id,
                depth + 1,
            );
        }
    }
}

fn registry_tree_label(label: impl Into<String>, strong: bool) -> egui::RichText {
    let text = egui::RichText::new(label)
        .size(11.0)
        .color(crate::theme::palette().text);
    if strong {
        text.strong()
    } else {
        text
    }
}

fn render_registry_tree_row(
    ui: &mut egui::Ui,
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    display_path: &str,
    label: &str,
    selected: bool,
    can_expand: bool,
    depth: usize,
) {
    let expanded = registry_path_expanded(registry_expanded_keys, display_path);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if depth > 0 {
            ui.add_space(depth as f32 * 14.0);
        }
        let toggle_text = if !can_expand {
            "   "
        } else if expanded {
            "[-]"
        } else {
            "[+]"
        };
        let toggle = egui::Label::new(
            egui::RichText::new(toggle_text)
                .size(11.0)
                .monospace()
                .color(crate::theme::palette().muted),
        )
        .sense(if toggle_text.trim().is_empty() {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        });
        let toggle_response = ui.add_sized([24.0, 15.0], toggle);
        if toggle_response.hovered() && !toggle_text.trim().is_empty() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if toggle_response.clicked() && can_expand {
            if expanded {
                collapse_registry_path(registry_expanded_keys, display_path);
            } else {
                expand_registry_path(registry_expanded_keys, display_path);
                queue_registry_display_path_select_and_request(
                    table_selected_row,
                    registry_key_requested,
                    display_path,
                );
            }
        }

        let palette = crate::theme::palette();
        let text = egui::RichText::new(label).size(11.0).color(if selected {
            palette.accent
        } else {
            palette.text
        });
        let text = if selected { text.strong() } else { text };
        let response = ui.add(
            egui::Label::new(text)
                .sense(egui::Sense::click())
                .wrap_mode(egui::TextWrapMode::Extend),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            queue_registry_display_path_select_and_request(
                table_selected_row,
                registry_key_requested,
                display_path,
            );
        }
        response.on_hover_text(display_path).context_menu(|ui| {
            if ui.button(t("Copy Path")).clicked() {
                ui.ctx().copy_text(display_path.to_string());
                ui.close();
            }
        });
    });
}

fn registry_path_expanded(
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
    display_path: &str,
) -> bool {
    registry_expanded_keys
        .lock()
        .map(|keys| keys.contains(display_path))
        .unwrap_or(false)
}

fn expand_registry_path(registry_expanded_keys: &Arc<Mutex<HashSet<String>>>, display_path: &str) {
    if let Ok(mut keys) = registry_expanded_keys.lock() {
        keys.insert(display_path.to_string());
    }
}

fn collapse_registry_path(
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
    display_path: &str,
) {
    if let Ok(mut keys) = registry_expanded_keys.lock() {
        let descendant_prefix = format!("{display_path}\\");
        keys.retain(|key| key != display_path && !key.starts_with(&descendant_prefix));
    }
}

fn render_registry_tree_leaf(
    ui: &mut egui::Ui,
    groups: &[RegistryGroup],
    group_index: usize,
    selected_group_index: usize,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    registry_expanded_keys: &Arc<Mutex<HashSet<String>>>,
    depth: usize,
) {
    let Some(group) = groups.get(group_index) else {
        return;
    };
    let label = registry_path_leaf(&group.path)
        .map(str::to_string)
        .unwrap_or_else(|| registry_display_hive(&group.hive));
    render_registry_tree_row(
        ui,
        registry_expanded_keys,
        table_selected_row,
        registry_key_requested,
        &registry_group_display_path(group),
        &label,
        selected_group_index == group_index,
        group.can_expand,
        depth,
    );
}

fn render_registry_values_panel(
    ui: &mut egui::Ui,
    table: &ResultTable,
    group: &RegistryGroup,
    selected_state: &Option<String>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
) {
    crate::theme::panel_frame()
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width(430.0);
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(registry_display_hive(&group.hive))
                            .size(12.0)
                            .color(crate::theme::palette().text)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(&group.path)
                            .size(12.0)
                            .color(crate::theme::palette().muted)
                            .font(egui::FontId::monospace(12.0)),
                    );
                });
                ui.add_space(8.0);
                render_registry_values_table(ui, table, group, selected_state, table_selected_row);
            });
        });
}

fn render_registry_values_table(
    ui: &mut egui::Ui,
    table: &ResultTable,
    group: &RegistryGroup,
    selected_state: &Option<String>,
    table_selected_row: &Arc<Mutex<Option<String>>>,
) {
    let path_text = registry_display_path(&group.hive, &group.path);
    let value_rows = group
        .rows
        .iter()
        .filter(|row| !registry_row_is_key(&table.headers, &row.cells))
        .collect::<Vec<_>>();
    if value_rows.is_empty() {
        ui.label(
            egui::RichText::new(t("No values in this key."))
                .size(12.0)
                .color(crate::theme::palette().muted),
        );
        return;
    }

    crate::theme::clickable_table(ui, ("registry_values_table", stable_hash(&path_text)), true)
        .auto_shrink([false, true])
        .column(Column::initial(170.0).at_least(110.0).clip(true))
        .column(Column::initial(92.0).at_least(72.0).clip(true))
        .column(Column::remainder().at_least(220.0).clip(true))
        .header(TABLE_HEADER_CELL_HEIGHT + 7.0, |mut header| {
            for label in [t("Name"), t("Type"), t("Data")] {
                header.col(|ui| {
                    let _ = table_cell_label(
                        ui,
                        label,
                        TABLE_HEADER_TEXT_SIZE,
                        crate::theme::palette().muted,
                        egui::Align::Min,
                        egui::Sense::hover(),
                    );
                });
            }
        })
        .body(|body| {
            body.rows(TABLE_BODY_CELL_HEIGHT + 8.0, value_rows.len(), |mut row| {
                let row_data = value_rows[row.index()];
                let row_key = table_row_key(row_data);
                row.set_selected(selected_state.as_deref() == Some(row_key.as_str()));

                let name = table_value(&table.headers, &row_data.cells, "name").unwrap_or("-");
                let type_name = table_value(&table.headers, &row_data.cells, "type").unwrap_or("-");
                let value = table_value(&table.headers, &row_data.cells, "value").unwrap_or("-");
                let row_text = row_data.cells.join("\t");
                let cells = [name, type_name, value];

                for (index, cell) in cells.iter().enumerate() {
                    let (_, cell_response) = row.col(|ui| {
                        let _ = table_cell_label(
                            ui,
                            cell,
                            TABLE_BODY_TEXT_SIZE,
                            crate::theme::palette().text,
                            egui::Align::Min,
                            egui::Sense::hover(),
                        );
                    });
                    let cell_text = (*cell).to_string();
                    let value_text = value.to_string();
                    let name_text = name.to_string();
                    let path_text = path_text.clone();
                    let row_text = row_text.clone();
                    cell_response.context_menu(|ui| {
                        let copy_label = match index {
                            0 => t("Copy Name"),
                            1 => t("Copy Type"),
                            _ => t("Copy Data"),
                        };
                        if ui.button(copy_label).clicked() {
                            ui.ctx().copy_text(cell_text.clone());
                            ui.close();
                        }
                        if ui.button(t("Copy Value Data")).clicked() {
                            ui.ctx().copy_text(value_text.clone());
                            ui.close();
                        }
                        if ui.button(t("Copy Value Name")).clicked() {
                            ui.ctx().copy_text(name_text.clone());
                            ui.close();
                        }
                        if ui.button(t("Copy Key Path")).clicked() {
                            ui.ctx().copy_text(path_text.clone());
                            ui.close();
                        }
                        if ui.button(t("Copy Row")).clicked() {
                            ui.ctx().copy_text(row_text.clone());
                            ui.close();
                        }
                    });
                }

                let response = row.response();
                if response.hovered() {
                    response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() || response.secondary_clicked() {
                    if let Ok(mut selected) = table_selected_row.lock() {
                        *selected = Some(row_key);
                    }
                }
            });
        });
}

fn registry_groups(table: &ResultTable, filter: &str) -> Vec<RegistryGroup> {
    let mut groups: Vec<RegistryGroup> = Vec::new();
    for row in filtered_table_rows(table, filter) {
        let hive = table_value(&table.headers, &row.cells, "hive")
            .and_then(registry_canonical_hive)
            .unwrap_or("-")
            .to_string();
        let path = table_value(&table.headers, &row.cells, "path")
            .unwrap_or("-")
            .to_string();
        let can_expand = registry_canonical_hive(&hive).is_some()
            && registry_row_is_key(&table.headers, &row.cells);

        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.hive == hive && group.path == path)
        {
            group.can_expand |= can_expand;
            group.rows.push(row);
        } else {
            groups.push(RegistryGroup {
                hive,
                path,
                can_expand,
                rows: vec![row],
            });
        }
    }
    groups
}

fn registry_tree(groups: &[RegistryGroup]) -> Vec<RegistryTreeRoot> {
    let mut roots: Vec<RegistryTreeRoot> = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        let root_index = match roots.iter().position(|root| root.hive == group.hive) {
            Some(index) => index,
            None => {
                roots.push(RegistryTreeRoot {
                    hive: group.hive.clone(),
                    group_index: None,
                    children: Vec::new(),
                });
                roots.len() - 1
            }
        };

        let parts = registry_path_parts(&group.path);
        if parts.is_empty() {
            roots[root_index].group_index = Some(group_index);
        } else {
            insert_registry_tree_node(&mut roots[root_index].children, &parts, group_index);
        }
    }
    roots
}

fn registry_tree_root_can_expand(groups: &[RegistryGroup], root: &RegistryTreeRoot) -> bool {
    !root.children.is_empty()
        || root
            .group_index
            .and_then(|index| groups.get(index))
            .map(|group| group.can_expand)
            .unwrap_or(false)
}

fn registry_tree_node_can_expand(groups: &[RegistryGroup], node: &RegistryTreeNode) -> bool {
    !node.children.is_empty()
        || node
            .group_index
            .and_then(|index| groups.get(index))
            .map(|group| group.can_expand)
            .unwrap_or(false)
}

fn insert_registry_tree_node(
    nodes: &mut Vec<RegistryTreeNode>,
    parts: &[&str],
    group_index: usize,
) {
    let Some((head, tail)) = parts.split_first() else {
        return;
    };
    let node_index = match nodes.iter().position(|node| node.name == *head) {
        Some(index) => index,
        None => {
            nodes.push(RegistryTreeNode {
                name: (*head).to_string(),
                group_index: None,
                children: Vec::new(),
            });
            nodes.len() - 1
        }
    };

    if tail.is_empty() {
        nodes[node_index].group_index = Some(group_index);
    } else {
        insert_registry_tree_node(&mut nodes[node_index].children, tail, group_index);
    }
}

fn registry_path_parts(path: &str) -> Vec<&str> {
    path.split(['\\', '/'])
        .filter(|part| !part.trim().is_empty() && *part != "-")
        .collect()
}

fn registry_path_leaf(path: &str) -> Option<&str> {
    registry_path_parts(path).last().copied()
}

fn registry_group_key(group: &RegistryGroup) -> String {
    format!("registry_group\t{}\t{}", group.hive, group.path)
}

fn registry_group_key_from_display_path(display_path: &str) -> Option<String> {
    let (hive, path) = registry_display_path_parts(display_path)?;
    Some(format!("registry_group\t{hive}\t{path}"))
}

fn registry_group_display_path(group: &RegistryGroup) -> String {
    registry_display_path(&group.hive, &group.path)
}

fn registry_display_path(hive: &str, path: &str) -> String {
    let hive = registry_display_hive(hive);
    let path = path.trim();
    if path.is_empty() || path == "-" {
        hive
    } else {
        format!("{hive}\\{path}")
    }
}

fn registry_display_hive(hive: &str) -> String {
    registry_canonical_hive(hive).unwrap_or(hive).to_string()
}

fn registry_display_path_parts(display_path: &str) -> Option<(&'static str, String)> {
    let (hive, path) = match display_path.split_once('\\') {
        Some((hive, path)) => (hive, path.trim()),
        None => (display_path, "-"),
    };
    let hive = registry_canonical_hive(hive)?;
    let path = if path.is_empty() { "-" } else { path };
    Some((hive, path.to_string()))
}

fn registry_canonical_hive(hive: &str) -> Option<&'static str> {
    match hive.trim().to_ascii_uppercase().as_str() {
        "HKCR" | "HKEY_CLASSES_ROOT" => Some("HKEY_CLASSES_ROOT"),
        "HKCU" | "HKEY_CURRENT_USER" => Some("HKEY_CURRENT_USER"),
        "HKLM" | "HKEY_LOCAL_MACHINE" => Some("HKEY_LOCAL_MACHINE"),
        "HKU" | "HKEY_USERS" => Some("HKEY_USERS"),
        "HKCC" | "HKEY_CURRENT_CONFIG" => Some("HKEY_CURRENT_CONFIG"),
        _ => None,
    }
}

fn queue_registry_display_path_select_and_request(
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    display_path: &str,
) {
    if let Some(group_key) = registry_group_key_from_display_path(display_path) {
        if let Ok(mut selected) = table_selected_row.lock() {
            *selected = Some(group_key);
        }
    }
    queue_registry_display_path_request(registry_key_requested, display_path);
}

fn queue_registry_display_path_request(
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    display_path: &str,
) {
    let Some((hive, path)) = registry_display_path_parts(display_path) else {
        return;
    };
    queue_registry_path_request(registry_key_requested, hive, &path);
}

fn queue_registry_path_request(
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    hive: &str,
    path: &str,
) {
    let Some(hive) = registry_canonical_hive(hive) else {
        return;
    };
    if let Ok(mut request) = registry_key_requested.lock() {
        *request = Some(registry_list_key_payload(hive, path));
    }
}

fn registry_list_key_payload(hive: &str, path: &str) -> String {
    let path = path.trim();
    let path = if path == "-" { "" } else { path };
    format!(
        "action=list_key\nhive_b64={}\npath_b64={}",
        STANDARD.encode(hive),
        STANDARD.encode(path)
    )
}

fn registry_row_is_key(headers: &[String], row: &[String]) -> bool {
    table_value(headers, row, "type")
        .map(|value| value.eq_ignore_ascii_case("key"))
        .unwrap_or(false)
}

fn registry_selected_group_index(groups: &[RegistryGroup], selected: Option<&str>) -> usize {
    if let Some(selected) = selected {
        if let Some(index) = groups
            .iter()
            .position(|group| registry_group_key(group) == selected)
        {
            return index;
        }
        if let Some(index) = groups
            .iter()
            .position(|group| group.rows.iter().any(|row| table_row_key(row) == selected))
        {
            return index;
        }
    }
    0
}

fn same_table_headers(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| normalized_table_header(left) == normalized_table_header(right))
}

fn serialize_result_table(section: &str, table: &ResultTable) -> String {
    let mut lines = Vec::with_capacity(table.rows.len() + 2);
    lines.push(format!("{section}:"));
    lines.push(table.headers.join("\t"));
    lines.extend(table.rows.iter().map(|row| row.join("\t")));
    lines.join("\n")
}

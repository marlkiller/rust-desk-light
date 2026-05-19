use super::{
    filtered_table_rows, normalized_table_header, parse_result_table, stable_hash,
    table_cell_label, table_row_key, table_value, DisplayTableRow, ResultTable,
    TABLE_BODY_CELL_HEIGHT, TABLE_BODY_TEXT_SIZE, TABLE_HEADER_CELL_HEIGHT, TABLE_HEADER_TEXT_SIZE,
};
use crate::i18n::t;
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
    );
}

struct RegistryGroup {
    hive: String,
    path: String,
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
) {
    let tree_width = (ui.available_width() * 0.34).clamp(260.0, 360.0);
    ui.horizontal_top(|ui| {
        render_registry_tree_panel(
            ui,
            groups,
            selected_group_index,
            table_selected_row,
            registry_key_requested,
            tree_width,
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
    width: f32,
) {
    egui::Frame::default()
        .fill(crate::theme::palette().panel_subtle)
        .stroke(egui::Stroke::new(1.0, crate::theme::palette().border))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 8))
        .show(ui, |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(t("Registry"))
                        .size(12.0)
                        .color(crate::theme::palette().text)
                        .strong(),
                );
                ui.add_space(6.0);
                egui::CollapsingHeader::new(
                    egui::RichText::new(t("Computer"))
                        .size(12.0)
                        .color(crate::theme::palette().text),
                )
                .id_salt("registry_tree_computer")
                .default_open(true)
                .show(ui, |ui| {
                    for root in registry_tree(groups) {
                        render_registry_tree_root(
                            ui,
                            groups,
                            &root,
                            selected_group_index,
                            table_selected_row,
                            registry_key_requested,
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
) {
    let response = egui::CollapsingHeader::new(
        egui::RichText::new(registry_display_hive(&root.hive))
            .size(12.0)
            .color(crate::theme::palette().text),
    )
    .id_salt(("registry_tree_root", &root.hive))
    .default_open(true)
    .show(ui, |ui| {
        for node in &root.children {
            render_registry_tree_node(
                ui,
                groups,
                node,
                selected_group_index,
                table_selected_row,
                registry_key_requested,
                &root.hive,
            );
        }
    });
    if response.header_response.clicked() {
        if let Some(group_index) = root.group_index.and_then(|index| groups.get(index)) {
            queue_registry_key_request(table_selected_row, registry_key_requested, group_index);
        } else {
            queue_registry_path_request(registry_key_requested, &root.hive, "-");
        }
    }
    response.header_response.context_menu(|ui| {
        if ui.button(t("Copy Path")).clicked() {
            ui.ctx().copy_text(registry_display_hive(&root.hive));
            ui.close();
        }
    });
}

fn render_registry_tree_node(
    ui: &mut egui::Ui,
    groups: &[RegistryGroup],
    node: &RegistryTreeNode,
    selected_group_index: usize,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    id_prefix: &str,
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
            );
        }
        return;
    }

    let id = format!("{id_prefix}\\{}", node.name);
    let response = egui::CollapsingHeader::new(
        egui::RichText::new(&node.name)
            .size(12.0)
            .color(crate::theme::palette().text),
    )
    .id_salt(("registry_tree_node", id.clone()))
    .default_open(true)
    .show(ui, |ui| {
        for child in &node.children {
            render_registry_tree_node(
                ui,
                groups,
                child,
                selected_group_index,
                table_selected_row,
                registry_key_requested,
                &id,
            );
        }
    });
    if response.header_response.clicked() {
        if let Some(group_index) = node.group_index.and_then(|index| groups.get(index)) {
            queue_registry_key_request(table_selected_row, registry_key_requested, group_index);
        }
    }
    response.header_response.context_menu(|ui| {
        if ui.button(t("Copy Path")).clicked() {
            ui.ctx()
                .copy_text(registry_display_path(id_prefix, &node.name));
            ui.close();
        }
    });
}

fn render_registry_tree_leaf(
    ui: &mut egui::Ui,
    groups: &[RegistryGroup],
    group_index: usize,
    selected_group_index: usize,
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
) {
    let Some(group) = groups.get(group_index) else {
        return;
    };
    let label = registry_path_leaf(&group.path)
        .map(str::to_string)
        .unwrap_or_else(|| registry_display_hive(&group.hive));
    let response = ui.selectable_label(
        selected_group_index == group_index,
        egui::RichText::new(&label)
            .size(12.0)
            .color(crate::theme::palette().text),
    );
    let response = response.on_hover_text(registry_display_path(&group.hive, &group.path));
    if response.clicked() {
        queue_registry_key_request(table_selected_row, registry_key_requested, group);
    }
    response.context_menu(|ui| {
        if ui.button(t("Copy Path")).clicked() {
            ui.ctx()
                .copy_text(registry_display_path(&group.hive, &group.path));
            ui.close();
        }
    });
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

        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.hive == hive && group.path == path)
        {
            group.rows.push(row);
        } else {
            groups.push(RegistryGroup {
                hive,
                path,
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

fn queue_registry_key_request(
    table_selected_row: &Arc<Mutex<Option<String>>>,
    registry_key_requested: &Arc<Mutex<Option<String>>>,
    group: &RegistryGroup,
) {
    if let Ok(mut selected) = table_selected_row.lock() {
        *selected = Some(registry_group_key(group));
    }
    queue_registry_path_request(registry_key_requested, &group.hive, &group.path);
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

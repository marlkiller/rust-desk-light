mod world_map_data;

use super::{ui, ClientRow};
use crate::windowing;
use eframe::egui;
use rdl_protocol::ClientInfo;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub(super) struct ClientMapWindow {
    open: bool,
    os_filter: String,
    close_requested: Arc<AtomicBool>,
}

impl ClientMapWindow {
    pub(super) fn new() -> Self {
        Self {
            open: false,
            os_filter: String::new(),
            close_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn open(&mut self) {
        self.open = true;
        self.close_requested.store(false, Ordering::Relaxed);
    }

    pub(super) fn render(
        &mut self,
        ctx: &egui::Context,
        clients: &[ClientRow],
        selected_client_id: &mut Option<String>,
        client_filter: &mut String,
    ) {
        if self.close_requested.swap(false, Ordering::Relaxed) {
            self.open = false;
        }
        if !self.open {
            return;
        }

        let close_requested = self.close_requested.clone();
        let selected_sink = Arc::new(Mutex::new(None::<String>));
        let selected_out = selected_sink.clone();
        let selected_current = selected_client_id.clone();
        let viewport_id = egui::ViewportId::from_hash_of("admin_client_map");
        let builder =
            windowing::child_viewport_builder("Client Map", [1040.0, 680.0], [760.0, 540.0]);

        ctx.show_viewport_immediate(viewport_id, builder, |ui, _class| {
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                close_requested.store(true, Ordering::Relaxed);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(ui::COLOR_BG).inner_margin(12.0))
                .show_inside(ui, |ui| {
                    windowing::render_child_window_controls(ui);
                    render_map_contents(
                        ui,
                        clients,
                        selected_current.as_deref(),
                        client_filter,
                        &mut self.os_filter,
                        &selected_sink,
                    );
                });
        });

        if let Some(client_id) = selected_out.lock().ok().and_then(|value| value.clone()) {
            *selected_client_id = Some(client_id);
        }
    }
}

pub(super) fn client_location_label(client: &ClientInfo) -> String {
    client
        .location
        .as_ref()
        .map(|location| {
            if location.accuracy_meters >= 1_000 {
                format!(
                    "{} ({}, ~{} km)",
                    location.label,
                    location.source,
                    location.accuracy_meters / 1_000
                )
            } else if location.accuracy_meters > 0 {
                format!(
                    "{} ({}, ~{} m)",
                    location.label, location.source, location.accuracy_meters
                )
            } else {
                format!("{} ({})", location.label, location.source)
            }
        })
        .unwrap_or_else(|| "-".to_string())
}

struct MapCluster {
    client_ids: Vec<String>,
    title: String,
    detail: String,
    pos: egui::Pos2,
}

const MAP_ASPECT_RATIO: f32 = 2.0;
const MAP_MIN_HEIGHT: f32 = 320.0;
const MAP_STATS_HEIGHT: f32 = 30.0;

fn render_map_contents(
    ui: &mut egui::Ui,
    clients: &[ClientRow],
    selected_client_id: Option<&str>,
    client_filter: &mut String,
    os_filter: &mut String,
    selected_sink: &Arc<Mutex<Option<String>>>,
) {
    ui::panel(ui, |ui| {
        ui.horizontal(|ui| {
            ui::section_title(ui, "Client Map");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("IP location is approximate")
                        .size(12.0)
                        .color(ui::COLOR_MUTED),
                );
            });
        });
        ui.add_space(8.0);
        let os_options = os_filter_options(clients);
        if !os_filter.is_empty() && !os_options.iter().any(|option| option == os_filter) {
            os_filter.clear();
        }
        render_map_filter_toolbar(ui, client_filter, os_filter, &os_options);
        ui.add_space(8.0);

        let clients = filtered_clients(clients, client_filter, os_filter);
        let located = clients
            .iter()
            .filter(|row| row.info.location.is_some())
            .count();
        if located == 0 {
            ui.add_space(18.0);
            ui.vertical_centered(|ui| {
                let (title, detail) = if clients.is_empty() {
                    (
                        "No matching clients",
                        "Clear or adjust the filter to show clients on the map.",
                    )
                } else {
                    (
                        "No geolocatable clients",
                        "GeoIP may be configured, but current clients have no public IP location. Local, LAN, VPN, proxy, and relay addresses cannot be placed on the map.",
                    )
                };
                ui.label(
                    egui::RichText::new(title)
                        .size(15.0)
                        .color(ui::COLOR_TEXT),
                );
                ui.label(
                    egui::RichText::new(detail)
                        .size(12.0)
                        .color(ui::COLOR_MUTED),
                );
                ui.label(
                    egui::RichText::new(
                        "If this is a public client, restart rdl-server-cli with --geoip-db /path/GeoLite2-City.mmdb.",
                    )
                        .size(12.0)
                        .color(ui::COLOR_MUTED),
                );
            });
            ui.add_space(18.0);
            return;
        }

        let map_size = world_map_size(ui.available_size());
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            render_map_stats_bar(ui, map_size.x, located, clients.len());
            ui.add_space(8.0);

            let (map_rect, _response) = ui.allocate_exact_size(map_size, egui::Sense::hover());
            let painter = ui.painter_at(map_rect);
            draw_world_map(&painter, map_rect);

            let clusters = map_clusters(&clients, map_rect);
            draw_map_summary(&painter, map_rect, located, clusters.len());
            for cluster in &clusters {
                let selected = cluster
                    .client_ids
                    .iter()
                    .any(|id| selected_client_id == Some(id.as_str()));
                draw_map_cluster(&painter, cluster, selected);
                draw_map_cluster_label(&painter, map_rect, cluster, selected);
            }

            let pointer_pos = ui.ctx().input(|input| input.pointer.hover_pos());
            if let Some((cluster, pointer)) = hovered_cluster(&clusters, map_rect, pointer_pos) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                draw_cluster_hover_card(&painter, map_rect, pointer, cluster);
                if ui.ctx().input(|input| input.pointer.primary_clicked()) {
                    if let Some(client_id) = cluster.client_ids.first() {
                        if let Ok(mut target) = selected_sink.lock() {
                            *target = Some(client_id.clone());
                        }
                    }
                }
            }
        });
    });
}

fn render_map_filter_toolbar(
    ui: &mut egui::Ui,
    client_filter: &mut String,
    os_filter: &mut String,
    os_options: &[String],
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = ui::TOOLBAR_CONTROL_HEIGHT;
        let combo_width = ui.available_width().min(210.0);
        let search_width = (ui.available_width() - combo_width - 8.0).max(180.0);
        ui.add_sized(
            [search_width, ui::TOOLBAR_CONTROL_HEIGHT],
            egui::TextEdit::singleline(client_filter)
                .hint_text("Search by id, fingerprint, host, user, or OS")
                .vertical_align(egui::Align::Center),
        );
        egui::ComboBox::from_id_salt("client_map_os_filter")
            .width(combo_width)
            .selected_text(os_filter_label(os_filter))
            .show_ui(ui, |ui| {
                ui.selectable_value(os_filter, String::new(), "All OS");
                if !os_options.is_empty() {
                    ui.separator();
                }
                for option in os_options {
                    ui.selectable_value(os_filter, option.clone(), option);
                }
            });
    });
}

fn filtered_clients(clients: &[ClientRow], filter: &str, os_filter: &str) -> Vec<ClientRow> {
    let filter = filter.trim().to_ascii_lowercase();
    let os_filter = os_filter.trim();
    clients
        .iter()
        .filter(|row| {
            if !os_filter.is_empty() && row.info.os.trim() != os_filter {
                return false;
            }
            if filter.is_empty() {
                return true;
            }
            row.info.id.to_ascii_lowercase().contains(&filter)
                || row.info.fingerprint.to_ascii_lowercase().contains(&filter)
                || row.info.hostname.to_ascii_lowercase().contains(&filter)
                || row.info.username.to_ascii_lowercase().contains(&filter)
                || row.info.os.to_ascii_lowercase().contains(&filter)
        })
        .cloned()
        .collect()
}

fn os_filter_options(clients: &[ClientRow]) -> Vec<String> {
    let mut options = clients
        .iter()
        .filter_map(|row| {
            let os = row.info.os.trim();
            (!os.is_empty()).then(|| os.to_string())
        })
        .collect::<Vec<_>>();
    options.sort_by_key(|value| value.to_ascii_lowercase());
    options.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    options
}

fn os_filter_label(os_filter: &str) -> String {
    if os_filter.trim().is_empty() {
        "OS: All".to_string()
    } else {
        format!("OS: {}", truncate_label(os_filter, 24))
    }
}

fn draw_world_map(painter: &egui::Painter, rect: egui::Rect) {
    draw_ocean(painter, rect);
    draw_graticule(painter, rect);
    draw_land_shapes(painter, rect);
    draw_map_labels(painter, rect);
    painter.rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(1.0, ui::COLOR_BORDER),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(1.0),
        7.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        ),
        egui::StrokeKind::Inside,
    );
}

fn world_map_size(available: egui::Vec2) -> egui::Vec2 {
    let available_height = (available.y - MAP_STATS_HEIGHT - 8.0).max(MAP_MIN_HEIGHT);
    let width = available
        .x
        .min(available_height * MAP_ASPECT_RATIO)
        .max(0.0);
    egui::vec2(width, width / MAP_ASPECT_RATIO)
}

fn render_map_stats_bar(
    ui: &mut egui::Ui,
    width: f32,
    located_count: usize,
    filtered_count: usize,
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, MAP_STATS_HEIGHT), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let chip_width = ((width - 8.0) / 2.0).min(220.0).max(0.0);
    let total_width = chip_width * 2.0 + 8.0;
    let left = rect.center().x - total_width / 2.0;
    let first = egui::Rect::from_min_size(
        egui::pos2(left, rect.top()),
        egui::vec2(chip_width, MAP_STATS_HEIGHT),
    );
    let second = first.translate(egui::vec2(chip_width + 8.0, 0.0));
    draw_stat_chip(&painter, first, "Located clients", located_count);
    draw_stat_chip(&painter, second, "Filtered clients", filtered_count);
}

fn draw_stat_chip(painter: &egui::Painter, rect: egui::Rect, label: &str, value: usize) {
    painter.rect_filled(
        rect,
        8.0,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180),
    );
    painter.rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(208, 218, 229, 180),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.left_center() + egui::vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.0),
        ui::COLOR_MUTED,
    );
    painter.text(
        rect.right_center() - egui::vec2(12.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        value.to_string(),
        egui::FontId::proportional(12.0),
        ui::COLOR_TEXT,
    );
}

fn draw_ocean(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 8.0, egui::Color32::from_rgb(226, 239, 249));
    let bands = [
        egui::Color32::from_rgba_unmultiplied(214, 231, 245, 120),
        egui::Color32::from_rgba_unmultiplied(236, 246, 251, 120),
        egui::Color32::from_rgba_unmultiplied(219, 235, 247, 120),
        egui::Color32::from_rgba_unmultiplied(241, 248, 252, 120),
    ];
    for index in 0..12 {
        let top = rect.top() + rect.height() * index as f32 / 12.0;
        let bottom = rect.top() + rect.height() * (index + 1) as f32 / 12.0;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), top),
                egui::pos2(rect.right(), bottom),
            ),
            0.0,
            bands[index % bands.len()],
        );
    }
}

fn draw_graticule(painter: &egui::Painter, rect: egui::Rect) {
    for lon in (-180..=180).step_by(30) {
        let x = map_project(rect, 0.0, lon as f64).x;
        let major = lon % 60 == 0;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            graticule_stroke(major),
        );
    }
    for lat in (-60..=60).step_by(15) {
        let y = map_project(rect, lat as f64, 0.0).y;
        let major = lat % 30 == 0;
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            graticule_stroke(major),
        );
    }

    let equator_y = map_project(rect, 0.0, 0.0).y;
    painter.line_segment(
        [
            egui::pos2(rect.left(), equator_y),
            egui::pos2(rect.right(), equator_y),
        ],
        egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(95, 132, 154, 80)),
    );

    for lon in (-120_i32..=120_i32).step_by(60) {
        let x = map_project(rect, 0.0, lon as f64).x;
        let label = if lon == 0 {
            "0".to_string()
        } else if lon < 0 {
            format!("{}W", lon.abs())
        } else {
            format!("{lon}E")
        };
        painter.text(
            egui::pos2(x, rect.bottom() - 8.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgba_unmultiplied(74, 92, 110, 120),
        );
    }
}

fn graticule_stroke(major: bool) -> egui::Stroke {
    if major {
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(112, 145, 168, 70),
        )
    } else {
        egui::Stroke::new(
            0.8,
            egui::Color32::from_rgba_unmultiplied(112, 145, 168, 38),
        )
    }
}

fn draw_land_shapes(painter: &egui::Painter, rect: egui::Rect) {
    draw_land_mesh(
        painter,
        rect.translate(egui::vec2(0.0, 1.6)),
        egui::Color32::from_rgba_unmultiplied(69, 88, 80, 32),
    );
    draw_land_mesh(painter, rect, egui::Color32::from_rgb(221, 231, 214));

    let coast_glow = egui::Stroke::new(
        2.2,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 95),
    );
    let coast = egui::Stroke::new(
        0.9,
        egui::Color32::from_rgba_unmultiplied(126, 151, 126, 170),
    );
    for polygon in world_map_data::LAND_POLYGONS {
        let points = projected_polygon_points(rect, polygon.points);
        painter.add(egui::Shape::closed_line(points.clone(), coast_glow));
        painter.add(egui::Shape::closed_line(points, coast));
    }
}

fn draw_land_mesh(painter: &egui::Painter, rect: egui::Rect, fill: egui::Color32) {
    let mut mesh = egui::Mesh::default();
    let point_count = world_map_data::LAND_POLYGONS
        .iter()
        .map(|polygon| polygon.points.len())
        .sum();
    let triangle_count = world_map_data::LAND_POLYGONS
        .iter()
        .map(|polygon| polygon.triangles.len())
        .sum();
    mesh.reserve_vertices(point_count);
    mesh.reserve_triangles(triangle_count);

    for polygon in world_map_data::LAND_POLYGONS {
        let base = mesh.vertices.len() as u32;
        for (lon, lat) in polygon.points {
            mesh.colored_vertex(map_project(rect, *lat as f64, *lon as f64), fill);
        }
        for [a, b, c] in polygon.triangles {
            mesh.add_triangle(base + *a as u32, base + *b as u32, base + *c as u32);
        }
    }

    painter.add(egui::Shape::mesh(mesh));
}

fn projected_polygon_points(rect: egui::Rect, polygon: &[(f32, f32)]) -> Vec<egui::Pos2> {
    polygon
        .iter()
        .map(|(lon, lat)| map_project(rect, *lat as f64, *lon as f64))
        .collect()
}

fn draw_map_labels(painter: &egui::Painter, rect: egui::Rect) {
    for label in MAP_LABELS {
        let pos = map_project(rect, label.latitude, label.longitude);
        painter.text(
            pos,
            egui::Align2::CENTER_CENTER,
            label.text,
            egui::FontId::proportional(label.size),
            egui::Color32::from_rgba_unmultiplied(76, 91, 77, label.alpha),
        );
    }
}

fn draw_map_summary(
    painter: &egui::Painter,
    rect: egui::Rect,
    located_count: usize,
    cluster_count: usize,
) {
    let summary = if located_count == cluster_count {
        format!("{located_count} locations")
    } else {
        format!("{located_count} clients / {cluster_count} clusters")
    };
    let badge_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(14.0, 14.0),
        egui::vec2(190.0, 34.0),
    );
    painter.rect_filled(
        badge_rect,
        8.0,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 218),
    );
    painter.rect_stroke(
        badge_rect,
        8.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(188, 202, 214, 165),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        summary,
        egui::FontId::proportional(12.0),
        ui::COLOR_TEXT,
    );
}

fn draw_map_cluster(painter: &egui::Painter, cluster: &MapCluster, selected: bool) {
    let count = cluster.client_ids.len();
    let radius = cluster_radius(cluster);
    let fill = if selected {
        ui::COLOR_ACCENT
    } else {
        ui::COLOR_GOOD
    };
    painter.circle_filled(
        cluster.pos,
        radius + 8.0,
        egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 32),
    );
    painter.circle_filled(
        cluster.pos + egui::vec2(0.0, 1.5),
        radius + 2.0,
        egui::Color32::from_rgba_unmultiplied(25, 36, 48, 45),
    );
    painter.circle_filled(cluster.pos, radius, fill);
    painter.circle_stroke(
        cluster.pos,
        radius,
        egui::Stroke::new(2.2, ui::COLOR_PANEL.gamma_multiply(0.98)),
    );
    if selected {
        painter.circle_stroke(
            cluster.pos,
            radius + 4.0,
            egui::Stroke::new(2.0, ui::COLOR_ACCENT.gamma_multiply(0.55)),
        );
    }
    if count > 1 {
        painter.text(
            cluster.pos,
            egui::Align2::CENTER_CENTER,
            count.to_string(),
            egui::FontId::proportional(11.0),
            ui::COLOR_PANEL,
        );
    }
}

fn draw_map_cluster_label(
    painter: &egui::Painter,
    map_rect: egui::Rect,
    cluster: &MapCluster,
    selected: bool,
) {
    let rect = cluster_label_rect(map_rect, cluster);
    let fill = if selected {
        egui::Color32::from_rgba_unmultiplied(229, 239, 253, 235)
    } else {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220)
    };
    let stroke = if selected {
        egui::Stroke::new(1.0, ui::COLOR_ACCENT.gamma_multiply(0.55))
    } else {
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(188, 202, 214, 165),
        )
    };

    painter.rect_filled(rect, 7.0, fill);
    painter.rect_stroke(rect, 7.0, stroke, egui::StrokeKind::Inside);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        map_cluster_label(cluster),
        egui::FontId::proportional(11.0),
        ui::COLOR_TEXT,
    );
}

fn cluster_hit_rect(map_rect: egui::Rect, cluster: &MapCluster) -> egui::Rect {
    let radius = cluster_radius(cluster) + 9.0;
    let marker_rect = egui::Rect::from_center_size(cluster.pos, egui::Vec2::splat(radius * 2.0));
    marker_rect
        .union(cluster_label_rect(map_rect, cluster))
        .expand(6.0)
        .intersect(map_rect)
}

fn hovered_cluster(
    clusters: &[MapCluster],
    map_rect: egui::Rect,
    pointer: Option<egui::Pos2>,
) -> Option<(&MapCluster, egui::Pos2)> {
    let pointer = pointer?;
    clusters
        .iter()
        .filter(|cluster| cluster_hit_rect(map_rect, cluster).contains(pointer))
        .min_by(|left, right| {
            left.pos
                .distance(pointer)
                .partial_cmp(&right.pos.distance(pointer))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|cluster| (cluster, pointer))
}

fn draw_cluster_hover_card(
    painter: &egui::Painter,
    map_rect: egui::Rect,
    pointer: egui::Pos2,
    cluster: &MapCluster,
) {
    let detail_lines = cluster.detail.lines().collect::<Vec<_>>();
    let max_chars = detail_lines
        .iter()
        .map(|line| line.chars().count())
        .chain(std::iter::once(cluster.title.chars().count()))
        .max()
        .unwrap_or(0);
    let width = (max_chars as f32 * 7.2 + 28.0)
        .clamp(250.0, 440.0)
        .min(map_rect.width() - 16.0);
    let mut height: f32 = 42.0;
    for line in &detail_lines {
        height += if line.is_empty() { 8.0 } else { 17.0 };
    }
    height = height.min(map_rect.height() - 16.0);

    let size = egui::vec2(width.max(120.0), height.max(74.0));
    let mut min = pointer + egui::vec2(16.0, 16.0);
    if min.x + size.x > map_rect.right() - 8.0 {
        min.x = pointer.x - size.x - 16.0;
    }
    if min.y + size.y > map_rect.bottom() - 8.0 {
        min.y = pointer.y - size.y - 16.0;
    }
    min.x = min
        .x
        .clamp(map_rect.left() + 8.0, map_rect.right() - size.x - 8.0);
    min.y = min
        .y
        .clamp(map_rect.top() + 8.0, map_rect.bottom() - size.y - 8.0);

    let rect = egui::Rect::from_min_size(min, size);
    painter.rect_filled(
        rect.translate(egui::vec2(0.0, 1.5)),
        9.0,
        egui::Color32::from_rgba_unmultiplied(19, 30, 42, 45),
    );
    painter.rect_filled(
        rect,
        9.0,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 242),
    );
    painter.rect_stroke(
        rect,
        9.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(172, 190, 208, 210),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.left_top() + egui::vec2(13.0, 11.0),
        egui::Align2::LEFT_TOP,
        &cluster.title,
        egui::FontId::proportional(12.0),
        ui::COLOR_TEXT,
    );

    let mut y = rect.top() + 34.0;
    for line in detail_lines {
        if line.is_empty() {
            y += 8.0;
            continue;
        }
        painter.text(
            egui::pos2(rect.left() + 13.0, y),
            egui::Align2::LEFT_TOP,
            line,
            egui::FontId::monospace(11.0),
            ui::COLOR_MUTED,
        );
        y += 17.0;
    }
}

fn cluster_label_rect(map_rect: egui::Rect, cluster: &MapCluster) -> egui::Rect {
    let label = map_cluster_label(cluster);
    let label_width = (label.chars().count() as f32 * 7.0 + 18.0).clamp(62.0, 190.0);
    let label_size = egui::vec2(label_width, 22.0);
    let mut min = cluster.pos + egui::vec2(13.0, -30.0);
    if min.x + label_size.x > map_rect.right() - 6.0 {
        min.x = cluster.pos.x - label_size.x - 13.0;
    }
    if min.x < map_rect.left() + 6.0 {
        min.x = map_rect.left() + 6.0;
    }
    if min.y < map_rect.top() + 6.0 {
        min.y = cluster.pos.y + 14.0;
    }
    if min.y + label_size.y > map_rect.bottom() - 6.0 {
        min.y = map_rect.bottom() - label_size.y - 6.0;
    }
    egui::Rect::from_min_size(min, label_size)
}

fn cluster_radius(cluster: &MapCluster) -> f32 {
    if cluster.client_ids.len() > 1 {
        14.0
    } else {
        8.5
    }
}

fn map_cluster_label(cluster: &MapCluster) -> String {
    if cluster.client_ids.len() > 1 {
        format!("{} clients", cluster.client_ids.len())
    } else {
        truncate_label(&cluster.title, 22)
    }
}

fn truncate_label(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut label = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        label.push_str("...");
    }
    label
}

fn map_clusters(clients: &[ClientRow], rect: egui::Rect) -> Vec<MapCluster> {
    let mut clusters = Vec::<MapCluster>::new();
    for row in clients {
        let Some(location) = row.info.location.as_ref() else {
            continue;
        };
        let lat = location.latitude().clamp(-90.0, 90.0);
        let lon = location.longitude().clamp(-180.0, 180.0);
        let pos = map_project(rect, lat, lon);
        let title = map_client_title(row);
        let detail = map_point_detail(row);

        if let Some(cluster) = clusters
            .iter_mut()
            .find(|cluster| cluster.pos.distance(pos) <= 18.0)
        {
            let count = cluster.client_ids.len() as f32;
            cluster.pos = egui::pos2(
                (cluster.pos.x * count + pos.x) / (count + 1.0),
                (cluster.pos.y * count + pos.y) / (count + 1.0),
            );
            cluster.client_ids.push(row.info.id.clone());
            cluster.title = format!("{} clients", cluster.client_ids.len());
            cluster.detail.push('\n');
            cluster.detail.push('\n');
            cluster.detail.push_str(&detail);
        } else {
            clusters.push(MapCluster {
                client_ids: vec![row.info.id.clone()],
                title,
                detail,
                pos,
            });
        }
    }
    clusters
}

fn map_client_title(row: &ClientRow) -> String {
    let hostname = row.info.hostname.trim();
    if !hostname.is_empty() {
        return hostname.to_string();
    }

    let peer_ip = client_peer_ip(&row.info.peer_addr);
    if peer_ip != "-" {
        return peer_ip;
    }

    ui::compact_id(&row.info.id)
}

fn map_point_detail(row: &ClientRow) -> String {
    let location = row
        .info
        .location
        .as_ref()
        .map(|location| {
            format!(
                "{} ({}, {})",
                location.label,
                location.source,
                map_accuracy(location.accuracy_meters)
            )
        })
        .unwrap_or_else(|| "-".to_string());

    format!(
        "id: {}\nip: {}\nhost: {}\nuser: {}\nos: {}\nlocation: {}",
        ui::compact_id(&row.info.id),
        client_peer_ip(&row.info.peer_addr),
        display_value(&row.info.hostname),
        display_value(&row.info.username),
        display_value(&row.info.os),
        location
    )
}

fn map_accuracy(accuracy_meters: u32) -> String {
    if accuracy_meters == 0 {
        "unknown accuracy".to_string()
    } else if accuracy_meters >= 1_000 {
        format!("~{} km", accuracy_meters / 1_000)
    } else {
        format!("~{} m", accuracy_meters)
    }
}

fn client_peer_ip(peer_addr: &str) -> String {
    let peer_addr = peer_addr.trim();
    if peer_addr.is_empty() {
        return "-".to_string();
    }
    peer_addr
        .parse::<std::net::SocketAddr>()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| peer_addr.to_string())
}

fn display_value(value: &str) -> &str {
    let value = value.trim();
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

fn map_project(rect: egui::Rect, latitude: f64, longitude: f64) -> egui::Pos2 {
    let x = rect.left() + (((longitude + 180.0) / 360.0) as f32 * rect.width());
    let y = rect.top() + (((90.0 - latitude) / 180.0) as f32 * rect.height());
    egui::pos2(x, y)
}

struct MapLabel {
    text: &'static str,
    latitude: f64,
    longitude: f64,
    size: f32,
    alpha: u8,
}

const MAP_LABELS: &[MapLabel] = &[
    MapLabel {
        text: "North America",
        latitude: 47.0,
        longitude: -108.0,
        size: 11.0,
        alpha: 92,
    },
    MapLabel {
        text: "South America",
        latitude: -19.0,
        longitude: -62.0,
        size: 11.0,
        alpha: 88,
    },
    MapLabel {
        text: "Europe",
        latitude: 51.0,
        longitude: 17.0,
        size: 10.5,
        alpha: 84,
    },
    MapLabel {
        text: "Africa",
        latitude: 7.0,
        longitude: 21.0,
        size: 11.0,
        alpha: 90,
    },
    MapLabel {
        text: "Asia",
        latitude: 42.0,
        longitude: 88.0,
        size: 11.0,
        alpha: 92,
    },
    MapLabel {
        text: "Oceania",
        latitude: -27.0,
        longitude: 136.0,
        size: 10.5,
        alpha: 82,
    },
];

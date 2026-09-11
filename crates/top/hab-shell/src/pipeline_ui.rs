//! ReactFlow-style native pipeline canvas used by the Hab shell.
//!
//! Rerun remains the data/visualization substrate. The pipeline editor is a
//! product control surface, so it is painted directly in egui and sourced from
//! the same YAML file that starts the C++ engine.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rerun::external::egui;
use serde_yaml::{Mapping, Value};

const NODE_WIDTH: f32 = 226.0;
const TITLE_HEIGHT: f32 = 60.0;
const PORT_HEIGHT: f32 = 23.0;
const NODE_FOOTER: f32 = 18.0;
const GRID_SIZE: f32 = 24.0;

const TEXT: egui::Color32 = egui::Color32::from_rgb(0x11, 0x11, 0x14);
const MUTED: egui::Color32 = egui::Color32::from_rgb(0x68, 0x68, 0x78);
const TERTIARY: egui::Color32 = egui::Color32::from_rgb(0x91, 0x91, 0x9f);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0xdf, 0xdf, 0xe7);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x6d, 0x28, 0xd9);
const SOURCE: egui::Color32 = egui::Color32::from_rgb(0x1f, 0x77, 0xb4);
const MODEL: egui::Color32 = egui::Color32::from_rgb(0xb5, 0x50, 0xd8);
const SINK: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x8b, 0x57);
const STREAM: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xb8, 0xc6);
const CONFIG: egui::Color32 = egui::Color32::from_rgb(0x8b, 0x5c, 0xf6);
const SERVICE: egui::Color32 = egui::Color32::from_rgb(0xe0, 0x7a, 0x3f);
const LIVE_ACTIVITY_WINDOW: Duration = Duration::from_millis(1_650);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Source,
    Transform,
    Model,
    Sink,
    Config,
    Service,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            Self::Source => "SOURCE",
            Self::Transform => "TRANSFORM",
            Self::Model => "MODEL",
            Self::Sink => "SINK",
            Self::Config => "CONFIG",
            Self::Service => "SERVICE",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Source => SOURCE,
            Self::Transform => ACCENT,
            Self::Model => MODEL,
            Self::Sink => SINK,
            Self::Config => CONFIG,
            Self::Service => SERVICE,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Source => "▲",
            Self::Transform => "●",
            Self::Model => "◆",
            Self::Sink => "■",
            Self::Config => "C",
            Self::Service => "S",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PipelineParameter {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct PipelineNode {
    pub id: String,
    pub transform_type: String,
    pub language: String,
    pub kind: NodeKind,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub parameters: Vec<PipelineParameter>,
    pub graph_only: bool,
    position: egui::Pos2,
}

impl PipelineNode {
    fn height(&self) -> f32 {
        TITLE_HEIGHT
            + self.inputs.len().max(self.outputs.len()).max(1) as f32 * PORT_HEIGHT
            + NODE_FOOTER
    }
}

#[derive(Clone, Debug)]
struct PipelineEdge {
    from: usize,
    from_port: String,
    to: usize,
    to_port: String,
    stream_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StreamActivitySnapshot {
    pub name: String,
    pub frequency_hz: f64,
    pub input_lag_ms: f64,
    pub batch_size: f64,
    pub total_samples: u64,
}

#[derive(Clone, Debug)]
struct EdgeActivity {
    frequency_hz: f64,
    input_lag_ms: f64,
    batch_size: f64,
    total_samples: u64,
    last_advanced_at: Option<Instant>,
    last_seen_at: Instant,
}

impl EdgeActivity {
    fn is_live_at(&self, now: Instant) -> bool {
        self.frequency_hz > 0.05
            && self.last_advanced_at.is_some_and(|advanced| {
                now.saturating_duration_since(advanced) <= LIVE_ACTIVITY_WINDOW
            })
    }
}

pub struct PipelineUi {
    pub name: String,
    pub path: PathBuf,
    pub nodes: Vec<PipelineNode>,
    edges: Vec<PipelineEdge>,
    pub selected: Option<usize>,
    zoom: f32,
    pan: egui::Vec2,
    fit_pending: bool,
    stream_activity: HashMap<String, EdgeActivity>,
    pub error: Option<String>,
}

impl PipelineUi {
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match parse_pipeline(&path) {
            Ok((name, mut nodes, edges)) => {
                layout_nodes(&mut nodes, &edges);
                Self {
                    name,
                    path,
                    nodes,
                    edges,
                    selected: None,
                    zoom: 1.0,
                    pan: egui::Vec2::ZERO,
                    fit_pending: true,
                    stream_activity: HashMap::new(),
                    error: None,
                }
            }
            Err(error) => Self {
                name: "Pipeline".to_owned(),
                path,
                nodes: Vec::new(),
                edges: Vec::new(),
                selected: None,
                zoom: 1.0,
                pan: egui::Vec2::ZERO,
                fit_pending: false,
                stream_activity: HashMap::new(),
                error: Some(error),
            },
        }
    }

    pub fn reload(&mut self, path: impl Into<PathBuf>) {
        *self = Self::load(path);
    }

    pub fn request_fit(&mut self) {
        self.fit_pending = true;
    }

    pub fn update_stream_activity(&mut self, snapshots: &[StreamActivitySnapshot]) {
        self.update_stream_activity_at(snapshots, Instant::now());
    }

    fn update_stream_activity_at(&mut self, snapshots: &[StreamActivitySnapshot], now: Instant) {
        for snapshot in snapshots {
            let activity = self
                .stream_activity
                .entry(snapshot.name.clone())
                .or_insert_with(|| EdgeActivity {
                    frequency_hz: 0.0,
                    input_lag_ms: 0.0,
                    batch_size: 0.0,
                    total_samples: 0,
                    last_advanced_at: None,
                    last_seen_at: now,
                });
            if snapshot.total_samples != activity.total_samples && snapshot.total_samples > 0 {
                activity.last_advanced_at = Some(now);
            }
            activity.frequency_hz = snapshot.frequency_hz;
            activity.input_lag_ms = snapshot.input_lag_ms;
            activity.batch_size = snapshot.batch_size;
            activity.total_samples = snapshot.total_samples;
            activity.last_seen_at = now;
        }
        self.stream_activity.retain(|_, activity| {
            now.saturating_duration_since(activity.last_seen_at) < Duration::from_secs(30)
        });
    }

    pub fn live_stream_count(&self) -> usize {
        self.live_stream_count_at(Instant::now())
    }

    fn live_stream_count_at(&self, now: Instant) -> usize {
        self.edges
            .iter()
            .filter_map(|edge| edge.stream_id.as_deref())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|stream_id| {
                self.stream_activity
                    .get(*stream_id)
                    .is_some_and(|activity| activity.is_live_at(now))
            })
            .count()
    }

    pub fn has_live_activity(&self) -> bool {
        self.live_stream_count() > 0
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size().max(egui::vec2(320.0, 320.0));
        let (background, painter) = ui.allocate_painter(available, egui::Sense::click_and_drag());
        let canvas = background.rect;
        painter.rect_filled(canvas, 0.0, egui::Color32::from_rgb(0xfc, 0xfc, 0xfd));

        if self.fit_pending {
            self.fit(canvas);
            self.fit_pending = false;
        }

        if background.dragged_by(egui::PointerButton::Middle)
            || background.dragged_by(egui::PointerButton::Secondary)
        {
            self.pan += ui.input(|input| input.pointer.delta());
        }
        if background.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > f32::EPSILON {
                let old_zoom = self.zoom;
                self.zoom = (self.zoom * (scroll * 0.0015).exp()).clamp(0.35, 1.8);
                if let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) {
                    let local = pointer - canvas.min - self.pan;
                    self.pan += local * (1.0 - self.zoom / old_zoom);
                }
            }
        }

        let pointer = ui.input(|input| input.pointer.hover_pos());
        let animation_seconds = ui.input(|input| input.time);
        let now = Instant::now();
        self.draw_grid(&painter, canvas);
        let hovered_edge = self.draw_edges(
            &painter,
            canvas,
            pointer.filter(|point| canvas.contains(*point)),
            now,
            animation_seconds,
        );

        let mut hovered_node = false;
        for index in 0..self.nodes.len() {
            let rect = self.node_rect(index, canvas);
            let response = ui.interact(
                rect,
                ui.id().with(("hab_pipeline_node", &self.nodes[index].id)),
                egui::Sense::click_and_drag(),
            );
            hovered_node |= response.hovered();
            if response.clicked() {
                self.selected = Some(index);
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                let delta = ui.input(|input| input.pointer.delta()) / self.zoom;
                self.nodes[index].position += delta;
            }
            self.draw_node(
                &painter,
                index,
                rect,
                self.selected == Some(index),
                response.hovered(),
                pointer,
            );
        }

        if background.clicked_by(egui::PointerButton::Primary) && !hovered_node {
            self.selected = None;
        }

        if let (Some(pointer), Some(edge_index)) = (pointer, hovered_edge) {
            self.draw_edge_tooltip(&painter, canvas, pointer, &self.edges[edge_index], now);
        }

        painter.text(
            canvas.left_bottom() + egui::vec2(14.0, -14.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "{} nodes  ·  {} edges  ·  {:.0}%",
                self.nodes.len(),
                self.edges.len(),
                self.zoom * 100.0
            ),
            egui::FontId::monospace(10.0),
            TERTIARY,
        );
    }

    fn fit(&mut self, canvas: egui::Rect) {
        if self.nodes.is_empty() {
            self.zoom = 1.0;
            self.pan = egui::Vec2::ZERO;
            return;
        }
        let min = self.nodes.iter().map(|node| node.position).fold(
            egui::Pos2::new(f32::INFINITY, f32::INFINITY),
            |acc, value| egui::pos2(acc.x.min(value.x), acc.y.min(value.y)),
        );
        let max = self.nodes.iter().fold(
            egui::Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY),
            |acc, node| {
                egui::pos2(
                    acc.x.max(node.position.x + NODE_WIDTH),
                    acc.y.max(node.position.y + node.height()),
                )
            },
        );
        let graph_size = (max - min).max(egui::vec2(1.0, 1.0));
        self.zoom = ((canvas.width() - 100.0) / graph_size.x)
            .min((canvas.height() - 90.0) / graph_size.y)
            .clamp(0.38, 1.15);
        let graph_center = min.to_vec2() + graph_size * 0.5;
        self.pan = canvas.size() * 0.5 - graph_center * self.zoom;
    }

    fn node_rect(&self, index: usize, canvas: egui::Rect) -> egui::Rect {
        let node = &self.nodes[index];
        let min = canvas.min + self.pan + node.position.to_vec2() * self.zoom;
        egui::Rect::from_min_size(min, egui::vec2(NODE_WIDTH, node.height()) * self.zoom)
    }

    fn input_port_position(&self, node_index: usize, port: &str, canvas: egui::Rect) -> egui::Pos2 {
        let node = &self.nodes[node_index];
        let index = node
            .inputs
            .iter()
            .position(|candidate| candidate == port)
            .unwrap_or(0);
        let rect = self.node_rect(node_index, canvas);
        egui::pos2(
            rect.left(),
            rect.top() + (TITLE_HEIGHT + PORT_HEIGHT * (index as f32 + 0.5)) * self.zoom,
        )
    }

    fn output_port_position(
        &self,
        node_index: usize,
        port: &str,
        canvas: egui::Rect,
    ) -> egui::Pos2 {
        let node = &self.nodes[node_index];
        let index = node
            .outputs
            .iter()
            .position(|candidate| candidate == port)
            .unwrap_or(0);
        let rect = self.node_rect(node_index, canvas);
        egui::pos2(
            rect.right(),
            rect.top() + (TITLE_HEIGHT + PORT_HEIGHT * (index as f32 + 0.5)) * self.zoom,
        )
    }

    fn draw_grid(&self, painter: &egui::Painter, canvas: egui::Rect) {
        let spacing = GRID_SIZE * self.zoom;
        if spacing < 8.0 {
            return;
        }
        let start_x = canvas.left() + self.pan.x.rem_euclid(spacing);
        let start_y = canvas.top() + self.pan.y.rem_euclid(spacing);
        let color = egui::Color32::from_gray(225);
        let mut x = start_x;
        while x < canvas.right() {
            let mut y = start_y;
            while y < canvas.bottom() {
                painter.circle_filled(egui::pos2(x, y), 0.8, color);
                y += spacing;
            }
            x += spacing;
        }
    }

    fn draw_edges(
        &self,
        painter: &egui::Painter,
        canvas: egui::Rect,
        pointer: Option<egui::Pos2>,
        now: Instant,
        animation_seconds: f64,
    ) -> Option<usize> {
        let mut hovered_edge: Option<(f32, usize)> = None;
        for (edge_index, edge) in self.edges.iter().enumerate() {
            let from = self.output_port_position(edge.from, &edge.from_port, canvas);
            let to = self.input_port_position(edge.to, &edge.to_port, canvas);
            let control = ((to.x - from.x).abs() * 0.48).max(55.0 * self.zoom);
            let points = [
                from,
                from + egui::vec2(control, 0.0),
                to - egui::vec2(control, 0.0),
                to,
            ];

            let activity = edge
                .stream_id
                .as_deref()
                .and_then(|stream_id| self.stream_activity.get(stream_id));
            let live = activity.is_some_and(|activity| activity.is_live_at(now));
            let color = if live {
                STREAM
            } else if edge.stream_id.is_none() {
                CONFIG.gamma_multiply(0.52)
            } else if activity.is_some() {
                TERTIARY.gamma_multiply(0.72)
            } else {
                STREAM.gamma_multiply(0.34)
            };

            if live {
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    points,
                    false,
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::new(6.0 * self.zoom, STREAM.gamma_multiply(0.14)),
                ));
            }
            if edge.stream_id.is_none() {
                draw_dashed_bezier(
                    painter,
                    points,
                    egui::Stroke::new((1.35 * self.zoom).max(1.0), color),
                );
            } else {
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    points,
                    false,
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::new((1.5 * self.zoom).max(1.0), color),
                ));
            }

            if let Some(activity) = activity.filter(|activity| activity.is_live_at(now)) {
                let dot_count = ((activity.frequency_hz.max(1.0).log10() * 2.2).ceil() as usize
                    + 1)
                .clamp(1, 6);
                let speed = (0.15 + activity.frequency_hz.max(1.0).ln() * 0.055).clamp(0.15, 0.82);
                for dot in 0..dot_count {
                    let phase = (animation_seconds * speed + dot as f64 / dot_count as f64)
                        .rem_euclid(1.0) as f32;
                    let position = cubic_bezier_point(points, phase);
                    painter.circle_filled(
                        position,
                        (5.2 * self.zoom).max(2.8),
                        STREAM.gamma_multiply(0.16),
                    );
                    painter.circle_filled(
                        position,
                        (2.7 * self.zoom).max(1.8),
                        egui::Color32::WHITE,
                    );
                    painter.circle_stroke(
                        position,
                        (2.7 * self.zoom).max(1.8),
                        egui::Stroke::new(1.0, STREAM),
                    );
                }
            }

            if let Some(pointer) = pointer {
                let distance = distance_to_bezier(pointer, points);
                if distance <= (8.0 * self.zoom).max(6.0)
                    && hovered_edge.is_none_or(|(best, _)| distance < best)
                {
                    hovered_edge = Some((distance, edge_index));
                }
            }
        }

        hovered_edge.map(|(_, edge_index)| edge_index)
    }

    fn draw_edge_tooltip(
        &self,
        painter: &egui::Painter,
        canvas: egui::Rect,
        pointer: egui::Pos2,
        edge: &PipelineEdge,
        now: Instant,
    ) {
        let from = &self.nodes[edge.from];
        let to = &self.nodes[edge.to];
        let route = format!(
            "{}.{}  ->  {}.{}",
            from.id, edge.from_port, to.id, edge.to_port
        );
        let details = if let Some(stream_id) = &edge.stream_id {
            if let Some(activity) = self.stream_activity.get(stream_id) {
                let state = if activity.is_live_at(now) {
                    "LIVE"
                } else {
                    "STALLED"
                };
                format!(
                    "{state}  |  {stream_id}\n{route}\n{:.1} Hz  |  {} samples  |  {:.1} ms lag  |  {:.1} avg batch",
                    activity.frequency_hz,
                    activity.total_samples,
                    activity.input_lag_ms,
                    activity.batch_size,
                )
            } else {
                format!(
                    "NO TELEMETRY  |  {stream_id}\n{route}\nWaiting for pipeline-monitor counters"
                )
            }
        } else {
            format!("CONFIGURED HANDOFF\n{route}\nOperator-triggered path; no live sample counter")
        };
        let galley = painter.layout(details, egui::FontId::monospace(10.0), TEXT, 350.0);
        let size = galley.size() + egui::vec2(22.0, 18.0);
        let mut min = pointer + egui::vec2(14.0, 14.0);
        min.x = min
            .x
            .min(canvas.right() - size.x - 8.0)
            .max(canvas.left() + 8.0);
        min.y = min
            .y
            .min(canvas.bottom() - size.y - 8.0)
            .max(canvas.top() + 8.0);
        let rect = egui::Rect::from_min_size(min, size);
        painter.rect_filled(rect, 7.0, egui::Color32::from_rgb(0xf8, 0xf8, 0xfc));
        painter.rect_stroke(
            rect,
            7.0,
            egui::Stroke::new(1.0, BORDER),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.min + egui::vec2(11.0, 9.0), galley, TEXT);
    }

    fn draw_node(
        &self,
        painter: &egui::Painter,
        index: usize,
        rect: egui::Rect,
        selected: bool,
        hovered: bool,
        pointer: Option<egui::Pos2>,
    ) {
        let node = &self.nodes[index];
        let radius = (7.0 * self.zoom).clamp(3.0, 9.0);
        painter.rect_filled(
            rect.translate(egui::vec2(0.0, 3.0 * self.zoom)),
            radius,
            egui::Color32::from_black_alpha(if hovered { 28 } else { 18 }),
        );
        painter.rect_filled(rect, radius, egui::Color32::WHITE);
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected { node.kind.color() } else { BORDER },
            ),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 3.0 * self.zoom)),
            radius,
            node.kind.color(),
        );

        let font_scale = self.zoom.clamp(0.65, 1.15);
        painter.text(
            rect.left_top() + egui::vec2(14.0, 13.0) * self.zoom,
            egui::Align2::LEFT_TOP,
            format!("{}  {}", node.kind.glyph(), node.kind.label()),
            egui::FontId::monospace(8.5 * font_scale),
            node.kind.color(),
        );
        painter.text(
            rect.left_top() + egui::vec2(14.0, 31.0) * self.zoom,
            egui::Align2::LEFT_TOP,
            &node.id,
            egui::FontId::proportional(13.5 * font_scale),
            TEXT,
        );
        painter.text(
            rect.right_top() + egui::vec2(-12.0, 33.0) * self.zoom,
            egui::Align2::RIGHT_TOP,
            &node.language,
            egui::FontId::monospace(8.0 * font_scale),
            TERTIARY,
        );
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.top() + TITLE_HEIGHT * self.zoom),
                egui::pos2(rect.right(), rect.top() + TITLE_HEIGHT * self.zoom),
            ],
            egui::Stroke::new(1.0, BORDER),
        );

        for (port_index, port) in node.inputs.iter().enumerate() {
            let center = egui::pos2(
                rect.left(),
                rect.top() + (TITLE_HEIGHT + PORT_HEIGHT * (port_index as f32 + 0.5)) * self.zoom,
            );
            let port_hovered = pointer.is_some_and(|point| point.distance(center) < 10.0);
            painter.circle_filled(
                center,
                if port_hovered { 5.5 } else { 4.2 } * self.zoom,
                if node.graph_only {
                    node.kind.color()
                } else {
                    STREAM
                },
            );
            painter.text(
                center + egui::vec2(12.0, 0.0) * self.zoom,
                egui::Align2::LEFT_CENTER,
                port,
                egui::FontId::monospace(8.5 * font_scale),
                MUTED,
            );
        }
        for (port_index, port) in node.outputs.iter().enumerate() {
            let center = egui::pos2(
                rect.right(),
                rect.top() + (TITLE_HEIGHT + PORT_HEIGHT * (port_index as f32 + 0.5)) * self.zoom,
            );
            let port_hovered = pointer.is_some_and(|point| point.distance(center) < 10.0);
            painter.circle_filled(
                center,
                if port_hovered { 5.5 } else { 4.2 } * self.zoom,
                if node.graph_only {
                    node.kind.color()
                } else {
                    STREAM
                },
            );
            painter.text(
                center - egui::vec2(12.0, 0.0) * self.zoom,
                egui::Align2::RIGHT_CENTER,
                port,
                egui::FontId::monospace(8.5 * font_scale),
                MUTED,
            );
        }
        painter.text(
            rect.left_bottom() + egui::vec2(13.0, -7.0) * self.zoom,
            egui::Align2::LEFT_BOTTOM,
            &node.transform_type,
            egui::FontId::monospace(8.0 * font_scale),
            TERTIARY,
        );
    }
}

fn cubic_bezier_point(points: [egui::Pos2; 4], t: f32) -> egui::Pos2 {
    let one_minus_t = 1.0 - t;
    let value = points[0].to_vec2() * one_minus_t.powi(3)
        + points[1].to_vec2() * (3.0 * one_minus_t.powi(2) * t)
        + points[2].to_vec2() * (3.0 * one_minus_t * t.powi(2))
        + points[3].to_vec2() * t.powi(3);
    egui::pos2(value.x, value.y)
}

fn draw_dashed_bezier(painter: &egui::Painter, points: [egui::Pos2; 4], stroke: egui::Stroke) {
    const SEGMENTS: usize = 48;
    for segment in 0..SEGMENTS {
        if segment % 4 >= 2 {
            continue;
        }
        let start = cubic_bezier_point(points, segment as f32 / SEGMENTS as f32);
        let end = cubic_bezier_point(points, (segment + 1) as f32 / SEGMENTS as f32);
        painter.line_segment([start, end], stroke);
    }
}

fn distance_to_bezier(point: egui::Pos2, points: [egui::Pos2; 4]) -> f32 {
    const SEGMENTS: usize = 36;
    let mut distance = f32::INFINITY;
    let mut previous = points[0];
    for segment in 1..=SEGMENTS {
        let current = cubic_bezier_point(points, segment as f32 / SEGMENTS as f32);
        distance = distance.min(distance_to_segment(point, previous, current));
        previous = current;
    }
    distance
}

fn distance_to_segment(point: egui::Pos2, start: egui::Pos2, end: egui::Pos2) -> f32 {
    let segment = end - start;
    let length_squared = segment.length_sq();
    if length_squared <= f32::EPSILON {
        return point.distance(start);
    }
    let projection = ((point - start).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance(start + segment * projection)
}

pub fn parse_parameter_value(raw: &str) -> Result<serde_json::Value, String> {
    let yaml: Value = serde_yaml::from_str(raw).map_err(|error| error.to_string())?;
    serde_json::to_value(yaml).map_err(|error| error.to_string())
}

fn parse_pipeline(path: &Path) -> Result<(String, Vec<PipelineNode>, Vec<PipelineEdge>), String> {
    let raw = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let document: Value = serde_yaml::from_str(&raw).map_err(|error| error.to_string())?;
    let root = document
        .as_mapping()
        .ok_or_else(|| "pipeline YAML root must be a mapping".to_owned())?;
    let name = map_get(root, "module")
        .and_then(Value::as_mapping)
        .and_then(|module| map_get(module, "name"))
        .and_then(Value::as_str)
        .unwrap_or("pipeline")
        .to_owned();
    let transforms = map_get(root, "transforms")
        .and_then(Value::as_mapping)
        .ok_or_else(|| "pipeline YAML has no transforms mapping".to_owned())?;

    let mut specs = Vec::new();
    for (id_value, spec_value) in transforms {
        if let (Some(id), Some(spec)) = (id_value.as_str(), spec_value.as_mapping()) {
            specs.push((id, spec, false));
        }
    }
    if let Some(graph_services) = map_get(root, "graph_services").and_then(Value::as_mapping) {
        for (id_value, spec_value) in graph_services {
            if let (Some(id), Some(spec)) = (id_value.as_str(), spec_value.as_mapping()) {
                specs.push((id, spec, true));
            }
        }
    }

    let mut nodes = Vec::new();
    for &(id, spec, graph_only) in &specs {
        let transform_type = map_get(spec, "type")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_owned();
        let language = map_get(spec, "language")
            .and_then(Value::as_str)
            .unwrap_or("cpp")
            .to_ascii_uppercase();
        let inputs = mapping_keys(map_get(spec, "inputs"));
        let mut outputs = sequence_strings(map_get(spec, "outputs"));
        if outputs.is_empty() {
            outputs = inferred_outputs(&transform_type);
        }
        let parameters = map_get(spec, "parameters")
            .and_then(Value::as_mapping)
            .map(|parameters| {
                parameters
                    .iter()
                    .filter_map(|(key, value)| {
                        Some(PipelineParameter {
                            name: key.as_str()?.to_owned(),
                            value: yaml_value_text(value),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let kind = classify_node(&transform_type, inputs.is_empty(), graph_only);
        nodes.push(PipelineNode {
            id: id.to_owned(),
            transform_type,
            language,
            kind,
            inputs,
            outputs,
            parameters,
            graph_only,
            position: egui::Pos2::ZERO,
        });
    }

    let by_id = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut edges = Vec::new();
    for (to, (_, spec, _)) in specs.iter().enumerate() {
        let Some(inputs) = map_get(spec, "inputs").and_then(Value::as_mapping) else {
            continue;
        };
        for (to_port, source) in inputs {
            let (Some(to_port), Some(source)) = (to_port.as_str(), source.as_str()) else {
                continue;
            };
            let Some((from_id, from_port)) = split_source(source, &by_id) else {
                continue;
            };
            let Some(&from) = by_id.get(from_id) else {
                continue;
            };
            let stream_id = (!nodes[from].graph_only).then(|| format!("{from_id}.{from_port}"));
            edges.push(PipelineEdge {
                from,
                from_port: from_port.to_owned(),
                to,
                to_port: to_port.to_owned(),
                stream_id,
            });
        }
    }
    Ok((name, nodes, edges))
}

fn layout_nodes(nodes: &mut [PipelineNode], edges: &[PipelineEdge]) {
    let mut depth = vec![0_usize; nodes.len()];
    for _ in 0..nodes.len() {
        let mut changed = false;
        for edge in edges {
            let next = depth[edge.from].saturating_add(1);
            if next > depth[edge.to] {
                depth[edge.to] = next.min(nodes.len());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut columns: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, value) in depth.into_iter().enumerate() {
        columns.entry(value).or_default().push(index);
    }
    for (column, indices) in columns {
        let mut y = 0.0;
        for index in indices {
            nodes[index].position = egui::pos2(column as f32 * 330.0, y);
            y += nodes[index].height() + 45.0;
        }
    }
}

fn classify_node(transform_type: &str, no_inputs: bool, graph_only: bool) -> NodeKind {
    let lower = transform_type.to_ascii_lowercase();
    if graph_only && (lower.contains("config") || lower.contains("corpus")) {
        NodeKind::Config
    } else if graph_only {
        NodeKind::Service
    } else if lower.contains("eventnode") || lower.contains("model") || lower.contains("classifier")
    {
        NodeKind::Model
    } else if lower.ends_with("sink") || lower.contains("recorder") {
        NodeKind::Sink
    } else if no_inputs || lower.ends_with("source") {
        NodeKind::Source
    } else {
        NodeKind::Transform
    }
}

fn inferred_outputs(transform_type: &str) -> Vec<String> {
    match transform_type {
        "SimulatedIMU" => vec!["imu_acc_gyro".to_owned(), "imu_mag".to_owned()],
        "SimulatedPPG" => vec!["ppg".to_owned()],
        "OpenCVCamera" | "CameraSource" => vec!["frame".to_owned()],
        "AudioSource" | "MicrophoneSource" => vec!["audio".to_owned()],
        _ => Vec::new(),
    }
}

fn split_source<'a>(source: &'a str, nodes: &HashMap<String, usize>) -> Option<(&'a str, &'a str)> {
    let mut candidates = nodes
        .keys()
        .filter(|id| source.starts_with(id.as_str()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|id| std::cmp::Reverse(id.len()));
    let id = candidates.first()?.as_str();
    let port = source
        .strip_prefix(id)?
        .strip_prefix('.')
        .unwrap_or("output");
    Some((&source[..id.len()], port))
}

fn map_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_owned()))
}

fn mapping_keys(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_mapping)
        .map(|mapping| {
            mapping
                .keys()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn sequence_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_sequence)
        .map(|sequence| {
            sequence
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn yaml_value_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        _ => serde_yaml::to_string(value)
            .unwrap_or_else(|_| "null".to_owned())
            .trim()
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_inspector_graph_and_model_nodes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|path| path.join("configs").join("examples").is_dir())
            .unwrap()
            .to_owned();
        let graph = PipelineUi::load(
            root.join("configs")
                .join("examples")
                .join("inspector_live.yaml"),
        );
        assert!(graph.error.is_none(), "{:?}", graph.error);
        assert_eq!(graph.nodes.len(), 14);
        assert_eq!(graph.edges.len(), 26);
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.kind == NodeKind::Model)
                .count(),
            4
        );
        assert_eq!(graph.nodes.iter().filter(|node| node.graph_only).count(), 5);
        assert_eq!(
            graph
                .edges
                .iter()
                .filter(|edge| edge.stream_id.is_none())
                .count(),
            5
        );
        assert!(graph.edges.iter().any(|edge| {
            edge.stream_id.as_deref() == Some("audio_pipeline.audio")
                && graph.nodes[edge.to].id == "audio_golden_recorder"
        }));
    }

    #[test]
    fn live_flow_requires_an_advancing_counter_and_stops_after_stall() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|path| path.join("configs").join("examples").is_dir())
            .unwrap()
            .to_owned();
        let mut graph = PipelineUi::load(
            root.join("configs")
                .join("examples")
                .join("inspector_live.yaml"),
        );
        let started = Instant::now();
        let snapshot = |total_samples, frequency_hz| StreamActivitySnapshot {
            name: "audio_pipeline.audio".to_owned(),
            frequency_hz,
            input_lag_ms: 4.0,
            batch_size: 512.0,
            total_samples,
        };

        graph.update_stream_activity_at(&[snapshot(512, 16_000.0)], started);
        assert_eq!(graph.live_stream_count_at(started), 1);

        let stalled = started + LIVE_ACTIVITY_WINDOW + Duration::from_millis(1);
        graph.update_stream_activity_at(&[snapshot(512, 16_000.0)], stalled);
        assert_eq!(graph.live_stream_count_at(stalled), 0);

        let resumed = stalled + Duration::from_millis(10);
        graph.update_stream_activity_at(&[snapshot(1_024, 16_000.0)], resumed);
        assert_eq!(graph.live_stream_count_at(resumed), 1);

        let zero_rate = resumed + Duration::from_millis(10);
        graph.update_stream_activity_at(&[snapshot(1_536, 0.0)], zero_rate);
        assert_eq!(graph.live_stream_count_at(zero_rate), 0);
    }

    #[test]
    fn parses_parameter_scalars() {
        assert_eq!(parse_parameter_value("0.55").unwrap(), json!(0.55));
        assert_eq!(parse_parameter_value("true").unwrap(), json!(true));
        assert_eq!(
            parse_parameter_value("Hey chat").unwrap(),
            json!("Hey chat")
        );
    }
}

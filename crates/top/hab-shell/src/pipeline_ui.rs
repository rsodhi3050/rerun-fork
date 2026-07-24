//! ReactFlow-style native pipeline canvas used by the Hab shell.
//!
//! Rerun remains the data/visualization substrate. The pipeline editor is a
//! product control surface, so it is painted directly in egui and sourced from
//! the same YAML file that starts the C++ engine.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Source,
    Transform,
    Model,
    Sink,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            Self::Source => "SOURCE",
            Self::Transform => "TRANSFORM",
            Self::Model => "MODEL",
            Self::Sink => "SINK",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Source => SOURCE,
            Self::Transform => ACCENT,
            Self::Model => MODEL,
            Self::Sink => SINK,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Source => "▲",
            Self::Transform => "●",
            Self::Model => "◆",
            Self::Sink => "■",
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

        self.draw_grid(&painter, canvas);
        self.draw_edges(&painter, canvas);

        let pointer = ui.input(|input| input.pointer.hover_pos());
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

    fn draw_edges(&self, painter: &egui::Painter, canvas: egui::Rect) {
        for edge in &self.edges {
            let from = self.output_port_position(edge.from, &edge.from_port, canvas);
            let to = self.input_port_position(edge.to, &edge.to_port, canvas);
            let control = ((to.x - from.x).abs() * 0.48).max(55.0 * self.zoom);
            let points = [
                from,
                from + egui::vec2(control, 0.0),
                to - egui::vec2(control, 0.0),
                to,
            ];
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                points,
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(5.0 * self.zoom, STREAM.gamma_multiply(0.11)),
            ));
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                points,
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new((1.5 * self.zoom).max(1.0), STREAM),
            ));
        }
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
                STREAM,
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
                STREAM,
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

    let mut nodes = Vec::new();
    for (id_value, spec_value) in transforms {
        let Some(id) = id_value.as_str() else {
            continue;
        };
        let Some(spec) = spec_value.as_mapping() else {
            continue;
        };
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
        let kind = classify_node(&transform_type, inputs.is_empty());
        nodes.push(PipelineNode {
            id: id.to_owned(),
            transform_type,
            language,
            kind,
            inputs,
            outputs,
            parameters,
            position: egui::Pos2::ZERO,
        });
    }

    let by_id = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut edges = Vec::new();
    for (to, (_, spec_value)) in transforms.iter().enumerate() {
        let Some(spec) = spec_value.as_mapping() else {
            continue;
        };
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
            edges.push(PipelineEdge {
                from,
                from_port: from_port.to_owned(),
                to,
                to_port: to_port.to_owned(),
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

fn classify_node(transform_type: &str, no_inputs: bool) -> NodeKind {
    let lower = transform_type.to_ascii_lowercase();
    if lower.contains("eventnode") || lower.contains("model") || lower.contains("classifier") {
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
        assert_eq!(graph.nodes.len(), 8);
        assert_eq!(graph.edges.len(), 12);
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.kind == NodeKind::Model)
                .count(),
            4
        );
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

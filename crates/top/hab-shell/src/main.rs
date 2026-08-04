//! Hab product shell embedding the customized Rerun viewer.
//
// Hab owns this shell palette independently from Rerun's internal design-token files.
#![allow(clippy::disallowed_methods)]

mod pipeline_ui;
mod services;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rerun::{
    blueprint::{
        Blueprint, BlueprintActivation, BlueprintPanel, ContainerLike, GraphView, Horizontal,
        SelectionPanel, Spatial2DView, Spatial3DView, StateTimelineView, TextLogView, TimePanel,
        TimeSeriesView, Vertical,
    },
    external::{
        eframe, egui, re_crash_handler, re_grpc_server, re_log, re_log_channel, re_memory,
        re_sdk_types, re_viewer, tokio,
    },
};

#[global_allocator]
static GLOBAL: re_memory::AccountingAllocator<mimalloc::MiMalloc> =
    re_memory::AccountingAllocator::new(mimalloc::MiMalloc);

const PAPER: egui::Color32 = egui::Color32::from_rgb(0xff, 0xff, 0xff);
const PANEL: egui::Color32 = egui::Color32::from_rgb(0xfa, 0xfa, 0xfa);
const ELEVATED: egui::Color32 = egui::Color32::from_rgb(0xf5, 0xf5, 0xf7);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0xe5, 0xe5, 0xeb);
const BORDER_SOFT: egui::Color32 = egui::Color32::from_rgb(0xef, 0xef, 0xf3);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0x11, 0x11, 0x14);
const MUTED: egui::Color32 = egui::Color32::from_rgb(0x5c, 0x5c, 0x6e);
const TERTIARY: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x8a, 0x98);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x6d, 0x28, 0xd9);
const SUCCESS: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x7d, 0x4f);
const SUCCESS_SOFT: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xf5, 0xed);
const WARNING: egui::Color32 = egui::Color32::from_rgb(0xb8, 0x86, 0x0b);
const INFO_SOFT: egui::Color32 = egui::Color32::from_rgb(0xec, 0xf4, 0xfb);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Workspace {
    #[default]
    Pipeline,
    Timeline,
    Playback,
    Streams,
    Metrics,
    RawData,
    Live,
    Device,
    Configs,
}

impl Workspace {
    const ALL: [Self; 9] = [
        Self::Pipeline,
        Self::Timeline,
        Self::Playback,
        Self::Streams,
        Self::Metrics,
        Self::RawData,
        Self::Live,
        Self::Device,
        Self::Configs,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Pipeline => "Pipeline",
            Self::Timeline => "Timeline",
            Self::Playback => "Playback",
            Self::Streams => "Streams",
            Self::Metrics => "Metric Data",
            Self::RawData => "Raw Data",
            Self::Live => "Live",
            Self::Device => "Device",
            Self::Configs => "Configs",
        }
    }

    fn icon(self) -> NavIcon {
        match self {
            Self::Pipeline => NavIcon::Pipeline,
            Self::Timeline => NavIcon::Timeline,
            Self::Playback => NavIcon::Playback,
            Self::Streams => NavIcon::Streams,
            Self::Metrics => NavIcon::Metrics,
            Self::RawData => NavIcon::RawData,
            Self::Live => NavIcon::Live,
            Self::Device => NavIcon::Device,
            Self::Configs => NavIcon::Configs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavIcon {
    Pipeline,
    Timeline,
    Playback,
    Streams,
    Metrics,
    RawData,
    Live,
    Device,
    Configs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PlaybackSub {
    #[default]
    Sessions,
    Canvas,
    Rerun,
}

impl PlaybackSub {
    const ALL: [Self; 3] = [Self::Sessions, Self::Canvas, Self::Rerun];

    fn label(self) -> &'static str {
        match self {
            Self::Sessions => "Sessions",
            Self::Canvas => "Canvas",
            Self::Rerun => "ReRun",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ConfigSub {
    #[default]
    Pipelines,
    AudioCalibration,
}

impl ConfigSub {
    const ALL: [Self; 2] = [Self::Pipelines, Self::AudioCalibration];

    fn label(self) -> &'static str {
        match self {
            Self::Pipelines => "Pipelines",
            Self::AudioCalibration => "Audio calibration",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AudioCaptureCategory {
    #[default]
    WakePositive,
    NearWakeNegative,
    OtherSpeech,
    Background,
}

impl AudioCaptureCategory {
    const ALL: [Self; 4] = [
        Self::WakePositive,
        Self::NearWakeNegative,
        Self::OtherSpeech,
        Self::Background,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::WakePositive => "wake_positive",
            Self::NearWakeNegative => "near_wake_negative",
            Self::OtherSpeech => "other_speech",
            Self::Background => "background",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "wake_positive" => Some(Self::WakePositive),
            "near_wake_negative" => Some(Self::NearWakeNegative),
            "other_speech" => Some(Self::OtherSpeech),
            "background" => Some(Self::Background),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::WakePositive => "Wake positive",
            Self::NearWakeNegative => "Near-wake negative",
            Self::OtherSpeech => "Other speech",
            Self::Background => "Background / silence",
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Self::WakePositive => "speech + wake",
            Self::NearWakeNegative => "speech, no wake",
            Self::OtherSpeech => "speech, no wake",
            Self::Background => "no speech, no wake",
        }
    }

    fn suggested_prompt(self) -> &'static str {
        match self {
            Self::WakePositive => "Hey chat",
            Self::NearWakeNegative => "Hey cat",
            Self::OtherSpeech => "Read a short sentence",
            Self::Background => "",
        }
    }

    fn suggested_condition(self) -> &'static str {
        match self {
            Self::WakePositive => "Natural voice, normal distance.",
            Self::NearWakeNegative => "Say the exact phrase naturally.",
            Self::OtherSpeech => "Read the sentence naturally.",
            Self::Background => "Do not speak during this capture.",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StreamSub {
    #[default]
    Line,
    Scatter,
    Event,
    Imu,
    Body,
    BodyModel,
    Video,
}

impl StreamSub {
    const ALL: [Self; 7] = [
        Self::Line,
        Self::Scatter,
        Self::Event,
        Self::Imu,
        Self::Body,
        Self::BodyModel,
        Self::Video,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Line => "Line",
            Self::Scatter => "Scatter",
            Self::Event => "Event",
            Self::Imu => "IMU",
            Self::Body => "Body",
            Self::BodyModel => "Body Model",
            Self::Video => "Video",
        }
    }
}

struct HabShell {
    rerun_app: re_viewer::App,
    blueprint_tx: re_log_channel::LogSender,
    grpc_shutdown: re_grpc_server::shutdown::Signal,
    workspace: Workspace,
    playback_sub: PlaybackSub,
    stream_sub: StreamSub,
    config_sub: ConfigSub,
    pending_blueprint: bool,
    last_blueprint_error: Option<String>,
    stub_notice: Option<(String, Instant)>,
    services: services::NativeServices,
    pipeline: pipeline_ui::PipelineUi,
    timeline_filters: [bool; 4],
    timeline_search: String,
    camera_texture: Option<egui::TextureHandle>,
    camera_texture_sequence: Option<(u64, i64)>,
    event_textures: HashMap<String, egui::TextureHandle>,
    playback_playing: bool,
    playback_speed: f32,
    playback_last_tick: Instant,
    playback_loaded_path: Option<PathBuf>,
    playback_texture: Option<egui::TextureHandle>,
    playback_texture_key: Option<(String, i64)>,
    audio_capture_speaker: String,
    audio_capture_session: String,
    audio_capture_test_split: bool,
    audio_capture_category: AudioCaptureCategory,
    audio_capture_prompt: String,
    audio_capture_condition: String,
    audio_capture_duration_s: f32,
    audio_capture_consent: bool,
}

impl HabShell {
    fn new(
        rerun_app: re_viewer::App,
        blueprint_tx: re_log_channel::LogSender,
        grpc_shutdown: re_grpc_server::shutdown::Signal,
    ) -> Self {
        let services = services::NativeServices::new();
        let pipeline = pipeline_ui::PipelineUi::load(
            services
                .root()
                .join("configs")
                .join("examples")
                .join("inspector_live.yaml"),
        );
        Self {
            rerun_app,
            blueprint_tx,
            grpc_shutdown,
            workspace: Workspace::Pipeline,
            playback_sub: PlaybackSub::Sessions,
            stream_sub: StreamSub::Line,
            config_sub: ConfigSub::Pipelines,
            pending_blueprint: false,
            last_blueprint_error: None,
            stub_notice: None,
            services,
            pipeline,
            timeline_filters: [true; 4],
            timeline_search: String::new(),
            camera_texture: None,
            camera_texture_sequence: None,
            event_textures: HashMap::new(),
            playback_playing: false,
            playback_speed: 1.0,
            playback_last_tick: Instant::now(),
            playback_loaded_path: None,
            playback_texture: None,
            playback_texture_key: None,
            audio_capture_speaker: "speaker-01".to_owned(),
            audio_capture_session: "session-01".to_owned(),
            audio_capture_test_split: false,
            audio_capture_category: AudioCaptureCategory::WakePositive,
            audio_capture_prompt: "Hey chat".to_owned(),
            audio_capture_condition: "Natural voice, normal distance.".to_owned(),
            audio_capture_duration_s: 3.0,
            audio_capture_consent: false,
        }
    }

    fn shutdown(&mut self) {
        if let Err(error) = self.services.shutdown() {
            re_log::warn!("HAB process shutdown was incomplete: {error}");
        }
        self.grpc_shutdown.stop();
    }

    fn recording_connected(&self) -> bool {
        self.rerun_app.recording_db().is_some()
    }

    fn application_id(&self) -> Option<String> {
        self.rerun_app
            .recording_db()
            .map(|db| db.application_id().to_string())
    }

    fn install_pending_blueprint(&mut self) {
        if !self.pending_blueprint || !workspace_uses_rerun_view(self.workspace, self.playback_sub)
        {
            self.pending_blueprint = false;
            return;
        }
        let Some(application_id) = self.application_id() else {
            return;
        };

        let activation = BlueprintActivation {
            make_active: true,
            make_default: self.workspace == Workspace::Live,
        };
        match blueprint_for(self.workspace, self.playback_sub, self.stream_sub)
            .to_log_msgs_with_activation(&application_id, activation)
        {
            Ok(messages) => {
                for message in messages {
                    if let Err(err) = self.blueprint_tx.send(message.into()) {
                        self.last_blueprint_error = Some(err.to_string());
                        return;
                    }
                }
                self.last_blueprint_error = None;
                self.pending_blueprint = false;
            }
            Err(err) => {
                self.last_blueprint_error = Some(err.to_string());
            }
        }
    }

    fn select_workspace(&mut self, workspace: Workspace) {
        if self.workspace != workspace {
            self.workspace = workspace;
            self.pending_blueprint = workspace_uses_rerun_view(self.workspace, self.playback_sub);
        }
    }

    fn show_stub_notice(&mut self, message: impl Into<String>) {
        self.stub_notice = Some((message.into(), Instant::now()));
    }

    fn masthead(&mut self, ui: &mut egui::Ui) {
        let connected = self.services.snapshot.connected;
        let device_streaming = self
            .services
            .pipeline_metrics
            .as_ref()
            .is_some_and(|metrics| {
                metrics.streams.iter().any(|stream| {
                    stream.name.starts_with("device_network.") && stream.frequency_hz > 0.05
                })
            });
        egui::Panel::top("hab_masthead")
            .exact_size(44.0)
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::symmetric(18, 0))
                    .stroke(egui::Stroke::new(1.0, BORDER_SOFT)),
            )
            .show_inside(ui, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let connect = egui::Button::new(
                        egui::RichText::new(if device_streaming {
                            "DEVICE STREAMING"
                        } else {
                            "CONNECT DEVICE"
                        })
                        .size(9.0)
                        .strong()
                        .color(if device_streaming { SUCCESS } else { MUTED })
                        .extra_letter_spacing(1.0),
                    )
                    .fill(PAPER)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .corner_radius(6.0)
                    .min_size(egui::vec2(132.0, 28.0));
                    if ui.add_enabled(!device_streaming, connect).clicked() {
                        self.services.connect_synthetic_device();
                    }
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(if connected { "CONNECTED" } else { "WAITING" })
                            .size(9.0)
                            .strong()
                            .color(if connected { SUCCESS } else { WARNING })
                            .extra_letter_spacing(1.1),
                    );
                    status_dot(ui, if connected { SUCCESS } else { WARNING });
                    ui.label(
                        egui::RichText::new("127.0.0.1:9999")
                            .size(9.0)
                            .monospace()
                            .color(TERTIARY),
                    );
                });
            });
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("hab_primary_navigation")
            .exact_size(45.0)
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::same(0))
                    .stroke(egui::Stroke::new(1.0, BORDER)),
            )
            .show_inside(ui, |ui| {
                let tab_width = Workspace::ALL
                    .iter()
                    .map(|workspace| nav_button_width(ui, workspace.label()))
                    .sum::<f32>();
                let separator_width = 34.0;
                let group_width = tab_width + separator_width;
                let leading_space = ((ui.available_width() - group_width) * 0.5).max(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(leading_space);
                    for workspace in Workspace::ALL {
                        if nav_button(
                            ui,
                            workspace.icon(),
                            workspace.label(),
                            self.workspace == workspace,
                        )
                        .clicked()
                        {
                            self.select_workspace(workspace);
                        }

                        if matches!(workspace, Workspace::Metrics | Workspace::Device) {
                            ui.add_space(8.0);
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, BORDER);
                            ui.add_space(8.0);
                        }
                    }
                });
            });
    }

    fn subnavigation(&mut self, ui: &mut egui::Ui) {
        match self.workspace {
            Workspace::Playback => {
                egui::Panel::top("hab_playback_subnav")
                    .exact_size(38.0)
                    .frame(
                        egui::Frame::new()
                            .fill(PAPER)
                            .inner_margin(egui::Margin::same(0))
                            .stroke(egui::Stroke::new(1.0, BORDER_SOFT)),
                    )
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            for sub in PlaybackSub::ALL {
                                if subnav_button(ui, sub.label(), self.playback_sub == sub)
                                    .clicked()
                                {
                                    self.playback_sub = sub;
                                    self.pending_blueprint = workspace_uses_rerun_view(
                                        self.workspace,
                                        self.playback_sub,
                                    );
                                }
                            }
                        });
                    });
            }
            Workspace::Streams => {
                egui::Panel::top("hab_streams_subnav")
                    .exact_size(38.0)
                    .frame(
                        egui::Frame::new()
                            .fill(PAPER)
                            .inner_margin(egui::Margin::same(0))
                            .stroke(egui::Stroke::new(1.0, BORDER_SOFT)),
                    )
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            for sub in StreamSub::ALL {
                                if subnav_button(ui, sub.label(), self.stream_sub == sub).clicked()
                                {
                                    self.stream_sub = sub;
                                    self.pending_blueprint = true;
                                }
                            }
                        });
                    });
            }
            Workspace::Configs => {
                egui::Panel::top("hab_configs_subnav")
                    .exact_size(38.0)
                    .frame(
                        egui::Frame::new()
                            .fill(PAPER)
                            .inner_margin(egui::Margin::same(0))
                            .stroke(egui::Stroke::new(1.0, BORDER_SOFT)),
                    )
                    .show_inside(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            for sub in ConfigSub::ALL {
                                if subnav_button(ui, sub.label(), self.config_sub == sub).clicked()
                                {
                                    self.config_sub = sub;
                                }
                            }
                        });
                    });
            }
            _ => {}
        }
    }

    fn page_header(&mut self, ui: &mut egui::Ui) {
        if !matches!(self.workspace, Workspace::Timeline | Workspace::Streams) {
            return;
        }
        let ready_models = self
            .services
            .model_statuses
            .iter()
            .filter(|status| status.state == "ready")
            .count();
        let model_errors = self
            .services
            .model_statuses
            .iter()
            .filter(|status| status.state == "error")
            .count();

        egui::Panel::top("hab_page_header")
            .exact_size(if self.workspace == Workspace::Timeline {
                70.0
            } else {
                76.0
            })
            .frame(
                egui::Frame::new()
                    .fill(if self.workspace == Workspace::Timeline {
                        ELEVATED
                    } else {
                        PAPER
                    })
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show_inside(ui, |ui| {
                if self.workspace == Workspace::Timeline {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Timeline")
                                    .size(21.0)
                                    .strong()
                                    .color(TEXT),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Model events from the canonical inspector_live graph",
                                )
                                .size(11.0)
                                .color(MUTED),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            outlined_chip(
                                ui,
                                &format!("{} / 250 events", self.services.timeline_events.len()),
                            );
                            if self.services.recording_path.is_some() {
                                if dark_button(ui, "STOP RECORDING").clicked() {
                                    self.services.stop_recording();
                                }
                            } else if dark_button(ui, "RECORD").clicked() {
                                let _ = self.services.start_recording();
                            }
                            status_chip(
                                ui,
                                if self.services.timeline_connected {
                                    "EVENT STREAM LIVE"
                                } else if self.services.snapshot.connected {
                                    "EVENT STREAM CONNECTING"
                                } else {
                                    "ENGINE DISCONNECTED"
                                },
                                if self.services.timeline_connected {
                                    SUCCESS
                                } else {
                                    WARNING
                                },
                                PAPER,
                            );
                            status_chip(
                                ui,
                                &format!(
                                    "{ready_models}/4 MODELS READY{}",
                                    if model_errors > 0 {
                                        format!(" Â· {model_errors} ERROR")
                                    } else {
                                        String::new()
                                    }
                                ),
                                if model_errors > 0 {
                                    egui::Color32::DARK_RED
                                } else if ready_models == 4 {
                                    SUCCESS
                                } else {
                                    WARNING
                                },
                                PAPER,
                            );
                        });
                    });
                } else {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(stream_title(self.stream_sub))
                                .size(20.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            egui::RichText::new(stream_description(self.stream_sub))
                                .size(11.0)
                                .color(MUTED),
                        );
                    });
                }
            });
    }

    fn timeline_event_feed(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        let mut review_timestamp_ns = None;
        let filter_kinds = ["grasp", "scene", "speech", "wake"];
        egui::Panel::right("hab_timeline_event_feed")
            .exact_size(370.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::same(16))
                    .stroke(egui::Stroke::new(1.0, BORDER)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("EVENT FEED")
                                .size(10.0)
                                .strong()
                                .color(TERTIARY)
                                .extra_letter_spacing(1.3),
                        );
                        ui.label(
                            egui::RichText::new("inspector_live")
                                .size(18.0)
                                .strong()
                                .color(TEXT),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if outline_button(ui, "CLEAR").clicked() {
                            clear = true;
                        }
                    });
                });
                ui.add_space(10.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.timeline_search)
                        .hint_text("Filter events…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for (index, kind) in filter_kinds.iter().enumerate() {
                        let color = timeline_kind_color(kind);
                        if filter_chip(ui, kind, self.timeline_filters[index], color).clicked() {
                            self.timeline_filters[index] = !self.timeline_filters[index];
                        }
                    }
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);

                let query = self.timeline_search.to_ascii_lowercase();
                let visible = self
                    .services
                    .timeline_events
                    .iter()
                    .filter(|event| {
                        let enabled = filter_kinds
                            .iter()
                            .position(|kind| *kind == event.kind)
                            .is_none_or(|index| self.timeline_filters[index]);
                        enabled
                            && (query.is_empty()
                                || event.title.to_ascii_lowercase().contains(&query)
                                || event.summary.to_ascii_lowercase().contains(&query)
                                || event.label.to_ascii_lowercase().contains(&query))
                    })
                    .cloned()
                    .collect::<Vec<_>>();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.add_space(30.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("Waiting for model events")
                                        .size(14.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "Camera and audio events will appear here as model nodes emit them.",
                                    )
                                    .size(11.0)
                                    .color(MUTED),
                                );
                            });
                        }
                        for event in &visible {
                            if !self.event_textures.contains_key(&event.id)
                                && let Some(thumbnail) = &event.thumbnail
                            {
                                let image = egui::ColorImage::from_rgb(
                                    [thumbnail.width, thumbnail.height],
                                    &thumbnail.rgb,
                                );
                                let texture = ui.ctx().load_texture(
                                    format!("hab-event-evidence-{}", event.id),
                                    image,
                                    egui::TextureOptions::LINEAR,
                                );
                                self.event_textures.insert(event.id.clone(), texture);
                            }
                            if timeline_event_card(
                                ui,
                                event,
                                self.event_textures.get(&event.id),
                            )
                            .clicked()
                            {
                                review_timestamp_ns = Some(event.timestamp_ns);
                            }
                            ui.add_space(8.0);
                        }
                    });
            });
        if clear {
            self.services.clear_timeline();
            self.event_textures.clear();
        }
        if let Some(timestamp_ns) = review_timestamp_ns {
            let _ = self.services.request_audio_evidence(timestamp_ns);
            self.rerun_app.hab_prepare_playback();
            if self.rerun_app.hab_seek_playback(timestamp_ns) {
                self.workspace = Workspace::Playback;
                self.playback_sub = PlaybackSub::Canvas;
                self.playback_playing = false;
                self.playback_last_tick = Instant::now();
                self.playback_texture_key = Some(("event-seek".to_owned(), timestamp_ns));
                self.show_stub_notice("Playback moved to the selected event evidence");
            } else {
                self.show_stub_notice(
                    "The selected event is not available in the active Rerun recording yet",
                );
            }
        }
    }

    fn update_camera_texture(&mut self, ctx: &egui::Context) {
        let Some(frame) = self.services.camera_frame.as_ref() else {
            return;
        };
        let frame_key = (frame.sequence, frame.timestamp_ns);
        if self.camera_texture_sequence == Some(frame_key) {
            return;
        }
        let image = egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb);
        if let Some(texture) = &mut self.camera_texture {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.camera_texture =
                Some(ctx.load_texture("hab-camera-preview", image, egui::TextureOptions::LINEAR));
        }
        self.camera_texture_sequence = Some(frame_key);
    }

    fn timeline_media_page(&mut self, ui: &mut egui::Ui) {
        self.update_camera_texture(ui.ctx());
        let camera_size = self
            .services
            .camera_frame
            .as_ref()
            .map(|frame| (frame.width, frame.height));
        let audio_levels = self
            .services
            .audio_levels
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let audio_rms = self.services.audio_rms;
        let audio_peak = self.services.audio_peak;
        let audio_rate = self.services.audio_sample_rate_hz;
        let audio_channels = self.services.audio_channels;
        let audio_source = self.services.audio_source.clone();
        let audio_sample_format = self.services.audio_sample_format.clone();
        let events = self
            .services
            .timeline_events
            .iter()
            .take(100)
            .cloned()
            .collect::<Vec<_>>();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ELEVATED)
                    .inner_margin(egui::Margin::same(18)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let gap = 14.0;
                        let available = ui.available_width();
                        let camera_width = ((available - gap) * 0.575).max(320.0);
                        let audio_width = (available - gap - camera_width).max(260.0);
                        let media_height = (ui.available_height() * 0.56).clamp(300.0, 500.0);

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            ui.allocate_ui_with_layout(
                                egui::vec2(camera_width, media_height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    media_card(ui, "CAMERA", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("Front camera")
                                                    .size(14.0)
                                                    .strong()
                                                    .color(TEXT),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    status_chip(
                                                        ui,
                                                        if camera_size.is_some() {
                                                            "LIVE"
                                                        } else {
                                                            "WAITING"
                                                        },
                                                        if camera_size.is_some() {
                                                            SUCCESS
                                                        } else {
                                                            WARNING
                                                        },
                                                        PAPER,
                                                    );
                                                    if let Some((width, height)) = camera_size {
                                                        outlined_chip(
                                                            ui,
                                                            &format!("{width} × {height}"),
                                                        );
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(10.0);
                                        let viewport_height =
                                            (ui.available_height() - 2.0).max(180.0);
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), viewport_height),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().rect_filled(
                                            rect,
                                            7.0,
                                            egui::Color32::from_rgb(14, 14, 18),
                                        );
                                        if let (Some(texture), Some((width, height))) =
                                            (&self.camera_texture, camera_size)
                                        {
                                            let scale = (rect.width() / width as f32)
                                                .min(rect.height() / height as f32);
                                            let image_size =
                                                egui::vec2(width as f32, height as f32) * scale;
                                            let image_rect = egui::Rect::from_center_size(
                                                rect.center(),
                                                image_size,
                                            );
                                            ui.painter().image(
                                                texture.id(),
                                                image_rect,
                                                egui::Rect::from_min_max(
                                                    egui::Pos2::ZERO,
                                                    egui::pos2(1.0, 1.0),
                                                ),
                                                egui::Color32::WHITE,
                                            );
                                        } else {
                                            paint_empty_media(
                                                ui.painter(),
                                                rect,
                                                NavIcon::Timeline,
                                                "Waiting for camera frames",
                                            );
                                        }
                                    });
                                },
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(audio_width, media_height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    media_card(ui, "AUDIO", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(
                                                    if audio_source == "synthetic" {
                                                        "Synthetic test tone"
                                                    } else {
                                                        "Microphone"
                                                    },
                                                )
                                                .size(14.0)
                                                .strong()
                                                .color(TEXT),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    status_chip(
                                                        ui,
                                                        if audio_rate > 0 {
                                                            "LIVE"
                                                        } else {
                                                            "WAITING"
                                                        },
                                                        if audio_rate > 0 {
                                                            SUCCESS
                                                        } else {
                                                            WARNING
                                                        },
                                                        PAPER,
                                                    );
                                                    if audio_rate > 0 {
                                                        outlined_chip(
                                                            ui,
                                                            &format!(
                                                                "{:.1} kHz · {} ch",
                                                                audio_rate as f32 / 1000.0,
                                                                audio_channels
                                                            ),
                                                        );
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(10.0);
                                        let waveform_height =
                                            (ui.available_height() - 48.0).max(170.0);
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), waveform_height),
                                            egui::Sense::hover(),
                                        );
                                        paint_audio_waveform(
                                            ui.painter(),
                                            rect,
                                            &audio_levels,
                                            audio_rms,
                                            audio_peak,
                                        );
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("RMS LEVEL")
                                                    .size(9.0)
                                                    .strong()
                                                    .color(TERTIARY)
                                                    .extra_letter_spacing(1.0),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{:.1} dBFS RMS  ·  {:.1} dBFS peak",
                                                            amplitude_to_dbfs(audio_rms),
                                                            amplitude_to_dbfs(audio_peak),
                                                        ))
                                                        .size(10.0)
                                                        .monospace()
                                                        .color(MUTED),
                                                    );
                                                },
                                            );
                                        });
                                        ui.horizontal_wrapped(|ui| {
                                            outlined_chip(
                                                ui,
                                                match audio_source.as_str() {
                                                    "wasapi" => "WASAPI DEFAULT INPUT",
                                                    "synthetic" => "SYNTHETIC FALLBACK",
                                                    _ => "SOURCE UNKNOWN",
                                                },
                                            );
                                            if !audio_sample_format.is_empty() {
                                                outlined_chip(
                                                    ui,
                                                    &audio_sample_format.to_uppercase(),
                                                );
                                            }
                                        });
                                    });
                                },
                            );
                        });

                        ui.add_space(14.0);
                        event_activity_card(ui, &events);
                    });
            });
    }

    fn pipeline_inspector(&mut self, ui: &mut egui::Ui) {
        if self.workspace != Workspace::Pipeline {
            return;
        }

        let selected_index = self.pipeline.selected;
        let selected_metric = selected_index
            .and_then(|index| self.pipeline.nodes.get(index))
            .and_then(|node| {
                self.services
                    .pipeline_metrics
                    .as_ref()?
                    .transforms
                    .iter()
                    .find(|metric| metric.id == node.id)
            })
            .cloned();
        let selected_status = selected_index
            .and_then(|index| self.pipeline.nodes.get(index))
            .and_then(|node| {
                self.services
                    .model_statuses
                    .iter()
                    .find(|status| status.node == node.id)
            })
            .cloned();
        let mut apply_parameter: Option<(String, String, String)> = None;
        egui::Panel::right("hab_pipeline_inspector")
            .exact_size(360.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::same(18))
                    .stroke(egui::Stroke::new(1.0, BORDER)),
            )
            .show_inside(ui, |ui| {
                ui.label(
                    egui::RichText::new("PIPELINE INSPECTOR")
                        .size(9.0)
                        .strong()
                        .color(TERTIARY)
                        .extra_letter_spacing(1.4),
                );
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(
                        "Select a node to inspect its ports and hot-edit parameters in the running C++ graph.",
                    )
                    .size(12.0)
                    .color(MUTED),
                );
                ui.add_space(18.0);
                if let Some(index) = selected_index {
                    if let Some(node) = self.pipeline.nodes.get_mut(index) {
                        info_card(ui, |ui| {
                            ui.horizontal(|ui| {
                                status_dot(ui, node.kind.color());
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&node.id)
                                            .size(15.0)
                                            .strong()
                                            .color(TEXT),
                                    );
                                    ui.label(
                                        egui::RichText::new(&node.transform_type)
                                            .size(10.0)
                                            .monospace()
                                            .color(TERTIARY),
                                    );
                                });
                            });
                            ui.add_space(12.0);
                            key_value(ui, "Runtime", &node.language);
                            key_value(ui, "Inputs", &node.inputs.len().to_string());
                            key_value(ui, "Outputs", &node.outputs.len().to_string());
                            if let Some(metric) = &selected_metric {
                                key_value(
                                    ui,
                                    "Live latency",
                                    &format!("{:.2} ms", metric.latency_ms),
                                );
                                key_value(
                                    ui,
                                    "Jitter",
                                    &format!("± {:.2} ms", metric.jitter_ms),
                                );
                            }
                            if let Some(status) = &selected_status {
                                key_value(ui, "Model state", &status.state.to_ascii_uppercase());
                                key_value(ui, "Event type", &status.event_type);
                                key_value(
                                    ui,
                                    "Model",
                                    &if status.model_version.is_empty() {
                                        status.model_name.clone()
                                    } else {
                                        format!("{}@{}", status.model_name, status.model_version)
                                    },
                                );
                                key_value(
                                    ui,
                                    "Execution",
                                    &format!(
                                        "{} Â· {}",
                                        status.model_runtime, status.model_backend
                                    ),
                                );
                                key_value(
                                    ui,
                                    "Startup warm-up",
                                    &format!("{:.2} ms", status.warmup_latency_ms),
                                );
                                key_value(ui, "Heartbeat", &status.clock_time());
                                if !status.error.is_empty() {
                                    ui.add_space(8.0);
                                    ui.colored_label(
                                        egui::Color32::DARK_RED,
                                        &status.error,
                                    );
                                }
                            }
                        });
                        ui.add_space(16.0);
                        section_label(ui, "PORTS");
                        ui.horizontal_wrapped(|ui| {
                            for input in &node.inputs {
                                outlined_chip(ui, &format!("in · {input}"));
                            }
                            for output in &node.outputs {
                                outlined_chip(ui, &format!("out · {output}"));
                            }
                        });
                        ui.add_space(18.0);
                        section_label(ui, "LIVE PARAMETERS");
                        if node.parameters.is_empty() {
                            ui.label(
                                egui::RichText::new("This transform declares no parameters.")
                                    .size(11.0)
                                    .color(TERTIARY),
                            );
                        }
                        for parameter in &mut node.parameters {
                            ui.label(
                                egui::RichText::new(parameter.name.to_ascii_uppercase())
                                    .size(8.5)
                                    .strong()
                                    .color(TERTIARY)
                                    .extra_letter_spacing(0.7),
                            );
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut parameter.value)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(205.0),
                                );
                                if outline_button(ui, "APPLY").clicked() {
                                    apply_parameter = Some((
                                        node.id.clone(),
                                        parameter.name.clone(),
                                        parameter.value.clone(),
                                    ));
                                }
                            });
                            ui.add_space(8.0);
                        }
                    }
                } else {
                    info_card(ui, |ui| {
                        ui.horizontal(|ui| {
                            status_dot(
                                ui,
                                if self.services.snapshot.connected {
                                    SUCCESS
                                } else {
                                    WARNING
                                },
                            );
                            ui.label(
                                egui::RichText::new(&self.pipeline.name)
                                    .size(13.0)
                                    .strong()
                                    .color(TEXT),
                            );
                        });
                        ui.add_space(12.0);
                        key_value(ui, "Runtime", "C++ native");
                        key_value(ui, "Nodes", &self.pipeline.nodes.len().to_string());
                        key_value(
                            ui,
                            "Model nodes",
                            &self
                                .pipeline
                                .nodes
                                .iter()
                                .filter(|node| node.kind == pipeline_ui::NodeKind::Model)
                                .count()
                                .to_string(),
                        );
                        key_value(ui, "Feeds", &self.services.snapshot.feed_count.to_string());
                    });
                    ui.add_space(16.0);
                    section_label(ui, "SELECT A NODE");
                    ui.label(
                        egui::RichText::new(
                            "The canvas supports selection, dragging, pan, wheel zoom, and fit-to-graph.",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );
                }
            });

        if let Some((transform, parameter, raw_value)) = apply_parameter {
            match pipeline_ui::parse_parameter_value(&raw_value) {
                Ok(value) => self.services.set_parameter(transform, parameter, value),
                Err(error) => self.show_stub_notice(format!("Invalid parameter value: {error}")),
            }
        }
    }

    fn pipeline_page(&mut self, ui: &mut egui::Ui) {
        let ready_models = self
            .services
            .model_statuses
            .iter()
            .filter(|status| status.state == "ready")
            .count();
        let model_errors = self
            .services
            .model_statuses
            .iter()
            .filter(|status| status.state == "error")
            .count();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PAPER))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&self.pipeline.name)
                                .size(16.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            egui::RichText::new(self.pipeline.path.to_string_lossy())
                                .size(9.0)
                                .monospace()
                                .color(TERTIARY),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(14.0);
                        if outline_button(ui, "FIT GRAPH").clicked() {
                            self.pipeline.request_fit();
                        }
                        if self.services.recording_path.is_some() {
                            if dark_button(ui, "STOP RECORDING").clicked() {
                                self.services.stop_recording();
                            }
                        } else if dark_button(ui, "RECORD").clicked() {
                            let _ = self.services.start_recording();
                        }
                        status_chip(
                            ui,
                            if self.services.snapshot.connected {
                                "ENGINE RUNNING"
                            } else {
                                "ENGINE OFFLINE"
                            },
                            if self.services.snapshot.connected {
                                SUCCESS
                            } else {
                                WARNING
                            },
                            if self.services.snapshot.connected {
                                SUCCESS_SOFT
                            } else {
                                PAPER
                            },
                        );
                        status_chip(
                            ui,
                            &format!("{ready_models}/4 MODELS READY"),
                            if model_errors > 0 {
                                egui::Color32::DARK_RED
                            } else if ready_models == 4 {
                                SUCCESS
                            } else {
                                WARNING
                            },
                            if ready_models == 4 {
                                SUCCESS_SOFT
                            } else {
                                PAPER
                            },
                        );
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                if let Some(error) = &self.pipeline.error {
                    ui.colored_label(egui::Color32::DARK_RED, error);
                } else {
                    self.pipeline.canvas(ui);
                }
            });
    }

    fn show_custom_page(&mut self, ui: &mut egui::Ui) -> bool {
        match self.workspace {
            Workspace::Pipeline => {
                self.pipeline_page(ui);
                true
            }
            Workspace::Timeline => {
                self.timeline_media_page(ui);
                true
            }
            Workspace::Playback if self.playback_sub == PlaybackSub::Sessions => {
                self.sessions_page_live(ui);
                true
            }
            Workspace::Playback if self.playback_sub == PlaybackSub::Canvas => {
                self.playback_canvas_page(ui);
                true
            }
            Workspace::Metrics => {
                self.metrics_page(ui);
                true
            }
            Workspace::RawData => {
                self.raw_data_page(ui);
                true
            }
            Workspace::Live => {
                self.live_page(ui);
                true
            }
            Workspace::Device => {
                self.device_page(ui);
                true
            }
            Workspace::Configs if self.config_sub == ConfigSub::Pipelines => {
                self.configs_page_live(ui);
                true
            }
            Workspace::Configs => {
                self.audio_calibration_page(ui);
                true
            }
            _ => false,
        }
    }

    fn live_page(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("hab_live_header")
            .exact_size(70.0)
            .frame(
                egui::Frame::new()
                    .fill(ELEVATED)
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    page_title(
                        ui,
                        "Live",
                        "Camera, microphone, and model activity from the active C++ graph.",
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.services.recording_path.is_some() {
                            if dark_button(ui, "STOP RECORDING").clicked() {
                                self.services.stop_recording();
                            }
                        } else if dark_button(ui, "RECORD").clicked() {
                            let _ = self.services.start_recording();
                        }
                        status_chip(
                            ui,
                            if self.services.timeline_connected {
                                "LIVE INPUTS"
                            } else {
                                "WAITING FOR INPUTS"
                            },
                            if self.services.timeline_connected {
                                SUCCESS
                            } else {
                                WARNING
                            },
                            PAPER,
                        );
                    });
                });
            });
        self.timeline_media_page(ui);
    }

    fn playback_canvas_page(&mut self, ui: &mut egui::Ui) {
        self.rerun_app.hab_prepare_playback();
        if self.playback_texture_key.is_none()
            && let Some((_, min, _, current, _)) = self.rerun_app.hab_playback_snapshot()
            && current != min
        {
            let _ = self.rerun_app.hab_seek_playback(min);
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.playback_last_tick);
        self.playback_last_tick = now;

        if self.playback_playing
            && let Some((timeline, min, max, current, _)) = self.rerun_app.hab_playback_snapshot()
        {
            let units_per_second = playback_units_per_second(&timeline, min, max);
            let advance =
                (elapsed.as_secs_f64() * self.playback_speed as f64 * units_per_second) as i64;
            let next = current.saturating_add(advance.max(1));
            if next >= max {
                let _ = self.rerun_app.hab_seek_playback(max);
                self.playback_playing = false;
            } else {
                let _ = self.rerun_app.hab_seek_playback(next);
            }
        }

        let snapshot = self.rerun_app.hab_playback_snapshot();
        if let Some((_, _, _, current, _)) = &snapshot
            && let Some((entity, width, height, rgb)) = self.rerun_app.hab_playback_image()
        {
            let key = (entity, *current);
            if self.playback_texture_key.as_ref() != Some(&key) {
                let image = egui::ColorImage::from_rgb([width as usize, height as usize], &rgb);
                if let Some(texture) = &mut self.playback_texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.playback_texture = Some(ui.ctx().load_texture(
                        "hab-playback-frame",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                self.playback_texture_key = Some(key);
            }
        }

        let selected_session = self
            .services
            .sessions
            .get(self.services.selected_session)
            .cloned();
        let mut return_to_sessions = false;
        let mut open_expert = false;
        let mut request_audio = false;
        let mut play_audio = false;
        let mut stop_audio = false;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ELEVATED)
                    .inner_margin(egui::Margin::same(22)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            let subtitle = self
                                .playback_loaded_path
                                .as_ref()
                                .map_or_else(
                                    || "Loading the selected recording.".to_owned(),
                                    |path| path.to_string_lossy().into_owned(),
                                );
                            page_title(
                                ui,
                                "Playback Canvas",
                                &subtitle,
                            );
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if outline_button(ui, "EXPERT RERUN").clicked() {
                                    open_expert = true;
                                }
                                if outline_button(ui, "CHANGE SESSION").clicked() {
                                    return_to_sessions = true;
                                }
                                status_chip(
                                    ui,
                                    if snapshot.is_some() {
                                        "RECORDING READY"
                                    } else {
                                        "LOADING"
                                    },
                                    if snapshot.is_some() { SUCCESS } else { WARNING },
                                    PAPER,
                                );
                            },
                        );
                    });
                    ui.add_space(18.0);

                    let Some((timeline, min, max, current, entities)) = snapshot.clone() else {
                        notice_card(
                            ui,
                            "Opening the recording",
                            "Rerun is indexing the selected RRD. The HAB canvas will appear as soon as its timeline and entities are available.",
                        );
                        return;
                    };

                    let gap = 14.0;
                    let left_width = ((ui.available_width() - gap) * 0.58).max(360.0);
                    let right_width = (ui.available_width() - gap - left_width).max(300.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.allocate_ui_with_layout(
                            egui::vec2(left_width, 390.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                media_card(ui, "RECORDED CAMERA", |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Frame at cursor")
                                                .size(14.0)
                                                .strong()
                                                .color(TEXT),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                outlined_chip(
                                                    ui,
                                                    &format_playback_position(
                                                        &timeline, min, current,
                                                    ),
                                                );
                                            },
                                        );
                                    });
                                    ui.add_space(10.0);
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), 320.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(
                                        rect,
                                        7.0,
                                        egui::Color32::from_rgb(14, 14, 18),
                                    );
                                    if let Some(texture) = &self.playback_texture {
                                        let texture_size = texture.size_vec2();
                                        let scale = (rect.width() / texture_size.x)
                                            .min(rect.height() / texture_size.y);
                                        let image_rect = egui::Rect::from_center_size(
                                            rect.center(),
                                            texture_size * scale,
                                        );
                                        ui.painter().image(
                                            texture.id(),
                                            image_rect,
                                            egui::Rect::from_min_max(
                                                egui::Pos2::ZERO,
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            egui::Color32::WHITE,
                                        );
                                    } else {
                                        paint_empty_media(
                                            ui.painter(),
                                            rect,
                                            NavIcon::Playback,
                                            "No raw camera image at this cursor",
                                        );
                                    }
                                });
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(right_width, 390.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                media_card(ui, "RECORDING", |ui| {
                                    if let Some(session) = &selected_session {
                                        key_value(ui, "Study", &session.study);
                                        key_value(ui, "Participant", &session.participant);
                                        key_value(ui, "Session", &session.name);
                                        key_value(
                                            ui,
                                            "File",
                                            &services::human_bytes(session.size_bytes),
                                        );
                                    }
                                    key_value(ui, "Timeline", &timeline);
                                    key_value(
                                        ui,
                                        "Duration",
                                        &format_playback_duration(&timeline, min, max),
                                    );
                                    key_value(ui, "Entities", &entities.len().to_string());
                                    ui.add_space(12.0);
                                    section_label(ui, "RECORDED STREAMS");
                                    egui::ScrollArea::vertical()
                                        .max_height(190.0)
                                        .show(ui, |ui| {
                                            for entity in entities
                                                .iter()
                                                .filter(|entity| entity.as_str() != "/")
                                                .take(12)
                                            {
                                                ui.horizontal(|ui| {
                                                    status_dot(
                                                        ui,
                                                        playback_lane_color(entity),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(entity)
                                                            .size(9.5)
                                                            .monospace()
                                                            .color(TEXT),
                                                    );
                                                });
                                            }
                                        });
                                });
                            },
                        );
                    });

                    ui.add_space(14.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 250.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| media_card(ui, "EVENT AUDIO EVIDENCE", |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("Audio around the playback cursor")
                                        .size(13.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "Lossless PCM is loaded from the 30-second live ring or the synchronized HDF5 recording.",
                                    )
                                    .size(10.0)
                                    .color(MUTED),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if self.services.audio_evidence_pending {
                                        status_chip(ui, "PREPARING", WARNING, PAPER);
                                    } else if self.services.audio_evidence_error.is_some() {
                                        status_chip(
                                            ui,
                                            "AUDIO UNAVAILABLE",
                                            egui::Color32::from_rgb(190, 64, 62),
                                            PAPER,
                                        );
                                    } else if self.services.audio_evidence.is_some() {
                                        status_chip(ui, "PCM READY", SUCCESS, PAPER);
                                    } else {
                                        status_chip(ui, "NOT LOADED", TERTIARY, PAPER);
                                    }
                                },
                            );
                        });
                        ui.add_space(10.0);
                        if let Some(clip) = &self.services.audio_evidence {
                            let peak = clip.levels.iter().copied().fold(0.0_f32, f32::max);
                            let rms = if clip.levels.is_empty() {
                                0.0
                            } else {
                                (clip
                                    .levels
                                    .iter()
                                    .map(|value| value * value)
                                    .sum::<f32>()
                                    / clip.levels.len() as f32)
                                    .sqrt()
                            };
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 106.0),
                                egui::Sense::hover(),
                            );
                            paint_audio_waveform(ui.painter(), rect, &clip.levels, rms, peak);
                            let marker_fraction = if clip.end_timestamp_ns > clip.start_timestamp_ns {
                                (clip.center_timestamp_ns - clip.start_timestamp_ns) as f32
                                    / (clip.end_timestamp_ns - clip.start_timestamp_ns) as f32
                            } else {
                                0.5
                            }
                            .clamp(0.0, 1.0);
                            let marker_x = rect.left() + rect.width() * marker_fraction;
                            ui.painter().line_segment(
                                [
                                    egui::pos2(marker_x, rect.top() + 6.0),
                                    egui::pos2(marker_x, rect.bottom() - 6.0),
                                ],
                                egui::Stroke::new(1.5, egui::Color32::from_rgb(224, 92, 89)),
                            );
                            ui.painter().text(
                                egui::pos2(marker_x + 5.0, rect.top() + 8.0),
                                egui::Align2::LEFT_TOP,
                                "EVENT",
                                egui::FontId::monospace(8.5),
                                egui::Color32::from_rgb(190, 64, 62),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                outlined_chip(ui, &format!("{:.2} s", clip.duration_s()));
                                outlined_chip(
                                    ui,
                                    &format!(
                                        "{:.1} kHz / {} ch",
                                        clip.sample_rate_hz as f32 / 1_000.0,
                                        clip.channels
                                    ),
                                );
                                ui.label(
                                    egui::RichText::new(&clip.stream_id)
                                        .size(9.0)
                                        .monospace()
                                        .color(TERTIARY),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if outline_button(ui, "STOP AUDIO").clicked() {
                                            stop_audio = true;
                                        }
                                        if dark_button(ui, "PLAY AUDIO").clicked() {
                                            play_audio = true;
                                        }
                                        if outline_button(ui, "RELOAD AT CURSOR").clicked() {
                                            request_audio = true;
                                        }
                                    },
                                );
                            });
                            ui.label(
                                egui::RichText::new(format!(
                                    "Source: {}",
                                    clip.source.display()
                                ))
                                .size(8.5)
                                .monospace()
                                .color(TERTIARY),
                            );
                            if let Some(error) = &self.services.audio_evidence_error {
                                ui.label(
                                    egui::RichText::new(error)
                                        .size(9.0)
                                        .color(egui::Color32::from_rgb(190, 64, 62)),
                                );
                            }
                        } else {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 82.0),
                                egui::Sense::hover(),
                            );
                            paint_empty_media(
                                ui.painter(),
                                rect,
                                NavIcon::Streams,
                                "Load the exact PCM window around this cursor",
                            );
                            ui.add_space(8.0);
                            if dark_button(ui, "LOAD AUDIO AT CURSOR").clicked() {
                                request_audio = true;
                            }
                        }
                        }),
                    );

                    ui.add_space(14.0);
                    info_card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("SESSION TIMELINE")
                                    .size(9.0)
                                    .strong()
                                    .color(TERTIARY)
                                    .extra_letter_spacing(1.1),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    outlined_chip(ui, &timeline);
                                },
                            );
                        });
                        ui.add_space(8.0);
                        playback_entity_lanes(ui, &entities, min, max, current);
                        ui.add_space(8.0);
                        let span = max.saturating_sub(min).max(1);
                        let mut fraction =
                            current.saturating_sub(min) as f64 / span as f64;
                        if let Some(requested_fraction) =
                            playback_scrubber(ui, fraction as f32)
                        {
                            fraction = requested_fraction as f64;
                            self.playback_playing = false;
                            let requested =
                                min.saturating_add((fraction * span as f64) as i64);
                            let _ = self.rerun_app.hab_seek_playback(requested);
                        }
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format_playback_position(
                                    &timeline, min, current,
                                ))
                                .size(10.0)
                                .monospace()
                                .color(TEXT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format_playback_duration(
                                            &timeline, min, max,
                                        ))
                                        .size(10.0)
                                        .monospace()
                                        .color(MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(12.0);
                        ui.horizontal_centered(|ui| {
                            let step = playback_units_per_second(&timeline, min, max) as i64;
                            if outline_button(ui, "START").clicked() {
                                self.playback_playing = false;
                                let _ = self.rerun_app.hab_seek_playback(min);
                            }
                            if outline_button(ui, "-10 SEC").clicked() {
                                self.playback_playing = false;
                                let _ = self
                                    .rerun_app
                                    .hab_seek_playback(current.saturating_sub(step * 10));
                            }
                            if dark_button(
                                ui,
                                if self.playback_playing { "PAUSE" } else { "PLAY" },
                            )
                            .clicked()
                            {
                                if !self.playback_playing && current >= max {
                                    let _ = self.rerun_app.hab_seek_playback(min);
                                }
                                self.playback_playing = !self.playback_playing;
                                self.playback_last_tick = Instant::now();
                            }
                            if outline_button(ui, "+10 SEC").clicked() {
                                self.playback_playing = false;
                                let _ = self
                                    .rerun_app
                                    .hab_seek_playback(current.saturating_add(step * 10));
                            }
                            if outline_button(ui, "END").clicked() {
                                self.playback_playing = false;
                                let _ = self.rerun_app.hab_seek_playback(max);
                            }
                            egui::ComboBox::from_id_salt("hab_playback_speed")
                                .selected_text(format!("{:.2}×", self.playback_speed))
                                .show_ui(ui, |ui| {
                                    for speed in [0.25, 0.5, 1.0, 1.5, 2.0] {
                                        ui.selectable_value(
                                            &mut self.playback_speed,
                                            speed,
                                            format!("{speed:.2}×"),
                                        );
                                    }
                                });
                        });
                    });
                });
            });

        if request_audio
            && let Some((_, _, _, current, _)) = snapshot
            && !self.services.request_audio_evidence(current)
        {
            self.show_stub_notice("No live or HDF5 audio is available at this cursor");
        }
        if play_audio
            && let Some(clip) = &self.services.audio_evidence
            && let Err(error) = play_wav_file(&clip.wav_path)
        {
            self.show_stub_notice(error);
        }
        if stop_audio {
            stop_wav_playback();
        }

        if return_to_sessions {
            self.playback_playing = false;
            self.playback_sub = PlaybackSub::Sessions;
        }
        if open_expert {
            self.playback_playing = false;
            self.playback_sub = PlaybackSub::Rerun;
            self.pending_blueprint = true;
        }
    }

    fn sessions_page_live(&mut self, ui: &mut egui::Ui) {
        let sessions = self.services.sessions.clone();
        let mut review_selected = false;
        let mut expert_selected = false;
        let mut review_hdf5 = false;
        let mut refresh = false;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        page_title(
                            ui,
                            "Recorded Sessions",
                            &format!(
                                "{} indexed recordings. Select one to inspect and review.",
                                sessions.len()
                            ),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if outline_button(ui, "REFRESH").clicked() {
                                    refresh = true;
                                }
                            },
                        );
                    });
                    ui.add_space(20.0);
                    if sessions.is_empty() {
                        notice_card(
                            ui,
                            "No recordings yet",
                            "Use Record on Pipeline or Timeline. HAB will index each finalized RRD or HDF5 session here.",
                        );
                        return;
                    }

                    section_label(ui, "SESSION");
                    ui.scope(|ui| {
                        ui.set_max_width(720.0);
                        let selected_text = sessions
                            .get(self.services.selected_session)
                            .map(|session| {
                                format!(
                                    "{} / {} / {}",
                                    session.study, session.participant, session.name
                                )
                            })
                            .unwrap_or_else(|| "Select a session".to_owned());
                        egui::ComboBox::from_id_salt("hab_session_selector")
                            .selected_text(selected_text)
                            .width(700.0)
                            .show_ui(ui, |ui| {
                                for (index, session) in sessions.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.services.selected_session,
                                        index,
                                        format!(
                                            "{} / {} / {}  ·  {}",
                                            session.study,
                                            session.participant,
                                            session.name,
                                            services::human_age(session.modified)
                                        ),
                                    );
                                }
                            });
                    });

                    let Some(session) = sessions.get(self.services.selected_session) else {
                        return;
                    };
                    ui.add_space(18.0);
                    ui.scope(|ui| {
                        ui.set_max_width(960.0);
                        info_card(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&session.name)
                                            .size(17.0)
                                            .strong()
                                            .color(TEXT),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}  ·  participant {}",
                                            session.study, session.participant
                                        ))
                                        .size(11.0)
                                        .color(MUTED),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        outlined_chip(
                                            ui,
                                            &services::human_age(session.modified),
                                        );
                                        outlined_chip(
                                            ui,
                                            &services::human_bytes(session.size_bytes),
                                        );
                                        status_chip(ui, session.format, ACCENT, INFO_SOFT);
                                    },
                                );
                            });
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(session.path.to_string_lossy())
                                    .size(9.0)
                                    .monospace()
                                    .color(TERTIARY),
                            );
                            ui.add_space(16.0);
                            section_label(ui, "ARTIFACTS");
                            ui.horizontal_wrapped(|ui| {
                                if session.artifacts.is_empty() {
                                    let filename = session
                                        .path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy();
                                    outlined_chip(ui, &filename);
                                } else {
                                    for artifact in &session.artifacts {
                                        outlined_chip(ui, artifact);
                                    }
                                }
                            });
                            ui.add_space(16.0);
                            section_label(ui, "ANNOTATION PREVIEW");
                            if session.annotation_preview.is_empty() {
                                ui.label(
                                    egui::RichText::new(
                                        "No annotation sidecar was found for this recording.",
                                    )
                                    .size(11.0)
                                    .color(MUTED),
                                );
                            } else {
                                for line in &session.annotation_preview {
                                    ui.label(
                                        egui::RichText::new(line)
                                            .size(10.0)
                                            .monospace()
                                            .color(TEXT),
                                    );
                                }
                            }
                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                if session.rrd_path.is_some() {
                                    if dark_button(ui, "REVIEW RRD").clicked() {
                                        review_selected = true;
                                    }
                                    if outline_button(ui, "RRD EXPERT").clicked() {
                                        expert_selected = true;
                                    }
                                }
                                if session.hdf5_path.is_some()
                                    && outline_button(ui, "VALIDATE + REVIEW HDF5").clicked()
                                {
                                    review_hdf5 = true;
                                }
                            });
                        });
                    });
                });
            });

        if refresh {
            let message = self.services.refresh_sessions();
            self.show_stub_notice(message);
        }
        if review_hdf5 {
            self.services.prepare_hdf5_playback();
            self.show_stub_notice("Validating HDF5 streams and preparing HAB playback");
        }
        if review_selected || expert_selected {
            let path = self.services.selected_rrd_path().map(Path::to_owned);
            match path {
                Some(path) => {
                    self.services.clear_audio_evidence();
                    self.rerun_app.open_file_path(path.clone());
                    self.playback_loaded_path = Some(path.clone());
                    self.playback_playing = false;
                    self.playback_last_tick = Instant::now();
                    self.playback_texture = None;
                    self.playback_texture_key = None;
                    self.playback_sub = if expert_selected {
                        PlaybackSub::Rerun
                    } else {
                        PlaybackSub::Canvas
                    };
                    self.pending_blueprint = expert_selected;
                    self.show_stub_notice(format!("Opening {}", path.display()));
                }
                None => self.show_stub_notice("Selected session has no RRD representation"),
            }
        }
    }

    #[allow(dead_code)]
    fn sessions_page_live_legacy(&mut self, ui: &mut egui::Ui) {
        let sessions = self.services.sessions.clone();
        let mut open_selected = false;
        let mut refresh = false;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            page_title(
                                ui,
                                "Recorded Sessions",
                                &format!(
                                    "{} recordings found under data/study_sessions/.",
                                    sessions.len()
                                ),
                            );
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if outline_button(ui, "REFRESH").clicked() {
                                    refresh = true;
                                }
                                if dark_button(ui, "OPEN IN RERUN").clicked() {
                                    open_selected = true;
                                }
                            },
                        );
                    });
                    ui.add_space(20.0);
                    section_label(ui, "SESSIONS");
                    if sessions.is_empty() {
                        notice_card(
                            ui,
                            "No recordings yet",
                            "Use Record on Pipeline or Timeline. Hab will fan the live C++ RerunSink to an indexed .rrd session while visualization continues.",
                        );
                    }
                    for (index, session) in sessions.iter().enumerate() {
                        let selected = self.services.selected_session == index;
                        let row = egui::Button::new(
                            egui::RichText::new(format!(
                                "{}\n{}  ·  {}",
                                session.name,
                                session.format,
                                services::human_bytes(session.size_bytes)
                            ))
                            .size(11.0)
                            .color(TEXT),
                        )
                        .selected(selected)
                        .fill(if selected { INFO_SOFT } else { PAPER })
                        .stroke(egui::Stroke::new(
                            if selected { 1.5 } else { 1.0 },
                            if selected { ACCENT } else { BORDER },
                        ))
                        .corner_radius(7.0)
                        .min_size(egui::vec2(ui.available_width(), 54.0));
                        if ui.add(row).clicked() {
                            self.services.selected_session = index;
                        }
                        ui.label(
                            egui::RichText::new(session.path.to_string_lossy())
                                .size(8.5)
                                .monospace()
                                .color(TERTIARY),
                        );
                        ui.add_space(8.0);
                    }
                });
            });

        if refresh {
            let message = self.services.refresh_sessions();
            self.show_stub_notice(message);
        }
        if open_selected {
            let path = self.services.selected_session_path().map(Path::to_owned);
            match path {
                Some(path) if path.extension().is_some_and(|extension| extension == "rrd") => {
                    self.rerun_app.open_file_path(path.clone());
                    self.playback_sub = PlaybackSub::Rerun;
                    self.pending_blueprint = true;
                    self.show_stub_notice(format!("Opening {}", path.display()));
                }
                Some(path) => self.show_stub_notice(format!(
                    "{} is HDF5; choose an .rrd session for native Rerun playback",
                    path.display()
                )),
                None => self.show_stub_notice("Select a recorded session first"),
            }
        }
    }

    #[allow(dead_code)]
    fn sessions_page(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    page_title(
                        ui,
                        "Recorded Sessions",
                        "45 sessions saved under data/study_sessions/. Pick one to inspect its recorded artefacts.",
                    );
                    ui.add_space(22.0);
                    section_label(ui, "SESSION");
                    info_card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("_runtime_smoke · latest · local")
                                        .size(13.0)
                                        .color(TEXT),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "data/study_sessions/_runtime_smoke/local/latest",
                                    )
                                    .size(10.0)
                                    .monospace()
                                    .color(TERTIARY),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if dark_button(ui, "OPEN SESSION").clicked() {
                                        self.show_stub_notice(
                                            "Use the indexed session list above to open a recording.",
                                        );
                                    }
                                },
                            );
                        });
                    });
                    ui.add_space(16.0);
                    info_card(ui, |ui| {
                        ui.label(
                            egui::RichText::new("_runtime_smoke")
                                .size(16.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Session id: local")
                                .size(11.0)
                                .color(MUTED),
                        );
                        ui.add_space(14.0);
                        ui.horizontal_wrapped(|ui| {
                            for artifact in [
                                "raw_data.csv",
                                "session.h5",
                                "annotations.csv",
                                "session_config.yaml",
                            ] {
                                outlined_chip(ui, artifact);
                            }
                        });
                        ui.add_space(14.0);
                        notice_card(
                            ui,
                            "Native playback stub",
                            "The recorded artefacts are present. The next tranche will bind session.h5 to the C++ replay source and activate the ReRun blueprint.",
                        );
                    });
                });
            });
    }

    fn metrics_page(&mut self, ui: &mut egui::Ui) {
        let metrics = self.services.pipeline_metrics.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ELEVATED)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    page_title(
                        ui,
                        "Metric Data",
                        "Live throughput and latency from the running C++ pipeline monitor.",
                    );
                    ui.add_space(16.0);
                    let Some(metrics) = metrics else {
                        notice_card(
                            ui,
                            "Waiting for pipeline metrics",
                            "Hab will populate this page after the next pipeline_monitor sample.",
                        );
                        return;
                    };

                    let active_streams = metrics
                        .streams
                        .iter()
                        .filter(|stream| stream.frequency_hz > 0.05)
                        .count();
                    let total_samples = metrics
                        .streams
                        .iter()
                        .map(|stream| stream.total_samples)
                        .sum::<u64>();
                    let mean_latency = if metrics.transforms.is_empty() {
                        0.0
                    } else {
                        metrics
                            .transforms
                            .iter()
                            .map(|transform| transform.latency_ms)
                            .sum::<f64>()
                            / metrics.transforms.len() as f64
                    };

                    ui.columns(4, |columns| {
                        metric_summary_card(
                            &mut columns[0],
                            "PIPELINE",
                            &metrics.status.to_uppercase(),
                            SUCCESS,
                        );
                        metric_summary_card(
                            &mut columns[1],
                            "ACTIVE STREAMS",
                            &active_streams.to_string(),
                            ACCENT,
                        );
                        metric_summary_card(
                            &mut columns[2],
                            "TOTAL SAMPLES",
                            &format_compact_count(total_samples),
                            egui::Color32::from_rgb(0x28, 0xad, 0xbe),
                        );
                        metric_summary_card(
                            &mut columns[3],
                            "MEAN LATENCY",
                            &format!("{mean_latency:.2} ms"),
                            egui::Color32::from_rgb(0xdf, 0x67, 0x67),
                        );
                    });

                    ui.add_space(22.0);
                    section_label(ui, "ACTIVE STREAMS");
                    let active = metrics
                        .streams
                        .iter()
                        .filter(|stream| stream.frequency_hz > 0.05)
                        .take(18)
                        .collect::<Vec<_>>();
                    for row in active.chunks(3) {
                        ui.columns(3, |columns| {
                            for (column, stream) in columns.iter_mut().zip(row) {
                                stream_metric_card(column, stream);
                            }
                        });
                        ui.add_space(8.0);
                    }

                    ui.add_space(20.0);
                    section_label(ui, "TRANSFORM LATENCY");
                    info_card(ui, |ui| {
                        egui::Grid::new("hab_transform_metrics")
                            .num_columns(4)
                            .striped(true)
                            .spacing(egui::vec2(24.0, 9.0))
                            .show(ui, |ui| {
                                table_heading(ui, "TRANSFORM");
                                table_heading(ui, "TYPE");
                                table_heading(ui, "LATENCY");
                                table_heading(ui, "JITTER");
                                ui.end_row();
                                for transform in &metrics.transforms {
                                    ui.label(
                                        egui::RichText::new(&transform.id)
                                            .size(11.0)
                                            .strong()
                                            .color(TEXT),
                                    );
                                    ui.label(
                                        egui::RichText::new(&transform.kind)
                                            .size(10.0)
                                            .monospace()
                                            .color(MUTED),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:.2} ms",
                                            transform.latency_ms
                                        ))
                                        .size(10.0)
                                        .monospace()
                                        .color(TEXT),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "± {:.2} ms",
                                            transform.jitter_ms
                                        ))
                                        .size(10.0)
                                        .monospace()
                                        .color(TERTIARY),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
                });
            });
    }

    fn raw_data_page(&mut self, ui: &mut egui::Ui) {
        let metrics = self.services.pipeline_metrics.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ELEVATED)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    page_title(
                        ui,
                        "Raw Data",
                        "Every output channel exposed by the C++ graph, including wiring and live transport statistics.",
                    );
                    ui.add_space(16.0);
                    let Some(metrics) = metrics else {
                        notice_card(
                            ui,
                            "Waiting for the stream catalog",
                            "Raw channels appear after Hab receives the first pipeline_monitor sample.",
                        );
                        return;
                    };
                    ui.horizontal(|ui| {
                        outlined_chip(ui, &format!("{} channels", metrics.streams.len()));
                        outlined_chip(
                            ui,
                            &format!(
                                "{} active",
                                metrics
                                    .streams
                                    .iter()
                                    .filter(|stream| stream.frequency_hz > 0.05)
                                    .count()
                            ),
                        );
                        outlined_chip(ui, "live C++ graph");
                    });
                    ui.add_space(16.0);
                    for stream in &metrics.streams {
                        raw_stream_row(ui, stream);
                        ui.add_space(8.0);
                    }
                });
            });
    }

    fn device_page(&mut self, ui: &mut egui::Ui) {
        let stream_frequency = |name: &str| {
            self.services
                .pipeline_metrics
                .as_ref()
                .and_then(|metrics| metrics.streams.iter().find(|stream| stream.name == name))
                .map_or(0.0, |stream| stream.frequency_hz)
        };
        let camera_ingress_hz = stream_frequency("device_network.camera");
        let camera_processed_hz = stream_frequency("camera_pipeline.frame");
        let audio_ingress_hz = stream_frequency("device_network.audio");
        let audio_processed_hz = stream_frequency("audio_pipeline.audio");
        let camera_active = camera_ingress_hz > 0.05;
        let audio_active = audio_ingress_hz > 0.05;
        let audio_source = self.services.audio_source.clone();
        let audio_format = self.services.audio_sample_format.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ELEVATED)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    page_title(
                        ui,
                        "Device",
                        "External producers, transport health, and sensor capabilities.",
                    );
                    ui.add_space(20.0);
                    info_card(ui, |ui| {
                        ui.horizontal(|ui| {
                            status_dot(
                                ui,
                                if camera_active || audio_active {
                                    SUCCESS
                                } else {
                                    WARNING
                                },
                            );
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("HAB Local Capture Device")
                                        .size(16.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.label(
                                    egui::RichText::new("OpenCV camera + WASAPI microphone producer")
                                        .size(11.0)
                                        .color(MUTED),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if camera_active || audio_active {
                                        status_chip(ui, "STREAMING", SUCCESS, PAPER);
                                    } else if dark_button(ui, "CONNECT DEVICE").clicked() {
                                        self.services.connect_synthetic_device();
                                    }
                                },
                            );
                        });
                        ui.add_space(18.0);
                        key_value(ui, "Host", "127.0.0.1");
                        key_value(ui, "Ingress", "WebSocket :9999");
                        key_value(
                            ui,
                            "Capture process",
                            &self.services.snapshot.device_pid.map_or_else(
                                || "Not recorded".to_owned(),
                                |pid| format!("PID {pid}"),
                            ),
                        );
                        key_value(
                            ui,
                            "Camera",
                            &format!(
                                "{camera_ingress_hz:.1} Hz ingress · {camera_processed_hz:.1} Hz graph"
                            ),
                        );
                        key_value(
                            ui,
                            "Audio",
                            &format!(
                                "{audio_ingress_hz:.1} Hz ingress · {audio_processed_hz:.1} Hz graph"
                            ),
                        );
                        key_value(
                            ui,
                            "Audio source",
                            match audio_source.as_str() {
                                "wasapi" => "WASAPI default capture endpoint",
                                "synthetic" => "Synthetic 440 Hz fallback/test tone",
                                _ => "Waiting for source metadata",
                            },
                        );
                        if !audio_format.is_empty() {
                            key_value(ui, "Audio format", &audio_format);
                        }
                    });
                    ui.add_space(16.0);
                    ui.columns(2, |columns| {
                        sensor_capability_card(
                            &mut columns[0],
                            "LIVE CAMERA",
                            &format!("OpenCV BGR frames · {camera_processed_hz:.1} Hz"),
                            camera_active,
                        );
                        sensor_capability_card(
                            &mut columns[1],
                            "LIVE AUDIO",
                            &format!("PCM envelope + blocks · {audio_processed_hz:.1} Hz"),
                            audio_active,
                        );
                    });
                    ui.add_space(16.0);
                    notice_card(
                        ui,
                        "External producer contract",
                        "The separate OpenCV/WASAPI device publishes camera and audio over the C++ WebSocket ingress. The same samples feed models, Rerun views, and the active recording sink.",
                    );
                });
            });
    }

    fn configs_page_live(&mut self, ui: &mut egui::Ui) {
        let configs = self.services.configs.clone();
        let active_config_name = self
            .services
            .snapshot
            .active_config
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned());
        let running_pipeline = self.services.snapshot.pipeline_name.clone();
        let mut run_selected = false;
        let mut stop_engine = false;
        let mut refresh = false;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        page_title(
                            ui,
                            "Configs",
                            &format!("Browse and run {} C++ graph configurations.", configs.len()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.services.snapshot.connected
                                && outline_button(ui, "STOP").clicked()
                            {
                                stop_engine = true;
                            }
                            if dark_button(ui, "RUN SELECTED").clicked() {
                                run_selected = true;
                            }
                            if outline_button(ui, "REFRESH").clicked() {
                                refresh = true;
                            }
                            let status = if self.services.snapshot.connected {
                                let pid = self
                                    .services
                                    .snapshot
                                    .engine_pid
                                    .map_or_else(|| "PID —".to_owned(), |pid| format!("PID {pid}"));
                                let config = active_config_name
                                    .as_deref()
                                    .or(running_pipeline.as_deref())
                                    .unwrap_or("active graph");
                                format!("RUNNING  ·  {pid}  ·  {config}")
                            } else {
                                "ENGINE STOPPED".to_owned()
                            };
                            status_chip(
                                ui,
                                &status,
                                if self.services.snapshot.connected {
                                    SUCCESS
                                } else {
                                    WARNING
                                },
                                if self.services.snapshot.connected {
                                    SUCCESS_SOFT
                                } else {
                                    PAPER
                                },
                            );
                        });
                    });
                    ui.add_space(20.0);

                    if configs.is_empty() {
                        notice_card(
                            ui,
                            "No pipeline configs found",
                            "Place YAML graph definitions under configs/examples and refresh.",
                        );
                        return;
                    }

                    let gap = 12.0;
                    let columns = (((ui.available_width() + gap) / (360.0 + gap)).floor() as usize)
                        .clamp(1, 6);
                    for (row_index, row) in configs.chunks(columns).enumerate() {
                        ui.columns(columns, |column_uis| {
                            for (column_index, (column, config)) in
                                column_uis.iter_mut().zip(row.iter()).enumerate()
                            {
                                let index = row_index * columns + column_index;
                                let running = self.services.snapshot.connected
                                    && (active_config_name.as_deref() == Some(&config.name)
                                        || running_pipeline.as_deref()
                                            == config
                                                .path
                                                .file_stem()
                                                .and_then(|stem| stem.to_str()));
                                if config_card_live(
                                    column,
                                    config,
                                    self.services.selected_config == index,
                                    running,
                                )
                                .clicked()
                                {
                                    self.services.selected_config = index;
                                }
                            }
                        });
                        ui.add_space(gap);
                    }
                });
            });

        if refresh {
            let message = self.services.refresh_configs();
            self.show_stub_notice(message);
        }
        if run_selected {
            if let Some(path) = self.services.selected_config_path().map(Path::to_owned) {
                self.pipeline.reload(path);
                self.pipeline.request_fit();
            }
            self.services.run_selected_config();
        }
        if stop_engine {
            self.services.stop_engine();
        }
    }

    fn audio_calibration_page(&mut self, ui: &mut egui::Ui) {
        let summary = self.services.audio_golden_summary.clone();
        let campaign = self.services.audio_campaign.clone();
        let campaign_error = self.services.audio_campaign_error.clone();
        let last_case = self.services.audio_golden_last_case.clone();
        let benchmark = self.services.audio_golden_benchmark.clone();
        let capture_progress = self.services.audio_golden_capture_progress();
        let capture_active = capture_progress.is_some() || self.services.audio_golden_saving;
        let mic_live = self.services.audio_timestamp_ns > 0
            && self.services.audio_sample_rate_hz > 0
            && self.services.audio_channels > 0;
        let audio_levels = self
            .services
            .audio_levels
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let selected_split = if self.audio_capture_test_split {
            "test"
        } else {
            "calibration"
        };
        let next_target = summary.next_campaign_target(&campaign, selected_split);
        let mut start_capture = false;
        let mut cancel_capture = false;
        let mut run_benchmark = false;
        let mut play_last = false;
        let mut load_next_target = false;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        page_title(
                            ui,
                            "Audio calibration",
                            "Record private microphone evidence and benchmark the native C++ speech and wake nodes.",
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            status_chip(
                                ui,
                                if mic_live { "MICROPHONE LIVE" } else { "WAITING FOR AUDIO" },
                                if mic_live { SUCCESS } else { WARNING },
                                if mic_live { SUCCESS_SOFT } else { PAPER },
                            );
                            status_chip(
                                ui,
                                "PRIVATE · NOT REDISTRIBUTABLE",
                                ACCENT,
                                INFO_SOFT,
                            );
                        });
                    });
                    ui.add_space(18.0);

                    ui.columns(2, |columns| {
                        columns[0].set_width(columns[0].available_width());
                        info_card(&mut columns[0], |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    section_label(ui, "CAPTURE A LABELED CLIP");
                                    ui.label(
                                        egui::RichText::new("Speak after pressing record")
                                            .size(15.0)
                                            .strong()
                                            .color(TEXT),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        outlined_chip(
                                            ui,
                                            &format!(
                                                "{} HZ · {} CH",
                                                self.services.audio_sample_rate_hz,
                                                self.services.audio_channels
                                            ),
                                        );
                                    },
                                );
                            });
                            ui.add_space(16.0);

                            egui::Frame::new()
                                .fill(INFO_SOFT)
                                .corner_radius(7.0)
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        section_label(ui, "GUIDED PILOT TARGET");
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                status_chip(
                                                    ui,
                                                    if selected_split == "test" {
                                                        "HELD-OUT TEST"
                                                    } else {
                                                        "CALIBRATION"
                                                    },
                                                    ACCENT,
                                                    PAPER,
                                                );
                                            },
                                        );
                                    });
                                    if let Some(target) = &next_target {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Next: {} · {}/{} clips · {}/{} speakers",
                                                target.label,
                                                target.cases,
                                                target.case_target,
                                                target.speakers,
                                                target.speaker_target
                                            ))
                                            .size(12.0)
                                            .strong()
                                            .color(TEXT),
                                        );
                                        ui.label(
                                            egui::RichText::new(&target.instruction)
                                                .size(9.5)
                                                .color(MUTED),
                                        );
                                        ui.add_space(8.0);
                                        if outline_button(ui, "LOAD NEXT TARGET").clicked() {
                                            load_next_target = true;
                                        }
                                    } else {
                                        ui.label(
                                            egui::RichText::new(
                                                "This split meets every configured category target.",
                                            )
                                            .size(10.5)
                                            .strong()
                                            .color(SUCCESS),
                                        );
                                    }
                                });
                            ui.add_space(14.0);

                            ui.add_enabled_ui(!capture_active, |ui| {
                                ui.columns(2, |fields| {
                                    field_label(&mut fields[0], "SPEAKER ID");
                                    fields[0].add(
                                        egui::TextEdit::singleline(&mut self.audio_capture_speaker)
                                            .desired_width(f32::INFINITY),
                                    );
                                    field_label(&mut fields[1], "SESSION ID");
                                    fields[1].add(
                                        egui::TextEdit::singleline(&mut self.audio_capture_session)
                                            .desired_width(f32::INFINITY),
                                    );
                                });
                                let assigned_splits = summary
                                    .assigned_splits(&self.audio_capture_speaker);
                                if !assigned_splits.is_empty() {
                                    let selected = if self.audio_capture_test_split {
                                        "test"
                                    } else {
                                        "calibration"
                                    };
                                    let mismatch = assigned_splits
                                        .iter()
                                        .any(|assigned| assigned != selected);
                                    ui.label(
                                        egui::RichText::new(if mismatch {
                                            format!(
                                                "Speaker is already assigned to {}. Capture will be blocked in {selected}.",
                                                assigned_splits.join(" + ")
                                            )
                                        } else {
                                            format!(
                                                "Stable assignment: {}",
                                                assigned_splits.join(" + ")
                                            )
                                        })
                                        .size(9.0)
                                        .color(if mismatch { WARNING } else { SUCCESS }),
                                    );
                                }
                                ui.add_space(14.0);
                                field_label(ui, "SPLIT");
                                ui.horizontal(|ui| {
                                    if capture_option_button(
                                        ui,
                                        "Calibration",
                                        !self.audio_capture_test_split,
                                    )
                                    .clicked()
                                    {
                                        self.audio_capture_test_split = false;
                                    }
                                    if capture_option_button(
                                        ui,
                                        "Held-out test",
                                        self.audio_capture_test_split,
                                    )
                                    .clicked()
                                    {
                                        self.audio_capture_test_split = true;
                                    }
                                });
                                ui.add_space(14.0);
                                field_label(ui, "EXPECTED EVENT");
                                ui.columns(2, |category_columns| {
                                    for (index, category) in
                                        AudioCaptureCategory::ALL.into_iter().enumerate()
                                    {
                                        let column = &mut category_columns[index % 2];
                                        if audio_category_button(
                                            column,
                                            category,
                                            self.audio_capture_category == category,
                                        )
                                        .clicked()
                                        {
                                            self.audio_capture_category = category;
                                            self.audio_capture_prompt =
                                                category.suggested_prompt().to_owned();
                                            self.audio_capture_condition =
                                                category.suggested_condition().to_owned();
                                        }
                                        if index == 1 {
                                            category_columns[0].add_space(8.0);
                                            category_columns[1].add_space(8.0);
                                        }
                                    }
                                });
                                ui.add_space(14.0);
                                field_label(ui, "PHRASE OR SOUND");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.audio_capture_prompt)
                                        .hint_text(if self.audio_capture_category
                                            == AudioCaptureCategory::Background
                                        {
                                            "Optional: HVAC, typing, silence…"
                                        } else {
                                            "Exact words spoken"
                                        })
                                        .desired_width(f32::INFINITY),
                                );
                                ui.add_space(10.0);
                                field_label(ui, "CAPTURE DIRECTION");
                                ui.add(
                                    egui::TextEdit::multiline(
                                        &mut self.audio_capture_condition,
                                    )
                                    .desired_rows(2)
                                    .desired_width(f32::INFINITY),
                                );
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    field_label(ui, "DURATION");
                                    ui.add(
                                        egui::Slider::new(
                                            &mut self.audio_capture_duration_s,
                                            1.0..=8.0,
                                        )
                                        .suffix(" sec")
                                        .step_by(0.5),
                                    );
                                });
                                ui.add_space(10.0);
                                ui.checkbox(
                                    &mut self.audio_capture_consent,
                                    "I consent to recording this microphone clip for private local model evaluation.",
                                );
                            });

                            ui.add_space(16.0);
                            let (waveform_rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 106.0),
                                egui::Sense::hover(),
                            );
                            paint_audio_waveform(
                                ui.painter(),
                                waveform_rect,
                                &audio_levels,
                                self.services.audio_rms,
                                self.services.audio_peak,
                            );
                            ui.add_space(12.0);

                            if let Some(progress) = capture_progress {
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .animate(true)
                                        .show_percentage()
                                        .text("RECORDING LIVE PCM"),
                                );
                                ui.add_space(8.0);
                                if outline_button(ui, "CANCEL").clicked() {
                                    cancel_capture = true;
                                }
                            } else if self.services.audio_golden_saving {
                                ui.add(
                                    egui::ProgressBar::new(1.0)
                                        .animate(true)
                                        .text("HASHING + SAVING"),
                                );
                            } else if dark_button(ui, "RECORD LABELED CLIP").clicked() {
                                start_capture = true;
                            }

                            if let Some(error) = &self.services.audio_golden_error {
                                ui.add_space(10.0);
                                ui.colored_label(egui::Color32::DARK_RED, error);
                            }
                        });

                        columns[1].vertical(|ui| {
                            info_card(ui, |ui| {
                                ui.horizontal(|ui| {
                                    section_label(ui, "PILOT CAMPAIGN READINESS");
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            status_chip(
                                                ui,
                                                if summary.campaign_ready(&campaign) {
                                                    "PROMOTION INPUT READY"
                                                } else {
                                                    "PILOT GATE INCOMPLETE"
                                                },
                                                if summary.campaign_ready(&campaign) {
                                                    SUCCESS
                                                } else {
                                                    WARNING
                                                },
                                                if summary.campaign_ready(&campaign) {
                                                    SUCCESS_SOFT
                                                } else {
                                                    PAPER
                                                },
                                            );
                                        },
                                    );
                                });
                                let target = campaign.split_target(selected_split);
                                campaign_coverage_row(
                                    ui,
                                    "Wake positives",
                                    summary.category_cases(selected_split, "wake_positive"),
                                    target.minimum_cases_per_category,
                                    summary.category_speakers(selected_split, "wake_positive"),
                                    target.minimum_speakers_per_category,
                                );
                                campaign_coverage_row(
                                    ui,
                                    "Near-wake negatives",
                                    summary.category_cases(
                                        selected_split,
                                        "near_wake_negative",
                                    ),
                                    target.minimum_cases_per_category,
                                    summary.category_speakers(
                                        selected_split,
                                        "near_wake_negative",
                                    ),
                                    target.minimum_speakers_per_category,
                                );
                                campaign_coverage_row(
                                    ui,
                                    "Other speech",
                                    summary.category_cases(selected_split, "other_speech"),
                                    target.minimum_cases_per_category,
                                    summary.category_speakers(selected_split, "other_speech"),
                                    target.minimum_speakers_per_category,
                                );
                                campaign_coverage_row(
                                    ui,
                                    "Background / silence",
                                    summary.category_cases(selected_split, "background"),
                                    target.minimum_cases_per_category,
                                    summary.category_speakers(selected_split, "background"),
                                    target.minimum_speakers_per_category,
                                );
                                ui.separator();
                                key_value(
                                    ui,
                                    "Viewing split",
                                    if selected_split == "test" {
                                        "held-out test"
                                    } else {
                                        "calibration"
                                    },
                                );
                                key_value(ui, "Total private clips", &summary.total_cases.to_string());
                                key_value(
                                    ui,
                                    "Speakers in split",
                                    &summary.split_speakers(selected_split).to_string(),
                                );
                                key_value(
                                    ui,
                                    "Calibration / test",
                                    &format!(
                                        "{} / {}",
                                        summary.calibration_cases, summary.test_cases
                                    ),
                                );
                                key_value(
                                    ui,
                                    "Speaker split",
                                    if summary.speaker_disjoint() {
                                        "disjoint"
                                    } else {
                                        "LEAKAGE DETECTED"
                                    },
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} v{}",
                                        summary.manifest_path.to_string_lossy(),
                                        campaign.name,
                                        campaign.version
                                    ))
                                        .size(8.5)
                                        .monospace()
                                        .color(TERTIARY),
                                );
                                ui.label(
                                    egui::RichText::new(campaign.path.to_string_lossy())
                                        .size(8.5)
                                        .monospace()
                                        .color(TERTIARY),
                                );
                                if let Some(error) = &campaign_error {
                                    ui.colored_label(
                                        WARNING,
                                        format!("Campaign fallback active: {error}"),
                                    );
                                }
                            });
                            ui.add_space(12.0);

                            info_card(ui, |ui| {
                                ui.horizontal(|ui| {
                                    section_label(ui, "NATIVE C++ BENCHMARK");
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if self.services.audio_golden_benchmark_pending {
                                                status_chip(ui, "RUNNING", WARNING, PAPER);
                                            } else if let Some(report) = &benchmark {
                                                status_chip(
                                                    ui,
                                                    if report.promotion_eligible {
                                                        "PILOT GATE PASSED"
                                                    } else {
                                                        "CANDIDATE ONLY"
                                                    },
                                                    if report.promotion_eligible {
                                                        SUCCESS
                                                    } else {
                                                        WARNING
                                                    },
                                                    if report.promotion_eligible {
                                                        SUCCESS_SOFT
                                                    } else {
                                                        PAPER
                                                    },
                                                );
                                            }
                                        },
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(
                                        "Runs recorded WAVs and the licensed baseline through the same Silero and Vosk nodes used by inspector_live.",
                                    )
                                    .size(10.5)
                                    .color(MUTED),
                                );
                                ui.add_space(12.0);
                                if let Some(report) = &benchmark {
                                    benchmark_metric_row(ui, "Speech", &report.speech);
                                    benchmark_metric_row(ui, "Wake", &report.wake);
                                    key_value(ui, "Evaluated cases", &report.total_cases.to_string());
                                    key_value(
                                        ui,
                                        "Speaker split",
                                        if report.speaker_disjoint {
                                            "disjoint"
                                        } else {
                                            "LEAKAGE DETECTED"
                                        },
                                    );
                                    key_value(
                                        ui,
                                        "Threshold status",
                                        if report.promotion_eligible {
                                            "eligible for pilot promotion review"
                                        } else {
                                            "candidate only"
                                        },
                                    );
                                    if !report.promotion_eligible {
                                        for reason in report.promotion_reasons.iter().take(3) {
                                            ui.label(
                                                egui::RichText::new(format!("• {reason}"))
                                                    .size(8.5)
                                                    .color(MUTED),
                                            );
                                        }
                                        if report.promotion_reasons.len() > 3 {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "+{} more requirements in report",
                                                    report.promotion_reasons.len() - 3
                                                ))
                                                .size(8.5)
                                                .color(TERTIARY),
                                            );
                                        }
                                    }
                                    ui.label(
                                        egui::RichText::new(report.report_path.to_string_lossy())
                                            .size(8.5)
                                            .monospace()
                                            .color(TERTIARY),
                                    );
                                    ui.add_space(10.0);
                                }
                                if !self.services.audio_golden_benchmark_pending
                                    && dark_button(ui, "RUN NATIVE BENCHMARK").clicked()
                                {
                                    run_benchmark = true;
                                }
                            });

                            if let Some(case) = &last_case {
                                ui.add_space(12.0);
                                info_card(ui, |ui| {
                                    section_label(ui, "LAST SAVED CLIP");
                                    ui.label(
                                        egui::RichText::new(&case.prompt)
                                            .size(13.0)
                                            .strong()
                                            .color(TEXT),
                                    );
                                    key_value(ui, "Speaker", &case.speaker_id);
                                    key_value(ui, "Split", &case.split);
                                    key_value(ui, "Category", &case.category);
                                    if !case.condition.is_empty() {
                                        key_value(ui, "Condition", &case.condition);
                                    }
                                    key_value(ui, "Duration", &format!("{:.2} s", case.duration_s));
                                    if outline_button(ui, "PLAY LAST CLIP").clicked() {
                                        play_last = true;
                                    }
                                });
                            }
                        });
                    });
                    ui.add_space(16.0);
                    notice_card(
                        ui,
                        "Keep speakers disjoint",
                        "A speaker ID is blocked from crossing splits. Threshold candidates use the licensed baseline plus calibration clips; held-out test clips are reserved for promotion evidence. Passing this configurable pilot gate is not production validation. Private recordings remain ignored by Git.",
                    );
                });
            });

        if load_next_target
            && let Some(target) = next_target
            && let Some(category) = AudioCaptureCategory::from_key(&target.category)
        {
            self.audio_capture_category = category;
            self.audio_capture_prompt = target.prompt;
            self.audio_capture_condition = target.instruction;
        }
        if start_capture {
            let request = services::AudioGoldenCaptureRequest {
                speaker_id: self.audio_capture_speaker.clone(),
                session_id: self.audio_capture_session.clone(),
                split: if self.audio_capture_test_split {
                    "test"
                } else {
                    "calibration"
                }
                .to_owned(),
                category: self.audio_capture_category.key().to_owned(),
                prompt: self.audio_capture_prompt.clone(),
                condition: self.audio_capture_condition.clone(),
                duration_ms: (self.audio_capture_duration_s * 1_000.0).round() as u64,
                consent: self.audio_capture_consent,
            };
            match self.services.start_audio_golden_capture(request) {
                Ok(()) => self.audio_capture_consent = false,
                Err(error) => {
                    self.show_stub_notice(format!("Audio capture not started: {error}"));
                }
            }
        }
        if cancel_capture && self.services.cancel_audio_golden_capture() {
            self.show_stub_notice("Audio calibration capture canceled");
        }
        if run_benchmark && let Err(error) = self.services.run_audio_golden_benchmark() {
            self.show_stub_notice(format!("Benchmark not started: {error}"));
        }
        if play_last && let Some(case) = last_case {
            match play_wav_file(&case.wav_path) {
                Ok(()) => self.show_stub_notice("Playing the last private calibration clip"),
                Err(error) => self.show_stub_notice(error),
            }
        }
    }

    #[allow(dead_code)]
    fn configs_page_live_legacy(&mut self, ui: &mut egui::Ui) {
        let configs = self.services.configs.clone();
        let mut run_selected = false;
        let mut stop_engine = false;
        let mut refresh = false;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            page_title(
                                ui,
                                "Configs",
                                &format!(
                                    "Browse and run {} engine pipeline configurations.",
                                    configs.len()
                                ),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if dark_button(ui, "RUN SELECTED").clicked() {
                                run_selected = true;
                            }
                            if outline_button(ui, "REFRESH").clicked() {
                                refresh = true;
                            }
                            if self.services.snapshot.connected
                                && outline_button(ui, "STOP ENGINE").clicked()
                            {
                                stop_engine = true;
                            }
                            status_chip(
                                ui,
                                if self.services.snapshot.connected {
                                    "ENGINE RUNNING"
                                } else {
                                    "ENGINE STOPPED"
                                },
                                if self.services.snapshot.connected {
                                    SUCCESS
                                } else {
                                    WARNING
                                },
                                SUCCESS_SOFT,
                            );
                        });
                    });
                    ui.add_space(20.0);
                    for (row_index, row) in configs.chunks(3).enumerate() {
                        ui.columns(3, |columns| {
                            for (column_index, (column, config)) in
                                columns.iter_mut().zip(row.iter()).enumerate()
                            {
                                let index = row_index * 3 + column_index;
                                if config_card(
                                    column,
                                    &config.name,
                                    &config.summary,
                                    &services::human_bytes(config.size_bytes),
                                    self.services.selected_config == index,
                                )
                                .clicked()
                                {
                                    self.services.selected_config = index;
                                }
                            }
                        });
                        ui.add_space(12.0);
                    }
                });
            });

        if refresh {
            let message = self.services.refresh_configs();
            self.show_stub_notice(message);
        }
        if run_selected {
            if let Some(path) = self.services.selected_config_path().map(Path::to_owned) {
                self.pipeline.reload(path);
                self.pipeline.request_fit();
            }
            self.services.run_selected_config();
        }
        if stop_engine {
            self.services.stop_engine();
        }
    }

    #[allow(dead_code)]
    fn configs_page(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            page_title(
                                ui,
                                "Configs",
                                "Browse available engine pipelines and run one.",
                            );
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if dark_button(ui, "RUN SELECTED").clicked() {
                                    self.services.run_selected_config();
                                }
                                if outline_button(ui, "REFRESH").clicked() {
                                    let message = self.services.refresh_configs();
                                    self.show_stub_notice(message);
                                }
                                status_chip(
                                    ui,
                                    if self.recording_connected() {
                                        "ENGINE RUNNING"
                                    } else {
                                        "ENGINE STOPPED"
                                    },
                                    if self.recording_connected() {
                                        SUCCESS
                                    } else {
                                        WARNING
                                    },
                                    SUCCESS_SOFT,
                                );
                            },
                        );
                    });
                    ui.add_space(20.0);

                    let configs = [
                        (
                            "inspector_live.yaml",
                            "Live camera, audio, grasp, scene, speech, and wake-event inspection.",
                            "2.2 KB",
                        ),
                        (
                            "multimodal_recording_smoke.yaml",
                            "Synchronized IMU, PPG, camera, and audio recording acceptance pipeline.",
                            "1.2 KB",
                        ),
                        (
                            "network_device_stream.yaml",
                            "Wi-Fi/local TCP camera and audio device acceptance pipeline.",
                            "0.8 KB",
                        ),
                        (
                            "activity_replay.yaml",
                            "Replay captured sensor data through the engine as a live source.",
                            "2.1 KB",
                        ),
                        (
                            "heart_rate_smoke.yaml",
                            "PPG, accelerometer, gyro, and live heart-rate stream smoke test.",
                            "4.2 KB",
                        ),
                        (
                            "study_recording.yaml",
                            "Capture a device session to HDF5 with annotations and metadata.",
                            "12.2 KB",
                        ),
                    ];

                    for row in configs.chunks(3) {
                        ui.columns(3, |columns| {
                            for (column, config) in columns.iter_mut().zip(row.iter()) {
                                let _ = config_card(
                                    column,
                                    config.0,
                                    config.1,
                                    config.2,
                                    false,
                                );
                            }
                        });
                        ui.add_space(12.0);
                    }
                });
            });
    }

    fn toast(&mut self, ctx: &egui::Context) {
        if self
            .stub_notice
            .as_ref()
            .is_some_and(|(_, shown_at)| shown_at.elapsed() > Duration::from_secs(4))
        {
            self.stub_notice = None;
            return;
        }
        let Some((message, _)) = self.stub_notice.as_ref() else {
            return;
        };

        egui::Area::new(egui::Id::new("hab_stub_notice"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(TEXT)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(message)
                                .size(11.0)
                                .color(egui::Color32::WHITE),
                        );
                    });
            });
    }
}

impl Drop for HabShell {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl eframe::App for HabShell {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.rerun_app.clear_color(visuals)
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.rerun_app.save(storage);
    }

    fn logic(&mut self, egui_ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.rerun_app.logic(egui_ctx, frame);
    }

    fn on_exit(&mut self) {
        self.shutdown();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(
            if self.workspace == Workspace::Timeline
                || self.workspace == Workspace::Live
                || (self.workspace == Workspace::Playback
                    && self.playback_sub == PlaybackSub::Canvas)
                || (self.workspace == Workspace::Configs
                    && self.config_sub == ConfigSub::AudioCalibration
                    && self.services.audio_golden_capture_progress().is_some())
            {
                50
            } else {
                250
            },
        ));
        for notice in self.services.poll() {
            self.show_stub_notice(notice);
        }
        if let Some(prepared) = self.services.take_prepared_playback() {
            self.services.clear_audio_evidence();
            self.rerun_app.open_file_path(prepared.rrd_path);
            self.playback_loaded_path = Some(prepared.source_path);
            self.playback_playing = false;
            self.playback_last_tick = Instant::now();
            self.playback_texture = None;
            self.playback_texture_key = None;
            self.playback_sub = PlaybackSub::Canvas;
            self.pending_blueprint = false;
        }
        self.services.maybe_probe();
        self.masthead(ui);
        self.navigation(ui);
        self.subnavigation(ui);
        self.page_header(ui);
        self.pipeline_inspector(ui);

        if let Some(error) = &self.last_blueprint_error {
            egui::Panel::top("hab_blueprint_error")
                .exact_size(30.0)
                .show_inside(ui, |ui| {
                    ui.colored_label(egui::Color32::DARK_RED, error);
                });
        }

        if self.workspace == Workspace::Timeline {
            self.timeline_event_feed(ui);
        }
        if !self.show_custom_page(ui) {
            self.install_pending_blueprint();
            self.rerun_app.ui(ui, frame);
        }
        self.toast(ui.ctx());
    }
}

fn workspace_uses_rerun_view(workspace: Workspace, playback_sub: PlaybackSub) -> bool {
    matches!(
        (workspace, playback_sub),
        (Workspace::Playback, PlaybackSub::Rerun) | (Workspace::Streams, _)
    )
}

fn blueprint_for(
    workspace: Workspace,
    playback_sub: PlaybackSub,
    stream_sub: StreamSub,
) -> Blueprint {
    use re_sdk_types::blueprint::components::{PanelState, PlayState};

    let camera = || {
        Spatial2DView::new("Camera")
            .with_origin("sensors/camera/front")
            .with_contents(["$origin"])
    };
    let audio = || {
        TimeSeriesView::new("Audio level")
            .with_origin("sensors/audio/microphone/rms")
            .with_contents(["$origin"])
    };
    let all_sensors = || {
        TimeSeriesView::new("Line plot")
            .with_origin("sensors")
            .with_contents(["$origin/**"])
    };
    let spatial_sensors = || {
        Spatial3DView::new("Spatial sensor view")
            .with_origin("sensors")
            .with_contents(["$origin/**"])
    };
    let timeline = || {
        StateTimelineView::new("Timeline events")
            .with_origin("events")
            .with_contents(["$origin/**"])
    };
    let event_log = || {
        TextLogView::new("Event details")
            .with_origin("events/log")
            .with_contents(["$origin"])
    };
    let graph = || {
        GraphView::new("Pipeline")
            .with_origin("pipeline/inspector_live")
            .with_contents(["$origin"])
    };

    let root: ContainerLike = match workspace {
        Workspace::Pipeline => graph().into(),
        Workspace::Timeline => Vertical::new([
            Horizontal::new([camera().into(), audio().into()])
                .with_column_shares(vec![1.35, 1.0])
                .into(),
            Horizontal::new([timeline().into(), event_log().into()])
                .with_column_shares(vec![1.35, 1.0])
                .into(),
        ])
        .with_name("Timeline")
        .with_row_shares(vec![1.0, 1.35])
        .into(),
        Workspace::Playback => match playback_sub {
            PlaybackSub::Sessions => Vertical::new([timeline().into(), event_log().into()])
                .with_row_shares(vec![2.0, 1.0])
                .into(),
            PlaybackSub::Canvas => Horizontal::new([all_sensors().into(), event_log().into()])
                .with_column_shares(vec![3.0, 2.0])
                .into(),
            PlaybackSub::Rerun => Vertical::new([
                Horizontal::new([camera().into(), timeline().into()])
                    .with_column_shares(vec![1.0, 1.35])
                    .into(),
                audio().into(),
            ])
            .with_row_shares(vec![3.0, 1.0])
            .into(),
        },
        Workspace::Streams => match stream_sub {
            StreamSub::Event => timeline().into(),
            StreamSub::Video => camera().into(),
            StreamSub::Imu | StreamSub::Body | StreamSub::BodyModel => spatial_sensors().into(),
            StreamSub::Line | StreamSub::Scatter => all_sensors().into(),
        },
        Workspace::Metrics => all_sensors().into(),
        Workspace::RawData => event_log().into(),
        Workspace::Live => Horizontal::new([
            Vertical::new([camera().into(), audio().into()])
                .with_row_shares(vec![3.0, 1.0])
                .into(),
            Vertical::new([timeline().into(), event_log().into()])
                .with_row_shares(vec![2.0, 1.0])
                .into(),
        ])
        .with_name("Live")
        .with_column_shares(vec![3.0, 2.0])
        .into(),
        Workspace::Device => Vertical::new([camera().into(), audio().into()])
            .with_row_shares(vec![3.0, 1.0])
            .into(),
        Workspace::Configs => graph().into(),
    };

    let time_panel_state = if matches!(
        (workspace, playback_sub),
        (Workspace::Live, _)
            | (
                Workspace::Playback,
                PlaybackSub::Canvas | PlaybackSub::Rerun
            )
    ) {
        PanelState::Expanded
    } else {
        PanelState::Hidden
    };

    Blueprint::new(root)
        .with_auto_layout(false)
        .with_auto_views(false)
        .with_blueprint_panel(BlueprintPanel::from_state(PanelState::Hidden))
        .with_selection_panel(SelectionPanel::from_state(PanelState::Hidden))
        .with_time_panel(
            TimePanel::new()
                .with_state(time_panel_state)
                .with_timeline("wallclock")
                .with_play_state(PlayState::Following),
        )
}

fn nav_button(ui: &mut egui::Ui, icon: NavIcon, label: &str, selected: bool) -> egui::Response {
    let color = if selected { TEXT } else { MUTED };
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), egui::FontId::proportional(11.0), color);
    let width = nav_button_width(ui, label);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.painter()
            .rect_filled(rect.shrink2(egui::vec2(3.0, 4.0)), 5.0, ELEVATED);
    }
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 14.0 + 7.0, rect.center().y),
        egui::vec2(14.0, 14.0),
    );
    paint_nav_icon(ui.painter(), icon, icon_rect, color);
    ui.painter().galley(
        egui::pos2(
            icon_rect.right() + 8.0,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
    if selected {
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + 10.0, rect.bottom() - 1.0),
                egui::pos2(rect.right() - 10.0, rect.bottom() - 1.0),
            ],
            egui::Stroke::new(2.0, TEXT),
        );
    }
    response
}

fn nav_button_width(ui: &egui::Ui, label: &str) -> f32 {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), egui::FontId::proportional(11.0), MUTED);
    14.0 + 8.0 + galley.size().x + 28.0
}

fn paint_nav_icon(painter: &egui::Painter, icon: NavIcon, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.35, color);
    let x = |fraction: f32| rect.left() + rect.width() * fraction;
    let y = |fraction: f32| rect.top() + rect.height() * fraction;
    match icon {
        NavIcon::Pipeline => {
            let center = egui::pos2(x(0.50), y(0.50));
            let nodes = [
                egui::pos2(x(0.18), y(0.22)),
                egui::pos2(x(0.82), y(0.22)),
                egui::pos2(x(0.18), y(0.78)),
                egui::pos2(x(0.82), y(0.78)),
            ];
            for node in nodes {
                painter.line_segment([center, node], stroke);
                painter.circle_filled(node, 1.6, color);
            }
            painter.circle_stroke(center, 2.4, stroke);
        }
        NavIcon::Timeline => {
            painter.line_segment(
                [egui::pos2(x(0.12), y(0.82)), egui::pos2(x(0.88), y(0.82))],
                stroke,
            );
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(x(0.12), y(0.66)),
                    egui::pos2(x(0.32), y(0.46)),
                    egui::pos2(x(0.50), y(0.58)),
                    egui::pos2(x(0.72), y(0.24)),
                    egui::pos2(x(0.88), y(0.34)),
                ],
                stroke,
            ));
        }
        NavIcon::Playback => {
            painter.circle_stroke(rect.center(), rect.width() * 0.39, stroke);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(x(0.43), y(0.32)),
                    egui::pos2(x(0.43), y(0.68)),
                    egui::pos2(x(0.70), y(0.50)),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        NavIcon::Streams => {
            painter.line_segment(
                [egui::pos2(x(0.15), y(0.84)), egui::pos2(x(0.15), y(0.18))],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(x(0.15), y(0.84)), egui::pos2(x(0.88), y(0.84))],
                stroke,
            );
            let points = [
                egui::pos2(x(0.28), y(0.66)),
                egui::pos2(x(0.49), y(0.42)),
                egui::pos2(x(0.66), y(0.54)),
                egui::pos2(x(0.86), y(0.24)),
            ];
            painter.add(egui::Shape::line(points.to_vec(), stroke));
            for point in points {
                painter.circle_filled(point, 1.35, color);
            }
        }
        NavIcon::Metrics => {
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(x(0.08), y(0.56)),
                    egui::pos2(x(0.28), y(0.56)),
                    egui::pos2(x(0.39), y(0.30)),
                    egui::pos2(x(0.54), y(0.76)),
                    egui::pos2(x(0.67), y(0.44)),
                    egui::pos2(x(0.91), y(0.44)),
                ],
                stroke,
            ));
        }
        NavIcon::RawData => {
            for index in 0..3 {
                let top = y(0.14 + index as f32 * 0.27);
                let layer = egui::Rect::from_min_max(
                    egui::pos2(x(0.15), top),
                    egui::pos2(x(0.85), top + rect.height() * 0.20),
                );
                painter.rect_stroke(layer, 2.0, stroke, egui::StrokeKind::Inside);
            }
        }
        NavIcon::Live => {
            painter.circle_stroke(rect.center(), rect.width() * 0.39, stroke);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(x(0.43), y(0.32)),
                    egui::pos2(x(0.43), y(0.68)),
                    egui::pos2(x(0.70), y(0.50)),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        NavIcon::Device => {
            let chip = rect.shrink(2.8);
            painter.rect_stroke(chip, 2.0, stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(chip.shrink(3.0), 1.0, stroke, egui::StrokeKind::Inside);
            for fraction in [0.28, 0.50, 0.72] {
                painter.line_segment(
                    [
                        egui::pos2(x(fraction), rect.top()),
                        egui::pos2(x(fraction), chip.top()),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        egui::pos2(x(fraction), chip.bottom()),
                        egui::pos2(x(fraction), rect.bottom()),
                    ],
                    stroke,
                );
            }
        }
        NavIcon::Configs => {
            for (fraction, knob) in [(0.24, 0.35), (0.50, 0.70), (0.76, 0.48)] {
                painter.line_segment(
                    [
                        egui::pos2(x(0.12), y(fraction)),
                        egui::pos2(x(0.88), y(fraction)),
                    ],
                    stroke,
                );
                painter.circle_filled(egui::pos2(x(knob), y(fraction)), 2.0, color);
            }
        }
    }
}

fn subnav_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let response = ui.add_sized(
        egui::vec2(108.0, 37.0),
        egui::Button::new(
            egui::RichText::new(label.to_uppercase())
                .size(9.0)
                .strong()
                .color(if selected { TEXT } else { TERTIARY })
                .extra_letter_spacing(1.0),
        )
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .frame(false),
    );
    if selected {
        ui.painter().line_segment(
            [
                egui::pos2(response.rect.left() + 10.0, response.rect.bottom() - 1.0),
                egui::pos2(response.rect.right() - 10.0, response.rect.bottom() - 1.0),
            ],
            egui::Stroke::new(2.0, ACCENT),
        );
    }
    response
}

fn stream_title(stream: StreamSub) -> &'static str {
    match stream {
        StreamSub::Line => "Line plot",
        StreamSub::Scatter => "Scatter plot",
        StreamSub::Event => "Event timeline",
        StreamSub::Imu => "IMU orientation",
        StreamSub::Body => "Body orientation",
        StreamSub::BodyModel => "Body model",
        StreamSub::Video => "Video",
    }
}

fn stream_description(stream: StreamSub) -> &'static str {
    match stream {
        StreamSub::Line => {
            "Time-series view of a numerical-array stream. Best for IMU axes, classifier softmaxes, and raw signals."
        }
        StreamSub::Scatter => "Two-dimensional relationship view for paired numerical streams.",
        StreamSub::Event => "Lane-per-name event timeline aligned to the source data.",
        StreamSub::Imu => "Orientation and motion visualization driven by inertial streams.",
        StreamSub::Body => "Human-centered pose and orientation visualization.",
        StreamSub::BodyModel => "Three-dimensional model view driven by tracked body signals.",
        StreamSub::Video => "Live or recorded camera stream with synchronized overlays.",
    }
}

fn media_card(ui: &mut egui::Ui, eyebrow: &str, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            ui.label(
                egui::RichText::new(eyebrow)
                    .size(9.0)
                    .strong()
                    .color(TERTIARY)
                    .extra_letter_spacing(1.3),
            );
            ui.add_space(5.0);
            content(ui);
        });
}

fn paint_empty_media(painter: &egui::Painter, rect: egui::Rect, icon: NavIcon, message: &str) {
    let icon_rect = egui::Rect::from_center_size(
        rect.center() - egui::vec2(0.0, 12.0),
        egui::vec2(22.0, 22.0),
    );
    paint_nav_icon(painter, icon, icon_rect, egui::Color32::from_gray(120));
    painter.text(
        rect.center() + egui::vec2(0.0, 17.0),
        egui::Align2::CENTER_CENTER,
        message,
        egui::FontId::proportional(11.0),
        egui::Color32::from_gray(150),
    );
}

fn amplitude_to_dbfs(amplitude: f32) -> f32 {
    20.0 * amplitude.max(0.000_001).log10()
}

#[cfg(target_os = "windows")]
fn play_wav_file(path: &Path) -> Result<(), String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt as _};

    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_FILENAME: u32 = 0x0002_0000;
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, module: *mut c_void, flags: u32) -> i32;
    }

    if !path.is_file() {
        return Err(format!("Audio evidence is missing: {}", path.display()));
    }
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `wide` is a valid, NUL-terminated Windows path for the duration
    // of the call; SND_FILENAME makes winmm copy/open the named WAV before the
    // asynchronous playback continues. No module handle is required.
    let accepted = unsafe {
        PlaySoundW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            SND_ASYNC | SND_NODEFAULT | SND_FILENAME,
        )
    };
    if accepted == 0 {
        Err(format!("Windows could not play {}", path.display()))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn play_wav_file(_path: &Path) -> Result<(), String> {
    Err("Audio evidence playback is currently available on Windows".to_owned())
}

#[cfg(target_os = "windows")]
fn stop_wav_playback() {
    use std::ffi::c_void;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, module: *mut c_void, flags: u32) -> i32;
    }
    // SAFETY: the documented null sound pointer stops the current PlaySound
    // waveform; both the module handle and flags are unused for this operation.
    let _ = unsafe { PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0) };
}

#[cfg(not(target_os = "windows"))]
fn stop_wav_playback() {}

fn paint_audio_waveform(
    painter: &egui::Painter,
    rect: egui::Rect,
    levels: &[f32],
    rms: f32,
    peak: f32,
) {
    painter.rect_filled(rect, 7.0, egui::Color32::from_rgb(248, 247, 252));
    painter.rect_stroke(
        rect,
        7.0,
        egui::Stroke::new(1.0, BORDER_SOFT),
        egui::StrokeKind::Inside,
    );
    for fraction in [0.25, 0.5, 0.75] {
        let y = rect.top() + rect.height() * fraction;
        painter.line_segment(
            [
                egui::pos2(rect.left() + 8.0, y),
                egui::pos2(rect.right() - 8.0, y),
            ],
            egui::Stroke::new(0.7, egui::Color32::from_rgb(230, 228, 237)),
        );
    }
    if levels.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Waiting for audio samples",
            egui::FontId::proportional(11.0),
            TERTIARY,
        );
        return;
    }

    let horizontal_padding = 10.0;
    let usable_width = rect.width() - horizontal_padding * 2.0;
    let center_y = rect.center().y;
    let max_amplitude = rect.height() * 0.40;
    let envelope = levels
        .iter()
        .enumerate()
        .map(|(index, level)| {
            let fraction = index as f32 / (levels.len() - 1) as f32;
            let normalized = ((amplitude_to_dbfs(*level) + 60.0) / 60.0).clamp(0.015, 1.0);
            (
                rect.left() + horizontal_padding + usable_width * fraction,
                max_amplitude * normalized,
            )
        })
        .collect::<Vec<_>>();
    let mut area = envelope
        .iter()
        .map(|(x, amplitude)| egui::pos2(*x, center_y - amplitude))
        .collect::<Vec<_>>();
    area.extend(
        envelope
            .iter()
            .rev()
            .map(|(x, amplitude)| egui::pos2(*x, center_y + amplitude)),
    );
    painter.add(egui::Shape::convex_polygon(
        area,
        egui::Color32::from_rgba_unmultiplied(109, 40, 217, 44),
        egui::Stroke::new(1.2, ACCENT),
    ));
    painter.line_segment(
        [
            egui::pos2(rect.left() + horizontal_padding, center_y),
            egui::pos2(rect.right() - horizontal_padding, center_y),
        ],
        egui::Stroke::new(0.8, egui::Color32::from_rgb(210, 205, 223)),
    );

    let rms_fraction = ((amplitude_to_dbfs(rms) + 60.0) / 60.0).clamp(0.0, 1.0);
    let meter = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - 4.0),
        egui::pos2(rect.left() + rect.width() * rms_fraction, rect.bottom()),
    );
    painter.rect_filled(meter, 2.0, ACCENT);
    let peak_fraction = ((amplitude_to_dbfs(peak) + 60.0) / 60.0).clamp(0.0, 1.0);
    let peak_x = rect.left() + rect.width() * peak_fraction;
    painter.line_segment(
        [
            egui::pos2(peak_x, rect.bottom() - 8.0),
            egui::pos2(peak_x, rect.bottom()),
        ],
        egui::Stroke::new(1.5, egui::Color32::from_rgb(224, 92, 89)),
    );
}

fn playback_units_per_second(timeline: &str, min: i64, max: i64) -> f64 {
    let name = timeline.to_ascii_lowercase();
    if name.contains("time")
        || name.contains("timestamp")
        || min.unsigned_abs() > 1_000_000_000_000
        || max.saturating_sub(min).unsigned_abs() > 100_000_000
    {
        1_000_000_000.0
    } else {
        30.0
    }
}

fn format_playback_duration(timeline: &str, min: i64, max: i64) -> String {
    let units = playback_units_per_second(timeline, min, max);
    format_clock_seconds(max.saturating_sub(min) as f64 / units)
}

fn format_playback_position(timeline: &str, min: i64, current: i64) -> String {
    let units = playback_units_per_second(timeline, min, current);
    format_clock_seconds(current.saturating_sub(min) as f64 / units)
}

fn format_clock_seconds(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    let whole = seconds.floor() as u64;
    let millis = ((seconds.fract() * 1_000.0).round() as u64).min(999);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        whole / 3_600,
        (whole / 60) % 60,
        whole % 60,
        millis
    )
}

fn playback_lane_color(entity: &str) -> egui::Color32 {
    let seed = entity.bytes().fold(0_u32, |value, byte| {
        value.wrapping_mul(33).wrapping_add(byte as u32)
    });
    let palette = [
        ACCENT,
        egui::Color32::from_rgb(0x19, 0x89, 0x91),
        egui::Color32::from_rgb(0xd6, 0x72, 0x22),
        egui::Color32::from_rgb(0x38, 0x86, 0x41),
        egui::Color32::from_rgb(0xc1, 0x3f, 0x70),
    ];
    palette[seed as usize % palette.len()]
}

fn playback_scrubber(ui: &mut egui::Ui, fraction: f32) -> Option<f32> {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 18.0),
        egui::Sense::click_and_drag(),
    );
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    ui.painter()
        .rect_filled(track, 2.0, egui::Color32::from_rgb(224, 222, 230));
    let fraction = fraction.clamp(0.0, 1.0);
    let cursor_x = egui::lerp(track.x_range(), fraction);
    let progress = egui::Rect::from_min_max(track.left_top(), egui::pos2(cursor_x, track.bottom()));
    ui.painter().rect_filled(progress, 2.0, ACCENT);
    ui.painter()
        .circle_filled(egui::pos2(cursor_x, track.center().y), 5.5, PAPER);
    ui.painter().circle_stroke(
        egui::pos2(cursor_x, track.center().y),
        5.5,
        egui::Stroke::new(1.5, ACCENT),
    );
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        Some(((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0))
    } else {
        None
    }
}

fn playback_entity_lanes(ui: &mut egui::Ui, entities: &[String], min: i64, max: i64, current: i64) {
    let visible = entities
        .iter()
        .filter(|entity| entity.as_str() != "/")
        .take(5)
        .collect::<Vec<_>>();
    if visible.is_empty() {
        ui.label(
            egui::RichText::new("No temporal entities were found in this recording.")
                .size(10.0)
                .color(MUTED),
        );
        return;
    }
    let span = max.saturating_sub(min).max(1);
    let cursor_fraction = (current.saturating_sub(min) as f64 / span as f64).clamp(0.0, 1.0) as f32;
    for entity in visible {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(178.0, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new(entity)
                            .size(8.5)
                            .monospace()
                            .color(MUTED),
                    );
                },
            );
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 10.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 5.0, egui::Color32::from_rgb(238, 237, 242));
            let color = playback_lane_color(entity);
            ui.painter()
                .rect_filled(rect.shrink(2.0), 4.0, color.gamma_multiply(0.38));
            let x = egui::lerp(rect.x_range(), cursor_fraction);
            ui.painter().line_segment(
                [
                    egui::pos2(x, rect.top() - 3.0),
                    egui::pos2(x, rect.bottom() + 3.0),
                ],
                egui::Stroke::new(1.4, TEXT),
            );
        });
    }
}

fn event_activity_card(ui: &mut egui::Ui, events: &[services::TimelineEvent]) {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("LIVE MODEL ACTIVITY")
                            .size(9.0)
                            .strong()
                            .color(TERTIARY)
                            .extra_letter_spacing(1.3),
                    );
                    ui.label(
                        egui::RichText::new("Event timeline")
                            .size(14.0)
                            .strong()
                            .color(TEXT),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    outlined_chip(ui, "LAST 30 SEC");
                });
            });
            ui.add_space(10.0);

            let newest_timestamp = events
                .iter()
                .map(|event| event.timestamp_ns)
                .max()
                .unwrap_or_default();
            let window_ns = 30_000_000_000_i64;
            let start_timestamp = newest_timestamp.saturating_sub(window_ns);
            for kind in ["grasp", "scene", "speech", "wake"] {
                let color = timeline_kind_color(kind);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(76.0, 32.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            status_dot(ui, color);
                            ui.label(
                                egui::RichText::new(kind.to_uppercase())
                                    .size(9.0)
                                    .strong()
                                    .color(MUTED)
                                    .extra_letter_spacing(0.8),
                            );
                        },
                    );
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 32.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, 5.0, PANEL);
                    ui.painter().line_segment(
                        [
                            egui::pos2(rect.left() + 8.0, rect.center().y),
                            egui::pos2(rect.right() - 8.0, rect.center().y),
                        ],
                        egui::Stroke::new(1.0, BORDER),
                    );
                    for event in events
                        .iter()
                        .filter(|event| event.kind == kind && event.timestamp_ns >= start_timestamp)
                    {
                        let fraction = if newest_timestamp > start_timestamp {
                            (event.timestamp_ns.saturating_sub(start_timestamp) as f64
                                / (newest_timestamp - start_timestamp) as f64)
                                .clamp(0.0, 1.0) as f32
                        } else {
                            1.0
                        };
                        let position = egui::pos2(
                            rect.left() + 8.0 + (rect.width() - 16.0) * fraction,
                            rect.center().y,
                        );
                        ui.painter().circle_filled(position, 3.2, color);
                        ui.painter().circle_stroke(
                            position,
                            5.2,
                            egui::Stroke::new(1.0, color.gamma_multiply(0.35)),
                        );
                    }
                });
                ui.add_space(5.0);
            }
        });
}

fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(egui::RichText::new(title).size(21.0).strong().color(TEXT));
    ui.add_space(2.0);
    ui.label(egui::RichText::new(subtitle).size(11.0).color(MUTED));
}

fn section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(9.0)
            .strong()
            .color(TERTIARY)
            .extra_letter_spacing(1.1),
    );
    ui.add_space(8.0);
}

fn field_label(ui: &mut egui::Ui, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(8.5)
            .strong()
            .color(TERTIARY)
            .extra_letter_spacing(0.8),
    );
    ui.add_space(4.0);
}

fn capture_option_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(10.0)
                .strong()
                .color(if selected { ACCENT } else { MUTED }),
        )
        .fill(if selected { INFO_SOFT } else { PAPER })
        .stroke(egui::Stroke::new(
            1.0,
            if selected { ACCENT } else { BORDER },
        ))
        .corner_radius(6.0)
        .min_size(egui::vec2(132.0, 32.0)),
    )
}

fn audio_category_button(
    ui: &mut egui::Ui,
    category: AudioCaptureCategory,
    selected: bool,
) -> egui::Response {
    let response = egui::Frame::new()
        .fill(if selected { INFO_SOFT } else { PAPER })
        .stroke(egui::Stroke::new(
            if selected { 1.5 } else { 1.0 },
            if selected { ACCENT } else { BORDER },
        ))
        .corner_radius(7.0)
        .inner_margin(egui::Margin::same(11))
        .show(ui, |ui| {
            ui.set_min_height(54.0);
            ui.label(
                egui::RichText::new(category.label())
                    .size(10.5)
                    .strong()
                    .color(TEXT),
            );
            ui.label(
                egui::RichText::new(category.expected())
                    .size(9.0)
                    .color(if selected { ACCENT } else { TERTIARY }),
            );
        })
        .response;
    ui.interact(
        response.rect,
        ui.id().with(("audio-category", category.key())),
        egui::Sense::click(),
    )
}

fn campaign_coverage_row(
    ui: &mut egui::Ui,
    label: &str,
    cases: usize,
    case_target: usize,
    speakers: usize,
    speaker_target: usize,
) {
    let ready = cases >= case_target && speakers >= speaker_target;
    egui::Frame::new()
        .fill(if ready { SUCCESS_SOFT } else { PANEL })
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(11, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                status_dot(ui, if ready { SUCCESS } else { TERTIARY });
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(label).size(10.0).strong().color(TEXT));
                    ui.label(
                        egui::RichText::new(format!(
                            "{speakers}/{speaker_target} distinct speakers"
                        ))
                        .size(8.5)
                        .color(MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{cases}/{case_target}"))
                            .size(13.0)
                            .strong()
                            .monospace()
                            .color(if ready { SUCCESS } else { TERTIARY }),
                    );
                });
            });
        });
    ui.add_space(6.0);
}

fn benchmark_metric_row(ui: &mut egui::Ui, label: &str, metric: &services::AudioBenchmarkMetric) {
    let f1 = metric
        .f1
        .map_or_else(|| "F1 --".to_owned(), |value| format!("F1 {value:.3}"));
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).size(10.5).strong().color(TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    outlined_chip(ui, &f1);
                });
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                outlined_chip(ui, &format!("TP {}", metric.true_positive));
                outlined_chip(ui, &format!("FP {}", metric.false_positive));
                outlined_chip(ui, &format!("TN {}", metric.true_negative));
                outlined_chip(ui, &format!("FN {}", metric.false_negative));
                if let Some(threshold) = metric.recommended_threshold {
                    outlined_chip(ui, &format!("RECOMMENDED {threshold:.2}"));
                }
            });
        });
    ui.add_space(8.0);
}

fn info_card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(16))
        .show(ui, contents);
}

fn config_card(
    ui: &mut egui::Ui,
    name: &str,
    description: &str,
    size: &str,
    selected: bool,
) -> egui::Response {
    let response = egui::Frame::new()
        .fill(if selected { INFO_SOFT } else { PAPER })
        .stroke(egui::Stroke::new(
            if selected { 1.5 } else { 1.0 },
            if selected { ACCENT } else { BORDER },
        ))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_height(150.0);
            ui.label(egui::RichText::new(name).size(12.0).strong().color(TEXT));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(description).size(11.0).color(MUTED));
            ui.add_space(14.0);
            outlined_chip(ui, size);
        })
        .response;
    ui.interact(response.rect, response.id, egui::Sense::click())
}

fn config_card_live(
    ui: &mut egui::Ui,
    config: &services::ConfigInfo,
    selected: bool,
    running: bool,
) -> egui::Response {
    let response = egui::Frame::new()
        .fill(if selected { INFO_SOFT } else { PAPER })
        .stroke(egui::Stroke::new(
            if selected || running { 1.5 } else { 1.0 },
            if running {
                SUCCESS
            } else if selected {
                ACCENT
            } else {
                BORDER
            },
        ))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_height(154.0);
            ui.set_max_height(154.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&config.name)
                        .size(12.0)
                        .strong()
                        .color(TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        status_chip(ui, "RUNNING", SUCCESS, SUCCESS_SOFT);
                    } else if selected {
                        outlined_chip(ui, "SELECTED");
                    }
                });
            });
            ui.add_space(9.0);
            ui.add_sized(
                [ui.available_width(), 60.0],
                egui::Label::new(egui::RichText::new(&config.summary).size(10.5).color(MUTED))
                    .wrap(),
            );
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                outlined_chip(ui, &services::human_bytes(config.size_bytes));
                outlined_chip(ui, &services::human_age(config.modified));
            });
        })
        .response;
    let response = ui.interact(response.rect, response.id, egui::Sense::click());
    if response.hovered() && !selected && !running {
        ui.painter().rect_stroke(
            response.rect,
            9.0,
            egui::Stroke::new(1.0, TERTIARY),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn sensor_capability_card(ui: &mut egui::Ui, title: &str, detail: &str, active: bool) {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_height(100.0);
            ui.horizontal(|ui| {
                status_dot(ui, if active { SUCCESS } else { TERTIARY });
                ui.label(
                    egui::RichText::new(title)
                        .size(10.0)
                        .strong()
                        .color(TEXT)
                        .extra_letter_spacing(1.0),
                );
            });
            ui.add_space(12.0);
            ui.label(egui::RichText::new(detail).size(12.0).color(MUTED));
            ui.label(
                egui::RichText::new(if active { "streaming" } else { "waiting" })
                    .size(10.0)
                    .color(if active { SUCCESS } else { TERTIARY }),
            );
        });
}

fn metric_summary_card(ui: &mut egui::Ui, label: &str, value: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_height(84.0);
            ui.label(
                egui::RichText::new(label)
                    .size(9.0)
                    .strong()
                    .color(TERTIARY)
                    .extra_letter_spacing(1.0),
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new(value).size(22.0).strong().color(color));
        });
}

fn stream_metric_card(ui: &mut egui::Ui, stream: &services::StreamMetric) {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_height(98.0);
            ui.horizontal(|ui| {
                status_dot(ui, SUCCESS);
                ui.label(
                    egui::RichText::new(&stream.name)
                        .size(11.0)
                        .strong()
                        .color(TEXT),
                );
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:.1} Hz", stream.frequency_hz))
                        .size(17.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format_compact_count(stream.total_samples))
                            .size(10.0)
                            .monospace()
                            .color(MUTED),
                    );
                });
            });
            ui.label(
                egui::RichText::new(format!("batch {:.1}", stream.batch_size))
                    .size(9.0)
                    .color(TERTIARY),
            );
        });
}

fn raw_stream_row(ui: &mut egui::Ui, stream: &services::StreamMetric) {
    let active = stream.frequency_hz > 0.05;
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                status_dot(ui, if active { SUCCESS } else { TERTIARY });
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&stream.name)
                            .size(12.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  →  {}",
                            if stream.source.is_empty() {
                                "source"
                            } else {
                                &stream.source
                            },
                            if stream.targets.is_empty() {
                                "unsubscribed"
                            } else {
                                &stream.targets
                            }
                        ))
                        .size(9.0)
                        .monospace()
                        .color(TERTIARY),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    outlined_chip(ui, &format_compact_count(stream.total_samples));
                    outlined_chip(ui, &format!("{:.1} ms lag", stream.input_lag_ms));
                    outlined_chip(ui, &format!("{:.1} Hz", stream.frequency_hz));
                });
            });
        });
}

fn table_heading(ui: &mut egui::Ui, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(9.0)
            .strong()
            .color(TERTIARY)
            .extra_letter_spacing(0.8),
    );
}

fn format_compact_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn notice_card(ui: &mut egui::Ui, title: &str, body: &str) {
    egui::Frame::new()
        .fill(INFO_SOFT)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.label(
                    egui::RichText::new("ⓘ")
                        .size(15.0)
                        .strong()
                        .color(egui::Color32::from_rgb(0x25, 0x63, 0xa8)),
                );
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(title).size(11.0).strong().color(TEXT));
                    ui.label(egui::RichText::new(body).size(10.0).color(MUTED));
                });
            });
        });
}

fn key_value(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(key).size(11.0).color(TERTIARY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .size(11.0)
                    .monospace()
                    .color(TEXT),
            );
        });
    });
    ui.add_space(5.0);
}

fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

fn status_chip(ui: &mut egui::Ui, label: &str, color: egui::Color32, background: egui::Color32) {
    egui::Frame::new()
        .fill(background)
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(20.0)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                status_dot(ui, color);
                ui.label(egui::RichText::new(label).size(9.0).strong().color(color));
            });
        });
}

fn outlined_chip(ui: &mut egui::Ui, label: &str) {
    egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(20.0)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(9.0).color(MUTED));
        });
}

fn timeline_kind_color(kind: &str) -> egui::Color32 {
    match kind {
        "grasp" => egui::Color32::from_rgb(0x42, 0x9c, 0x66),
        "scene" => egui::Color32::from_rgb(0x68, 0x72, 0xc4),
        "speech" => egui::Color32::from_rgb(0xdf, 0x67, 0x67),
        "wake" => egui::Color32::from_rgb(0x28, 0xad, 0xbe),
        _ => ACCENT,
    }
}

fn filter_chip(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    color: egui::Color32,
) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label.to_uppercase())
                .size(9.0)
                .strong()
                .color(if selected { color } else { TERTIARY }),
        )
        .fill(if selected {
            color.gamma_multiply(0.10)
        } else {
            PAPER
        })
        .stroke(egui::Stroke::new(
            1.0,
            if selected {
                color.gamma_multiply(0.55)
            } else {
                BORDER
            },
        ))
        .corner_radius(20.0),
    )
}

fn timeline_event_card(
    ui: &mut egui::Ui,
    event: &services::TimelineEvent,
    evidence_texture: Option<&egui::TextureHandle>,
) -> egui::Response {
    let color = timeline_kind_color(&event.kind);
    let frame = egui::Frame::new()
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(13))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(5.0, 46.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, color);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(event.kind.to_uppercase())
                                .size(9.0)
                                .strong()
                                .color(color)
                                .extra_letter_spacing(0.9),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{:.0}%", event.confidence * 100.0))
                                    .size(10.0)
                                    .strong()
                                    .color(color),
                            );
                            ui.label(
                                egui::RichText::new(event.clock_time())
                                    .size(9.0)
                                    .monospace()
                                    .color(TERTIARY),
                            );
                        });
                    });
                    ui.label(
                        egui::RichText::new(&event.title)
                            .size(13.0)
                            .strong()
                            .color(TEXT),
                    );
                    if !event.summary.is_empty() {
                        ui.label(egui::RichText::new(&event.summary).size(10.0).color(MUTED));
                    }
                    if let Some(texture) = evidence_texture {
                        ui.add_space(7.0);
                        let width = ui.available_width().min(248.0);
                        let texture_size = texture.size_vec2();
                        let height = width * texture_size.y / texture_size.x.max(1.0);
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(width, height.min(148.0)),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(rect, 6.0, ELEVATED);
                        ui.painter().image(
                            texture.id(),
                            rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    ui.add_space(5.0);
                    ui.horizontal_wrapped(|ui| {
                        if !event.label.is_empty() {
                            outlined_chip(ui, &event.label);
                        }
                        if !event.node.is_empty() {
                            ui.label(
                                egui::RichText::new(&event.node)
                                    .size(9.0)
                                    .monospace()
                                    .color(TERTIARY),
                            );
                        }
                        if !event.source_stream_id.is_empty() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} #{}{}",
                                    event.source_stream_id,
                                    event.source_sequence,
                                    if event.media_kind.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" Â· {}", event.media_kind)
                                    }
                                ))
                                .size(8.5)
                                .monospace()
                                .color(TERTIARY),
                            );
                        }
                        if !event.model_name.is_empty() {
                            let model = if event.model_version.is_empty() {
                                event.model_name.clone()
                            } else {
                                format!("{}@{}", event.model_name, event.model_version)
                            };
                            outlined_chip(ui, &model);
                        }
                        if !event.model_backend.is_empty() {
                            let execution = if event.model_runtime.is_empty() {
                                event.model_backend.clone()
                            } else {
                                format!("{} · {}", event.model_runtime, event.model_backend)
                            };
                            ui.label(
                                egui::RichText::new(execution)
                                    .size(9.0)
                                    .monospace()
                                    .color(TERTIARY),
                            );
                        }
                        if !event.model_name.is_empty()
                            || !event.model_backend.is_empty()
                            || !event.model_runtime.is_empty()
                        {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{:.2} ms",
                                    event.inference_latency_ms
                                ))
                                .size(9.0)
                                .monospace()
                                .color(TERTIARY),
                            );
                        }
                    });
                });
            });
        });
    let response = ui.interact(
        frame.response.rect,
        ui.id().with(("timeline-event", &event.id)),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter().rect_stroke(
            response.rect,
            8.0,
            egui::Stroke::new(1.5, color.gamma_multiply(0.7)),
            egui::StrokeKind::Inside,
        );
        response.on_hover_text("Open playback at this event")
    } else {
        response
    }
}

fn dark_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(9.0)
                .strong()
                .color(egui::Color32::WHITE)
                .extra_letter_spacing(0.8),
        )
        .fill(TEXT)
        .stroke(egui::Stroke::NONE)
        .corner_radius(6.0)
        .min_size(egui::vec2(116.0, 32.0)),
    )
}

fn outline_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(9.0)
                .strong()
                .color(MUTED)
                .extra_letter_spacing(0.8),
        )
        .fill(PAPER)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(6.0)
        .min_size(egui::vec2(92.0, 32.0)),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let main_thread_token = re_viewer::MainThreadToken::i_promise_i_am_on_the_main_thread();
    re_log::setup_logging();
    re_crash_handler::install_crash_handlers(re_viewer::build_info());

    let (grpc_shutdown, grpc_shutdown_request) = re_grpc_server::shutdown::shutdown();
    let grpc_rx = re_grpc_server::spawn_with_recv(
        "127.0.0.1:9876".parse()?,
        Default::default(),
        grpc_shutdown_request,
    );
    let (blueprint_tx, blueprint_rx) = re_log_channel::log_channel(re_log_channel::LogSource::Sdk);

    let mut native_options = re_viewer::native::eframe_options(None);
    native_options.viewport = native_options
        .viewport
        .with_app_id("hab_native_shell")
        .with_icon(hab_icon_data())
        .with_inner_size([1600.0, 1000.0])
        .with_min_inner_size([1100.0, 720.0]);

    let startup_options = re_viewer::StartupOptions {
        persist_state: false,
        hide_welcome_screen: true,
        expect_data_soon: Some(true),
        panel_state_overrides: re_viewer::PanelStateOverrides {
            top: Some(re_sdk_types::blueprint::components::PanelState::Hidden),
            blueprint: Some(re_sdk_types::blueprint::components::PanelState::Hidden),
            selection: Some(re_sdk_types::blueprint::components::PanelState::Hidden),
            time: None,
        },
        ..Default::default()
    };

    eframe::run_native(
        "HAB",
        native_options,
        Box::new(move |cc| {
            re_viewer::customize_eframe_and_setup_renderer(cc)?;
            cc.egui_ctx.set_theme(egui::Theme::Light);

            let mut rerun_app = re_viewer::App::new(
                main_thread_token,
                re_viewer::build_info(),
                re_viewer::AppEnvironment::Custom("Hab native shell".to_owned()),
                startup_options,
                cc,
                None,
                re_viewer::AsyncRuntimeHandle::from_current_tokio_runtime_or_wasmbindgen()?,
            );
            rerun_app.add_log_receiver(grpc_rx);
            rerun_app.add_log_receiver(blueprint_rx);

            Ok(Box::new(HabShell::new(
                rerun_app,
                blueprint_tx,
                grpc_shutdown,
            )))
        }),
    )?;

    Ok(())
}

fn hab_icon_data() -> egui::IconData {
    const SIZE: u32 = 64;
    const SAMPLES_PER_AXIS: u32 = 4;
    const SAMPLE_COUNT: u32 = SAMPLES_PER_AXIS * SAMPLES_PER_AXIS;

    let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut coverage = 0;
            for sample_y in 0..SAMPLES_PER_AXIS {
                for sample_x in 0..SAMPLES_PER_AXIS {
                    let px = x as f32 + (sample_x as f32 + 0.5) / SAMPLES_PER_AXIS as f32;
                    let py = y as f32 + (sample_y as f32 + 0.5) / SAMPLES_PER_AXIS as f32;

                    let outer_x = px - 32.0;
                    let outer_y = py - 32.0;
                    let inside_outer = outer_x * outer_x + outer_y * outer_y <= 23.5 * 23.5;

                    let cutout_x = px - 46.0;
                    let cutout_y = py - 19.0;
                    let inside_cutout = cutout_x * cutout_x + cutout_y * cutout_y <= 11.5 * 11.5;

                    if inside_outer && !inside_cutout {
                        coverage += 1;
                    }
                }
            }

            let offset = ((y * SIZE + x) * 4) as usize;
            rgba[offset] = 17;
            rgba[offset + 1] = 17;
            rgba[offset + 2] = 20;
            rgba[offset + 3] = ((coverage * 255) / SAMPLE_COUNT) as u8;
        }
    }

    egui::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hab_icon_is_a_valid_transparent_rgba_image() {
        let icon = hab_icon_data();
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        let mut alpha = icon.rgba.iter().skip(3).step_by(4).copied();
        assert!(alpha.clone().any(|value| value == 0));
        assert!(alpha.any(|value| value == 255));
    }

    #[test]
    fn each_workspace_builds_an_activatable_blueprint() {
        for workspace in Workspace::ALL {
            let messages = blueprint_for(workspace, PlaybackSub::default(), StreamSub::default())
                .to_log_msgs_with_activation("hab", BlueprintActivation::default())
                .expect("blueprint should serialize");
            assert!(messages.len() > 4);
            assert!(matches!(
                messages.last(),
                Some(rerun::external::re_log_types::LogMsg::BlueprintActivationCommand(_))
            ));
        }
    }

    #[test]
    fn native_pages_do_not_schedule_rerun_blueprints() {
        for workspace in [
            Workspace::Pipeline,
            Workspace::Timeline,
            Workspace::Metrics,
            Workspace::RawData,
            Workspace::Live,
            Workspace::Device,
            Workspace::Configs,
        ] {
            assert!(!workspace_uses_rerun_view(workspace, PlaybackSub::Sessions));
        }
        assert!(!workspace_uses_rerun_view(
            Workspace::Playback,
            PlaybackSub::Sessions
        ));
        assert!(!workspace_uses_rerun_view(
            Workspace::Playback,
            PlaybackSub::Canvas
        ));
        assert!(workspace_uses_rerun_view(
            Workspace::Streams,
            PlaybackSub::Sessions
        ));
    }
}

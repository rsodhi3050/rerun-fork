//! Native control plane for the Hab shell.
//!
//! The viewer remains an egui wrapper around Rerun, while this module owns
//! non-visual operations: C++ engine requests, config discovery/lifecycle,
//! synthetic-device startup, and recording/session discovery. All network and
//! process work runs on a worker thread so a slow device can never stall egui.

use std::{
    collections::{VecDeque, hash_map::DefaultHasher},
    fs,
    hash::{Hash as _, Hasher as _},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use tungstenite::{Message, connect};

const ENGINE_ENDPOINT: &str = "ws://127.0.0.1:9999";
const ENGINE_ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9999);
const TIMELINE_STREAMS: [&str; 4] = [
    "grasp_detector.events",
    "scene_classifier.events",
    "speech_detector.events",
    "wake_detector.events",
];
const CAMERA_STREAM: &str = "camera_pipeline.frame";
const AUDIO_STREAM: &str = "audio_pipeline.audio";
const MAX_TIMELINE_EVENTS: usize = 250;
const MAX_AUDIO_LEVELS: usize = 320;

#[derive(Clone, Debug, Default)]
pub struct EngineSnapshot {
    pub connected: bool,
    pub pipeline_name: Option<String>,
    pub active_config: Option<PathBuf>,
    pub engine_pid: Option<u32>,
    pub device_pid: Option<u32>,
    pub feed_count: usize,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ConfigInfo {
    pub path: PathBuf,
    pub name: String,
    pub summary: String,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub path: PathBuf,
    pub rrd_path: Option<PathBuf>,
    pub hdf5_path: Option<PathBuf>,
    pub name: String,
    pub study: String,
    pub participant: String,
    pub format: &'static str,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
    pub artifacts: Vec<String>,
    pub annotation_preview: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PreparedPlayback {
    pub source_path: PathBuf,
    pub rrd_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct TimelineEvent {
    pub id: String,
    pub kind: String,
    pub timestamp_ns: i64,
    pub title: String,
    pub summary: String,
    pub label: String,
    pub confidence: f32,
    pub node: String,
    pub model_name: String,
    pub model_version: String,
    pub model_backend: String,
    pub model_runtime: String,
    pub inference_latency_ms: f64,
}

#[derive(Clone, Debug)]
pub struct CameraFrame {
    pub sequence: u64,
    pub timestamp_ns: i64,
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct AudioFrame {
    pub timestamp_ns: i64,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub rms: f32,
    pub peak: f32,
    pub source: String,
    pub sample_format: String,
    pub levels: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct TransformMetric {
    pub id: String,
    pub kind: String,
    pub latency_ms: f64,
    pub jitter_ms: f64,
}

#[derive(Clone, Debug, Default)]
pub struct StreamMetric {
    pub name: String,
    pub source: String,
    pub targets: String,
    pub frequency_hz: f64,
    pub input_lag_ms: f64,
    pub batch_size: f64,
    pub total_samples: u64,
}

#[derive(Clone, Debug, Default)]
pub struct PipelineMetrics {
    pub status: String,
    pub transforms: Vec<TransformMetric>,
    pub streams: Vec<StreamMetric>,
}

impl TimelineEvent {
    pub fn clock_time(&self) -> String {
        let seconds = self.timestamp_ns.max(0) as u64 / 1_000_000_000;
        let millis = (self.timestamp_ns.max(0) as u64 / 1_000_000) % 1_000;
        let seconds_in_day = seconds % 86_400;
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            seconds_in_day / 3_600,
            (seconds_in_day / 60) % 60,
            seconds_in_day % 60,
            millis
        )
    }
}

enum WorkerCommand {
    Probe,
    SetParameter {
        transform: String,
        parameter: String,
        value: Value,
    },
    StartRecording(PathBuf),
    StopRecording,
    PrepareHdf5Playback(PathBuf),
    RunConfig(PathBuf),
    StopEngine,
    StartSyntheticDevice,
}

enum WorkerEvent {
    ProbeFinished(Result<EngineSnapshot, String>),
    ActionFinished(Result<String, String>),
    RecordingStarted(Result<PathBuf, String>),
    RecordingStopped(Result<String, String>),
    Hdf5PlaybackPrepared(Result<PreparedPlayback, String>),
    ConfigStarted(Result<PathBuf, String>),
    EngineStopped(Result<String, String>),
}

enum TimelineListenerEvent {
    Status(Result<(), String>),
    Event(TimelineEvent),
    Metrics(PipelineMetrics),
    Camera(CameraFrame),
    Audio(AudioFrame),
}

pub struct NativeServices {
    root: PathBuf,
    command_tx: mpsc::Sender<WorkerCommand>,
    event_rx: mpsc::Receiver<WorkerEvent>,
    timeline_rx: mpsc::Receiver<TimelineListenerEvent>,
    pub snapshot: EngineSnapshot,
    pub configs: Vec<ConfigInfo>,
    pub sessions: Vec<SessionInfo>,
    pub selected_config: usize,
    pub selected_session: usize,
    pub recording_path: Option<PathBuf>,
    pub timeline_events: VecDeque<TimelineEvent>,
    pub timeline_connected: bool,
    pub timeline_error: Option<String>,
    pub pipeline_metrics: Option<PipelineMetrics>,
    pub camera_frame: Option<CameraFrame>,
    pub audio_levels: VecDeque<f32>,
    pub audio_rms: f32,
    pub audio_peak: f32,
    pub audio_timestamp_ns: i64,
    pub audio_sample_rate_hz: u32,
    pub audio_channels: u16,
    pub audio_source: String,
    pub audio_sample_format: String,
    prepared_playback: Option<PreparedPlayback>,
    pub busy: bool,
    last_probe: Instant,
    probe_pending: bool,
    shutdown_started: bool,
}

impl Default for NativeServices {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeServices {
    pub fn new() -> Self {
        let root = find_hab_root();
        let configs = discover_configs(&root);
        let sessions = discover_sessions(&root);
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (timeline_tx, timeline_rx) = mpsc::channel();
        let worker_root = root.clone();
        thread::Builder::new()
            .name("hab-native-services".to_owned())
            .spawn(move || worker_loop(worker_root, command_rx, event_tx))
            .expect("Hab service worker should start");
        thread::Builder::new()
            .name("hab-timeline-listener".to_owned())
            .spawn(move || timeline_listener_loop(timeline_tx))
            .expect("Hab timeline listener should start");

        Self {
            root,
            command_tx,
            event_rx,
            timeline_rx,
            snapshot: EngineSnapshot::default(),
            configs,
            sessions,
            selected_config: 0,
            selected_session: 0,
            recording_path: None,
            timeline_events: VecDeque::with_capacity(MAX_TIMELINE_EVENTS),
            timeline_connected: false,
            timeline_error: None,
            pipeline_metrics: None,
            camera_frame: None,
            audio_levels: VecDeque::with_capacity(MAX_AUDIO_LEVELS),
            audio_rms: 0.0,
            audio_peak: 0.0,
            audio_timestamp_ns: 0,
            audio_sample_rate_hz: 0,
            audio_channels: 0,
            audio_source: "waiting".to_owned(),
            audio_sample_format: String::new(),
            prepared_playback: None,
            busy: false,
            last_probe: Instant::now() - Duration::from_secs(10),
            probe_pending: false,
            shutdown_started: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Finalize active recording and stop the engine/device stack owned by this
    /// HAB window. This is synchronous so native window teardown cannot race
    /// process cleanup or leave camera and microphone capture running.
    pub fn shutdown(&mut self) -> Result<(), String> {
        if self.shutdown_started {
            return Ok(());
        }
        self.shutdown_started = true;

        let mut errors = Vec::new();
        if self.recording_path.is_some()
            && let Err(error) =
                set_parameter("rerun", "recording_path", Value::String(String::new()))
        {
            errors.push(format!("recording finalization failed: {error}"));
        }
        self.recording_path = None;

        if let Err(error) = stop_owned_processes(&self.root) {
            errors.push(error);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn poll(&mut self) -> Vec<String> {
        let mut notices = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                WorkerEvent::ProbeFinished(result) => {
                    self.probe_pending = false;
                    match result {
                        Ok(mut snapshot) => {
                            snapshot.active_config = self.snapshot.active_config.clone();
                            self.snapshot = snapshot;
                        }
                        Err(error) => {
                            self.snapshot.connected = false;
                            self.snapshot.last_error = Some(error);
                        }
                    }
                }
                WorkerEvent::ActionFinished(result) => {
                    self.busy = false;
                    notices.push(result.unwrap_or_else(|error| format!("Action failed: {error}")));
                }
                WorkerEvent::RecordingStarted(result) => {
                    self.busy = false;
                    match result {
                        Ok(path) => {
                            self.recording_path = Some(path.clone());
                            notices.push(format!(
                                "Recording synchronized {} + {}",
                                path.display(),
                                path.with_extension("h5").display()
                            ));
                        }
                        Err(error) => notices.push(format!("Recording failed: {error}")),
                    }
                }
                WorkerEvent::RecordingStopped(result) => {
                    self.busy = false;
                    self.recording_path = None;
                    self.refresh_sessions();
                    self.selected_session = 0;
                    notices.push(
                        result.unwrap_or_else(|error| format!("Stop recording failed: {error}")),
                    );
                }
                WorkerEvent::Hdf5PlaybackPrepared(result) => {
                    self.busy = false;
                    match result {
                        Ok(prepared) => {
                            notices.push(format!(
                                "Validated HDF5 and prepared {}",
                                prepared.rrd_path.display()
                            ));
                            self.prepared_playback = Some(prepared);
                        }
                        Err(error) => {
                            notices.push(format!("HDF5 playback failed: {error}"));
                        }
                    }
                }
                WorkerEvent::ConfigStarted(result) => {
                    self.busy = false;
                    match result {
                        Ok(path) => {
                            self.snapshot.active_config = Some(path.clone());
                            self.snapshot.pipeline_name = pipeline_name_from_file(&path);
                            notices.push(format!("Started {}", path.display()));
                            self.last_probe = Instant::now() - Duration::from_secs(10);
                        }
                        Err(error) => notices.push(format!("Config start failed: {error}")),
                    }
                }
                WorkerEvent::EngineStopped(result) => {
                    self.busy = false;
                    if result.is_ok() {
                        self.snapshot = EngineSnapshot::default();
                    }
                    notices.push(
                        result.unwrap_or_else(|error| format!("Engine stop failed: {error}")),
                    );
                }
            }
        }
        while let Ok(event) = self.timeline_rx.try_recv() {
            match event {
                TimelineListenerEvent::Status(result) => match result {
                    Ok(()) => {
                        self.timeline_connected = true;
                        self.timeline_error = None;
                    }
                    Err(error) => {
                        self.timeline_connected = false;
                        self.timeline_error = Some(error);
                    }
                },
                TimelineListenerEvent::Event(event) => {
                    self.timeline_connected = true;
                    self.timeline_error = None;
                    if self
                        .timeline_events
                        .front()
                        .is_some_and(|existing| existing.id == event.id)
                    {
                        continue;
                    }
                    self.timeline_events.push_front(event);
                    self.timeline_events.truncate(MAX_TIMELINE_EVENTS);
                }
                TimelineListenerEvent::Metrics(metrics) => {
                    self.pipeline_metrics = Some(metrics);
                }
                TimelineListenerEvent::Camera(frame) => {
                    self.camera_frame = Some(frame);
                }
                TimelineListenerEvent::Audio(frame) => {
                    self.audio_rms = frame.rms;
                    self.audio_peak = frame.peak;
                    self.audio_timestamp_ns = frame.timestamp_ns;
                    self.audio_sample_rate_hz = frame.sample_rate_hz;
                    self.audio_channels = frame.channels;
                    self.audio_source = frame.source;
                    self.audio_sample_format = frame.sample_format;
                    self.audio_levels.extend(frame.levels);
                    while self.audio_levels.len() > MAX_AUDIO_LEVELS {
                        self.audio_levels.pop_front();
                    }
                }
            }
        }
        notices
    }

    pub fn maybe_probe(&mut self) {
        if self.probe_pending || self.last_probe.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_probe = Instant::now();
        self.probe_pending = self.command_tx.send(WorkerCommand::Probe).is_ok();
    }

    pub fn refresh_configs(&mut self) -> String {
        let selected = self
            .configs
            .get(self.selected_config)
            .map(|config| config.path.clone());
        self.configs = discover_configs(&self.root);
        if let Some(selected) = selected {
            self.selected_config = self
                .configs
                .iter()
                .position(|config| config.path == selected)
                .unwrap_or(0);
        } else {
            self.selected_config = 0;
        }
        format!("Discovered {} pipeline configs", self.configs.len())
    }

    pub fn refresh_sessions(&mut self) -> String {
        let selected = self
            .sessions
            .get(self.selected_session)
            .map(|session| session.path.clone());
        self.sessions = discover_sessions(&self.root);
        if let Some(selected) = selected {
            self.selected_session = self
                .sessions
                .iter()
                .position(|session| session.path == selected)
                .unwrap_or(0);
        } else {
            self.selected_session = 0;
        }
        format!("Discovered {} recorded sessions", self.sessions.len())
    }

    pub fn selected_config_path(&self) -> Option<&Path> {
        self.configs
            .get(self.selected_config)
            .map(|config| config.path.as_path())
    }

    pub fn selected_session_path(&self) -> Option<&Path> {
        self.sessions
            .get(self.selected_session)
            .map(|session| session.path.as_path())
    }

    pub fn selected_rrd_path(&self) -> Option<&Path> {
        self.sessions
            .get(self.selected_session)
            .and_then(|session| session.rrd_path.as_deref())
    }

    pub fn selected_hdf5_path(&self) -> Option<&Path> {
        self.sessions
            .get(self.selected_session)
            .and_then(|session| session.hdf5_path.as_deref())
    }

    pub fn prepare_hdf5_playback(&mut self) {
        if self.busy {
            return;
        }
        let Some(path) = self.selected_hdf5_path().map(Path::to_owned) else {
            return;
        };
        self.busy = true;
        if self
            .command_tx
            .send(WorkerCommand::PrepareHdf5Playback(path))
            .is_err()
        {
            self.busy = false;
        }
    }

    pub fn take_prepared_playback(&mut self) -> Option<PreparedPlayback> {
        self.prepared_playback.take()
    }

    pub fn set_parameter(
        &mut self,
        transform: impl Into<String>,
        parameter: impl Into<String>,
        value: Value,
    ) {
        self.busy = true;
        if self
            .command_tx
            .send(WorkerCommand::SetParameter {
                transform: transform.into(),
                parameter: parameter.into(),
                value,
            })
            .is_err()
        {
            self.busy = false;
        }
    }

    pub fn start_recording(&mut self) -> Option<PathBuf> {
        if self.busy || self.recording_path.is_some() {
            return None;
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = self
            .root
            .join("data")
            .join("study_sessions")
            .join("inspector_live")
            .join("local")
            .join(format!("session-{timestamp}"))
            .join("session.rrd");
        self.busy = true;
        if self
            .command_tx
            .send(WorkerCommand::StartRecording(path.clone()))
            .is_err()
        {
            self.busy = false;
            return None;
        }
        Some(path)
    }

    pub fn stop_recording(&mut self) {
        if self.busy || self.recording_path.is_none() {
            return;
        }
        self.busy = true;
        if self.command_tx.send(WorkerCommand::StopRecording).is_err() {
            self.busy = false;
        }
    }

    pub fn run_selected_config(&mut self) {
        let Some(path) = self.selected_config_path().map(Path::to_owned) else {
            return;
        };
        self.busy = true;
        if self
            .command_tx
            .send(WorkerCommand::RunConfig(path))
            .is_err()
        {
            self.busy = false;
        }
    }

    pub fn stop_engine(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        if self.command_tx.send(WorkerCommand::StopEngine).is_err() {
            self.busy = false;
        }
    }

    pub fn connect_synthetic_device(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        if self
            .command_tx
            .send(WorkerCommand::StartSyntheticDevice)
            .is_err()
        {
            self.busy = false;
        }
    }

    pub fn clear_timeline(&mut self) {
        self.timeline_events.clear();
    }
}

impl Drop for NativeServices {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("HAB shutdown warning: {error}");
        }
    }
}

fn timeline_listener_loop(event_tx: mpsc::Sender<TimelineListenerEvent>) {
    loop {
        match connect(ENGINE_ENDPOINT) {
            Ok((mut socket, _)) => {
                if event_tx
                    .send(TimelineListenerEvent::Status(Ok(())))
                    .is_err()
                {
                    return;
                }
                let mut subscribe_failed = None;
                for (index, stream) in TIMELINE_STREAMS.iter().enumerate() {
                    let payload = json!({
                        "api_version": "0.12",
                        "api_request": {
                            "request_id": format!("hab-timeline-{index}"),
                            "start_stream_request": {
                                "stream_id": format!("hab-timeline-{index}"),
                                "app_id": "hab-shell",
                                (*stream): {}
                            }
                        }
                    });
                    if let Err(error) = socket.send(Message::Text(payload.to_string().into())) {
                        subscribe_failed = Some(error.to_string());
                        break;
                    }
                }
                if subscribe_failed.is_none() {
                    let payload = json!({
                        "api_version": "0.12",
                        "api_request": {
                            "request_id": "hab-pipeline-monitor",
                            "start_stream_request": {
                                "stream_id": "hab-pipeline-monitor",
                                "app_id": "hab-shell",
                                "pipeline_monitor": {}
                            }
                        }
                    });
                    if let Err(error) = socket.send(Message::Text(payload.to_string().into())) {
                        subscribe_failed = Some(error.to_string());
                    }
                }
                for (stream, request_id) in [
                    (CAMERA_STREAM, "hab-camera-preview"),
                    (AUDIO_STREAM, "hab-audio-preview"),
                ] {
                    if subscribe_failed.is_some() {
                        break;
                    }
                    let payload = json!({
                        "api_version": "0.12",
                        "api_request": {
                            "request_id": request_id,
                            "start_stream_request": {
                                "stream_id": request_id,
                                "app_id": "hab-shell",
                                (stream): {}
                            }
                        }
                    });
                    if let Err(error) = socket.send(Message::Text(payload.to_string().into())) {
                        subscribe_failed = Some(error.to_string());
                    }
                }
                if let Some(error) = subscribe_failed {
                    if event_tx
                        .send(TimelineListenerEvent::Status(Err(error)))
                        .is_err()
                    {
                        return;
                    }
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }

                let mut last_camera_preview = Instant::now() - Duration::from_secs(1);
                let mut last_audio_preview = Instant::now() - Duration::from_secs(1);
                loop {
                    match socket.read() {
                        Ok(Message::Text(text)) => {
                            let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else {
                                continue;
                            };
                            let Some(batches) =
                                value.get("stream_batch").and_then(Value::as_object)
                            else {
                                continue;
                            };
                            if let Some(sample) = batches
                                .get("pipeline_monitor")
                                .and_then(|batch| batch.get("samples"))
                                .and_then(Value::as_array)
                                .and_then(|samples| samples.last())
                                && let Some(metrics) = parse_pipeline_metrics(sample)
                                && event_tx
                                    .send(TimelineListenerEvent::Metrics(metrics))
                                    .is_err()
                            {
                                return;
                            }
                            if last_camera_preview.elapsed() >= Duration::from_millis(66)
                                && let Some(sample) = batches
                                    .get(CAMERA_STREAM)
                                    .and_then(|batch| batch.get("samples"))
                                    .and_then(Value::as_array)
                                    .and_then(|samples| samples.last())
                                && let Some(frame) = parse_camera_frame(sample)
                            {
                                last_camera_preview = Instant::now();
                                if event_tx.send(TimelineListenerEvent::Camera(frame)).is_err() {
                                    return;
                                }
                            }
                            if last_audio_preview.elapsed() >= Duration::from_millis(40)
                                && let Some(sample) = batches
                                    .get(AUDIO_STREAM)
                                    .and_then(|batch| batch.get("samples"))
                                    .and_then(Value::as_array)
                                    .and_then(|samples| samples.last())
                                && let Some(frame) = parse_audio_frame(sample)
                            {
                                last_audio_preview = Instant::now();
                                if event_tx.send(TimelineListenerEvent::Audio(frame)).is_err() {
                                    return;
                                }
                            }
                            for stream in TIMELINE_STREAMS {
                                let Some(samples) = batches
                                    .get(stream)
                                    .and_then(|batch| batch.get("samples"))
                                    .and_then(Value::as_array)
                                else {
                                    continue;
                                };
                                for sample in samples {
                                    if let Some(event) = parse_timeline_event(stream, sample)
                                        && event_tx
                                            .send(TimelineListenerEvent::Event(event))
                                            .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                        }
                        Ok(Message::Ping(payload)) => {
                            if socket.send(Message::Pong(payload)).is_err() {
                                break;
                            }
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
                if event_tx
                    .send(TimelineListenerEvent::Status(Err(
                        "timeline stream reconnecting".to_owned(),
                    )))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                if event_tx
                    .send(TimelineListenerEvent::Status(Err(format!(
                        "timeline stream offline: {error}"
                    ))))
                    .is_err()
                {
                    return;
                }
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn sample_data(sample: &Value) -> Option<Value> {
    let data = sample.get("data")?;
    if let Some(raw) = data.as_str() {
        serde_json::from_str(raw).ok()
    } else {
        Some(data.clone())
    }
}

fn sample_timestamp_ns(sample: &Value, data: &Value) -> i64 {
    data.get("sample_time_ns")
        .and_then(Value::as_i64)
        .or_else(|| {
            sample
                .get("timestamp_s")
                .and_then(Value::as_f64)
                .map(|seconds| (seconds * 1_000_000_000.0) as i64)
        })
        .unwrap_or_default()
}

fn parse_camera_frame(sample: &Value) -> Option<CameraFrame> {
    let data = sample_data(sample)?;
    let metadata = data.get("metadata")?;
    let width = usize::try_from(metadata.get("width")?.as_u64()?).ok()?;
    let height = usize::try_from(metadata.get("height")?.as_u64()?).ok()?;
    let channels = usize::try_from(
        metadata
            .get("channels")
            .and_then(Value::as_u64)
            .unwrap_or(3),
    )
    .ok()?;
    if width == 0 || height == 0 || width > 4096 || height > 4096 || channels < 3 {
        return None;
    }
    let required = width.checked_mul(height)?.checked_mul(channels)?;
    let bytes = BASE64.decode(data.get("payload_base64")?.as_str()?).ok()?;
    if bytes.len() < required {
        return None;
    }

    let encoding = metadata
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("bgr8");
    let mut rgb = Vec::with_capacity(width * height * 3);
    for pixel in bytes[..required].chunks_exact(channels) {
        if encoding.eq_ignore_ascii_case("rgb8") {
            rgb.extend_from_slice(&pixel[..3]);
        } else {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }

    Some(CameraFrame {
        sequence: data
            .get("sequence")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        timestamp_ns: sample_timestamp_ns(sample, &data),
        width,
        height,
        rgb,
    })
}

fn parse_audio_frame(sample: &Value) -> Option<AudioFrame> {
    let data = sample_data(sample)?;
    let metadata = data.get("metadata")?;
    let sample_rate_hz = metadata
        .get("sample_rate_hz")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    let channels = metadata
        .get("channels")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(1);
    let format = metadata
        .get("sample_format")
        .and_then(Value::as_str)
        .unwrap_or("s16le");
    let source = metadata
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let bytes = BASE64.decode(data.get("payload_base64")?.as_str()?).ok()?;

    let samples = match format {
        "s16le" => bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / i16::MAX as f32)
            .collect::<Vec<_>>(),
        "s24le" => bytes
            .chunks_exact(3)
            .map(|chunk| {
                let value = i32::from_le_bytes([
                    chunk[0],
                    chunk[1],
                    chunk[2],
                    if chunk[2] & 0x80 == 0 { 0 } else { 0xff },
                ]);
                value as f32 / 8_388_607.0
            })
            .collect::<Vec<_>>(),
        "s32le" => bytes
            .chunks_exact(4)
            .map(|chunk| {
                i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f32
                    / i32::MAX as f32
            })
            .collect::<Vec<_>>(),
        "f32le" => bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .filter(|sample| sample.is_finite())
            .map(|sample| sample.clamp(-1.0, 1.0))
            .collect::<Vec<_>>(),
        "u8" => bytes
            .iter()
            .map(|sample| (*sample as f32 - 128.0) / 128.0)
            .collect::<Vec<_>>(),
        _ => return None,
    };
    if samples.is_empty() {
        return None;
    }

    let rms =
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples
        .iter()
        .fold(0.0_f32, |current, sample| current.max(sample.abs()));
    Some(AudioFrame {
        timestamp_ns: sample_timestamp_ns(sample, &data),
        sample_rate_hz,
        channels,
        rms,
        peak,
        source: source.to_owned(),
        sample_format: format.to_owned(),
        // The UI visualizes a rolling block envelope. Raw PCM samples made a
        // live microphone look like static noise even when the signal was valid.
        levels: vec![rms],
    })
}

fn parse_pipeline_metrics(sample: &Value) -> Option<PipelineMetrics> {
    let data = sample.get("data")?;
    let pipeline = if data.get("transform").is_some() || data.get("streamer").is_some() {
        data
    } else {
        data.get("pipeline")?
    };

    let mut transforms = pipeline
        .get("transform")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(id, value)| TransformMetric {
            id: id.clone(),
            kind: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("transform")
                .to_owned(),
            latency_ms: value
                .get("transform_latency_avg_ms")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            jitter_ms: value
                .get("transform_latency_jitter_ms")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    transforms.sort_by(|left, right| left.id.cmp(&right.id));

    let mut streams = pipeline
        .get("streamer")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, value)| StreamMetric {
            name: name.clone(),
            source: format_connections(value.get("source")),
            targets: format_connections(value.get("targets")),
            frequency_hz: value
                .get("sample_frequency")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            input_lag_ms: value
                .get("sample_input_lag_ms")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            batch_size: value
                .get("batch_size_avg")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            total_samples: value
                .get("total_sample_count")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    streams.sort_by(|left, right| {
        right
            .frequency_hz
            .total_cmp(&left.frequency_hz)
            .then_with(|| left.name.cmp(&right.name))
    });

    Some(PipelineMetrics {
        status: pipeline
            .get("pipeline_status")
            .and_then(Value::as_str)
            .unwrap_or("running")
            .to_owned(),
        transforms,
        streams,
    })
}

fn format_connections(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_object)
        .map(|connections| {
            connections
                .iter()
                .map(|(transform, port)| format!("{transform}.{}", port.as_str().unwrap_or("port")))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn parse_timeline_event(stream: &str, sample: &Value) -> Option<TimelineEvent> {
    let data = sample.get("data")?;
    let data = if let Some(raw) = data.as_str() {
        serde_json::from_str::<Value>(raw).ok()?
    } else {
        data.clone()
    };
    let kind = data
        .get("event_type")
        .or_else(|| data.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| stream.split('_').next().unwrap_or("event"))
        .to_owned();
    let timestamp_ns = data
        .get("timestamp_ns")
        .or_else(|| data.get("sample_time_ns"))
        .and_then(Value::as_i64)
        .or_else(|| {
            sample
                .get("produced_timestamp_s")
                .and_then(Value::as_f64)
                .map(|seconds| (seconds * 1_000_000_000.0) as i64)
        })
        .unwrap_or_default();
    Some(TimelineEvent {
        id: data
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{kind}-{timestamp_ns}")),
        kind,
        timestamp_ns,
        title: data
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Model event")
            .to_owned(),
        summary: data
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        label: data
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        confidence: data
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or_default() as f32,
        node: data
            .get("node")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        model_name: data
            .get("model_name")
            .or_else(|| data.pointer("/model/name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        model_version: data
            .get("model_version")
            .or_else(|| data.pointer("/model/version"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        model_backend: data
            .get("model_backend")
            .or_else(|| data.pointer("/model/backend"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        model_runtime: data
            .get("model_runtime")
            .or_else(|| data.pointer("/model/runtime"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        inference_latency_ms: data
            .get("inference_latency_ms")
            .or_else(|| data.pointer("/model/latency_ms"))
            .and_then(Value::as_f64)
            .unwrap_or_default(),
    })
}

fn worker_loop(
    root: PathBuf,
    command_rx: mpsc::Receiver<WorkerCommand>,
    event_tx: mpsc::Sender<WorkerEvent>,
) {
    while let Ok(command) = command_rx.recv() {
        let event = match command {
            WorkerCommand::Probe => WorkerEvent::ProbeFinished(probe_engine(&root)),
            WorkerCommand::SetParameter {
                transform,
                parameter,
                value,
            } => WorkerEvent::ActionFinished(
                set_parameter(&transform, &parameter, value)
                    .map(|()| format!("Updated {transform}.{parameter}")),
            ),
            WorkerCommand::StartRecording(path) => {
                let result = start_recording(&path).map(|()| path);
                WorkerEvent::RecordingStarted(result)
            }
            WorkerCommand::StopRecording => WorkerEvent::RecordingStopped(
                set_parameter("rerun", "recording_path", Value::String(String::new()))
                    .map(|()| "RRD + HDF5 recording finalized and indexed".to_owned()),
            ),
            WorkerCommand::PrepareHdf5Playback(path) => {
                WorkerEvent::Hdf5PlaybackPrepared(prepare_hdf5_playback(&root, &path))
            }
            WorkerCommand::RunConfig(path) => {
                WorkerEvent::ConfigStarted(restart_stack(&root, &path).map(|()| path))
            }
            WorkerCommand::StopEngine => WorkerEvent::EngineStopped(
                stop_recorded_processes(&root).map(|()| "C++ engine and device stopped".to_owned()),
            ),
            WorkerCommand::StartSyntheticDevice => {
                WorkerEvent::ActionFinished(start_synthetic_device(&root).and_then(|pid| {
                    update_run_state_device_pid(&root, pid)?;
                    Ok(format!("Local camera/microphone device ready (PID {pid})"))
                }))
            }
        };
        if event_tx.send(event).is_err() {
            break;
        }
    }
}

fn probe_engine(root: &Path) -> Result<EngineSnapshot, String> {
    let pipeline = ws_request(json!({"get_pipeline": {}}))?;
    let feeds = ws_request(json!({"list_feeds_request": {}})).unwrap_or(Value::Null);
    let graph = pipeline
        .pointer("/api_event/get_pipeline_event/graph")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let feed_count = feeds
        .pointer("/api_event/feed_catalog/feeds")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    Ok(EngineSnapshot {
        connected: true,
        pipeline_name: pipeline_name_from_yaml(graph),
        active_config: recorded_config_path(root),
        engine_pid: recorded_process_id(root, "engine_pid"),
        device_pid: recorded_process_id(root, "device_pid"),
        feed_count,
        last_error: None,
    })
}

fn set_parameter(transform: &str, parameter: &str, value: Value) -> Result<(), String> {
    let encoded = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    let response = ws_request(json!({
        "change_parameter_request": {
            "transforms": {
                (transform): {
                    "parameters": {
                        (parameter): encoded
                    }
                }
            }
        }
    }))?;
    let accepted = response
        .pointer(&format!(
            "/api_event/change_parameter_event/results/{transform}/{parameter}"
        ))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if accepted {
        Ok(())
    } else {
        Err(format!("engine rejected parameter {transform}.{parameter}"))
    }
}

fn start_recording(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    set_parameter(
        "rerun",
        "recording_path",
        Value::String(path.to_string_lossy().into_owned()),
    )
}

fn prepare_hdf5_playback(root: &Path, source_path: &Path) -> Result<PreparedPlayback, String> {
    if !source_path.is_file() {
        return Err(format!("HDF5 session missing: {}", source_path.display()));
    }
    let script = root.join("scripts").join("hab_hdf5_to_rrd.py");
    if !script.is_file() {
        return Err(format!(
            "HDF5 playback adapter missing: {}",
            script.display()
        ));
    }
    let modified = fs::metadata(source_path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    let adapter_modified = fs::metadata(&script)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    let mut hasher = DefaultHasher::new();
    source_path.hash(&mut hasher);
    modified.hash(&mut hasher);
    adapter_modified.hash(&mut hasher);
    let cache_key = hasher.finish();
    let output_path = root
        .join("data")
        .join("playback_cache")
        .join(format!("{cache_key:016x}.hdf5-review.rrd"));
    if output_path.is_file() && fs::metadata(&output_path).is_ok_and(|metadata| metadata.len() > 0)
    {
        return Ok(PreparedPlayback {
            source_path: source_path.to_owned(),
            rrd_path: output_path,
        });
    }
    let _ = fs::remove_file(&output_path);
    let partial_path = output_path.with_extension("rrd.partial");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let _ = fs::remove_file(&partial_path);
    let python = if let Ok(explicit) = std::env::var("HAB_PYTHON") {
        PathBuf::from(explicit)
    } else if cfg!(target_os = "windows") {
        root.join(".venv").join("Scripts").join("python.exe")
    } else {
        root.join(".venv").join("bin").join("python")
    };
    let mut command = Command::new(&python);
    command
        .current_dir(root)
        .arg(&script)
        .arg("--h5")
        .arg(source_path)
        .arg("--out")
        .arg(&partial_path)
        .arg("--app-id")
        .arg("hab");
    hide_window(&mut command);
    let output = command.output().map_err(|error| {
        format!(
            "failed to start {} for HDF5 playback: {error}",
            python.display()
        )
    })?;
    if !output.status.success() {
        let _ = fs::remove_file(&partial_path);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("HDF5 adapter exited with {}", output.status)
        } else {
            stderr
        });
    }
    if !partial_path.is_file()
        || fs::metadata(&partial_path).map_or(true, |metadata| metadata.len() == 0)
    {
        let _ = fs::remove_file(&partial_path);
        return Err(format!(
            "HDF5 adapter produced no RRD: {}",
            partial_path.display()
        ));
    }
    fs::rename(&partial_path, &output_path).map_err(|error| {
        let _ = fs::remove_file(&partial_path);
        format!(
            "failed to publish HDF5 playback cache {}: {error}",
            output_path.display()
        )
    })?;
    Ok(PreparedPlayback {
        source_path: source_path.to_owned(),
        rrd_path: output_path,
    })
}

fn ws_request(kind: Value) -> Result<Value, String> {
    let request_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let mut request = kind
        .as_object()
        .cloned()
        .ok_or_else(|| "WebSocket request must be an object".to_owned())?;
    request.insert("request_id".to_owned(), Value::from(request_id));
    let payload = json!({
        "api_version": "0.12",
        "api_request": Value::Object(request),
    });

    let (mut socket, _) =
        connect(ENGINE_ENDPOINT).map_err(|error| format!("engine offline: {error}"))?;
    socket
        .send(Message::Text(payload.to_string().into()))
        .map_err(|error| error.to_string())?;
    for _ in 0..32 {
        match socket.read().map_err(|error| error.to_string())? {
            Message::Text(text) => {
                let value: Value =
                    serde_json::from_str(text.as_str()).map_err(|error| error.to_string())?;
                if value
                    .pointer("/api_event/request_id")
                    .and_then(Value::as_u64)
                    == Some(request_id)
                {
                    let _ = socket.close(None);
                    return Ok(value);
                }
            }
            Message::Ping(payload) => {
                socket
                    .send(Message::Pong(payload))
                    .map_err(|error| error.to_string())?;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    Err("engine closed before acknowledging the request".to_owned())
}

fn restart_stack(root: &Path, config: &Path) -> Result<(), String> {
    stop_recorded_processes(root)?;
    wait_for_port(false, Duration::from_secs(5))?;

    let engine = root
        .join("build-native")
        .join("bin")
        .join("Release")
        .join("hab_ctrl_engine.exe");
    if !engine.is_file() {
        return Err(format!("engine executable missing: {}", engine.display()));
    }
    if !config.is_file() {
        return Err(format!("config missing: {}", config.display()));
    }

    let logs = root.join("artifacts").join("hab-shell");
    fs::create_dir_all(&logs).map_err(|error| error.to_string())?;
    let stdout =
        fs::File::create(logs.join("engine.stdout.log")).map_err(|error| error.to_string())?;
    let stderr =
        fs::File::create(logs.join("engine.stderr.log")).map_err(|error| error.to_string())?;
    let mut command = Command::new(&engine);
    command
        .current_dir(root)
        .arg("run")
        .arg(config)
        .args(["--ws-port", "9999", "--zmq-port", "8765"])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    hide_window(&mut command);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let Err(error) = wait_for_port(true, Duration::from_secs(20)) {
        let _ = child.kill();
        return Err(error);
    }

    let raw_config = fs::read_to_string(config).unwrap_or_default();
    let device_pid = if raw_config.contains("ClientPushSource") {
        Some(start_synthetic_device(root)?)
    } else {
        None
    };
    write_run_state(root, child.id(), device_pid, config)?;
    Ok(())
}

fn start_synthetic_device(root: &Path) -> Result<u32, String> {
    if TcpStream::connect_timeout(&ENGINE_ADDRESS, Duration::from_millis(300)).is_err() {
        return Err("start the C++ engine before connecting a device".to_owned());
    }
    if let Some(pid) = recorded_process_id(root, "device_pid")
        && process_is_running(pid)
    {
        return Ok(pid);
    }
    let executable =
        PathBuf::from(r"C:\Dev\Hab-Synthetic-Device\build\Release\hab-synthetic-device.exe");
    if !executable.is_file() {
        return Err(format!(
            "synthetic device executable missing: {}",
            executable.display()
        ));
    }
    let logs = root.join("artifacts").join("hab-shell");
    fs::create_dir_all(&logs).map_err(|error| error.to_string())?;
    let stdout =
        fs::File::create(logs.join("device.stdout.log")).map_err(|error| error.to_string())?;
    let stderr =
        fs::File::create(logs.join("device.stderr.log")).map_err(|error| error.to_string())?;
    let mut command = Command::new(&executable);
    command
        .current_dir(executable.parent().unwrap_or(root))
        .args(["--host", "127.0.0.1", "--port", "9999"])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    hide_window(&mut command);
    command
        .spawn()
        .map(|child| child.id())
        .map_err(|error| error.to_string())
}

fn recorded_process_id(root: &Path, key: &str) -> Option<u32> {
    let path = root
        .join("artifacts")
        .join("inspector-live")
        .join("run.json");
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path).ok()?).ok()?;
    value.get(key)?.as_u64().map(|pid| pid as u32)
}

fn recorded_config_path(root: &Path) -> Option<PathBuf> {
    let path = root
        .join("artifacts")
        .join("inspector-live")
        .join("run.json");
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path).ok()?).ok()?;
    value
        .get("config")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

fn update_run_state_device_pid(root: &Path, pid: u32) -> Result<(), String> {
    let path = root
        .join("artifacts")
        .join("inspector-live")
        .join("run.json");
    if !path.is_file() {
        return Ok(());
    }
    let mut value = serde_json::from_str::<Value>(
        &fs::read_to_string(&path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    value["device_pid"] = Value::from(pid);
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn process_is_running(pid: u32) -> bool {
    let mut command = Command::new("tasklist");
    command.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
    hide_window(&mut command);
    command.output().is_ok_and(|output| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.contains(&format!("\"{pid}\"")))
    })
}

#[cfg(not(target_os = "windows"))]
fn process_is_running(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

fn stop_recorded_processes(root: &Path) -> Result<(), String> {
    let Some((run_state, value)) = read_run_state(root)? else {
        return Ok(());
    };
    stop_processes_in_state(&value)?;
    remove_run_state(&run_state)
}

fn stop_owned_processes(root: &Path) -> Result<(), String> {
    let Some((run_state, value)) = read_run_state(root)? else {
        return Ok(());
    };
    if value.get("viewer_pid").and_then(Value::as_u64) != Some(std::process::id() as u64) {
        return Ok(());
    }
    stop_processes_in_state(&value)?;
    remove_run_state(&run_state)
}

fn read_run_state(root: &Path) -> Result<Option<(PathBuf, Value)>, String> {
    let path = root
        .join("artifacts")
        .join("inspector-live")
        .join("run.json");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    Ok(Some((path, value)))
}

fn remove_run_state(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to remove run state {}: {error}",
            path.display()
        )),
    }
}

fn stop_processes_in_state(value: &Value) -> Result<(), String> {
    let mut errors = Vec::new();
    for (key, sentinel, timeout) in [
        (
            "device_pid",
            "hab_synthetic_device.shutdown",
            Duration::from_secs(5),
        ),
        (
            "engine_pid",
            "hab_ctrl_engine.shutdown",
            Duration::from_secs(10),
        ),
    ] {
        let Some(pid) = value.get(key).and_then(Value::as_u64) else {
            continue;
        };
        if let Err(error) = stop_process_gracefully(pid as u32, sentinel, timeout) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(target_os = "windows")]
fn stop_process_gracefully(
    pid: u32,
    sentinel_prefix: &str,
    timeout: Duration,
) -> Result<(), String> {
    if !process_is_running(pid) {
        return Ok(());
    }

    let sentinel = std::env::temp_dir().join(format!("{sentinel_prefix}.{pid}"));
    fs::write(&sentinel, b"shutdown").map_err(|error| {
        format!(
            "failed to request graceful shutdown for PID {pid} via {}: {error}",
            sentinel.display()
        )
    })?;

    let deadline = Instant::now() + timeout;
    while process_is_running(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    let _ = fs::remove_file(&sentinel);

    if process_is_running(pid) {
        stop_process(pid);
        let force_deadline = Instant::now() + Duration::from_secs(2);
        while process_is_running(pid) && Instant::now() < force_deadline {
            thread::sleep(Duration::from_millis(100));
        }
    }

    if process_is_running(pid) {
        Err(format!("process PID {pid} did not stop"))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn stop_process_gracefully(
    pid: u32,
    _sentinel_prefix: &str,
    timeout: Duration,
) -> Result<(), String> {
    if !process_is_running(pid) {
        return Ok(());
    }
    stop_process(pid);
    let deadline = Instant::now() + timeout;
    while process_is_running(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if process_is_running(pid) {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output();
    }
    if process_is_running(pid) {
        Err(format!("process PID {pid} did not stop"))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn stop_process(pid: u32) {
    let mut command = Command::new("taskkill");
    command.arg("/PID").arg(pid.to_string()).args(["/T", "/F"]);
    hide_window(&mut command);
    let _ = command.output();
}

#[cfg(not(target_os = "windows"))]
fn stop_process(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).output();
}

#[cfg(target_os = "windows")]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(target_os = "windows"))]
fn hide_window(_command: &mut Command) {}

fn wait_for_port(open: bool, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let connected =
            TcpStream::connect_timeout(&ENGINE_ADDRESS, Duration::from_millis(200)).is_ok();
        if connected == open {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(if open {
                "C++ engine did not open WebSocket port 9999".to_owned()
            } else {
                "previous C++ engine did not release WebSocket port 9999".to_owned()
            });
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn write_run_state(
    root: &Path,
    engine_pid: u32,
    device_pid: Option<u32>,
    config: &Path,
) -> Result<(), String> {
    let directory = root.join("artifacts").join("inspector-live");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let state = json!({
        "engine_pid": engine_pid,
        "device_pid": device_pid,
        "viewer_pid": std::process::id(),
        "started_at_epoch_s": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "config": config,
    });
    fs::write(
        directory.join("run.json"),
        serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn truncate_words(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut result = String::new();
    for word in value.split_whitespace() {
        let separator = usize::from(!result.is_empty());
        if result.chars().count() + separator + word.chars().count() > max_chars {
            break;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(word);
    }
    result.push('…');
    result
}

fn discover_configs(root: &Path) -> Vec<ConfigInfo> {
    let directory = root.join("configs").join("examples");
    let mut configs = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
            if extension != "yaml" && extension != "yml" {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            let raw = fs::read_to_string(&path).unwrap_or_default();
            let summary = raw
                .lines()
                .map(str::trim)
                .skip_while(|line| line.is_empty())
                .take_while(|line| line.starts_with('#'))
                .map(|line| line.trim_start_matches('#').trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let summary = if summary.is_empty() {
                "Hab pipeline configuration".to_owned()
            } else {
                truncate_words(&summary, 180)
            };
            Some(ConfigInfo {
                name: path.file_name()?.to_string_lossy().into_owned(),
                path,
                summary,
                size_bytes: metadata.len(),
                modified: metadata.modified().ok(),
            })
        })
        .collect::<Vec<_>>();
    configs.sort_by(|left, right| {
        let left_priority = usize::from(left.name != "inspector_live.yaml");
        let right_priority = usize::from(right.name != "inspector_live.yaml");
        (left_priority, &left.name).cmp(&(right_priority, &right.name))
    });
    configs
}

fn discover_sessions(root: &Path) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    collect_sessions(&root.join("data").join("study_sessions"), &mut sessions, 0);
    sessions.sort_by(|left, right| right.modified.cmp(&left.modified));
    sessions
}

fn collect_sessions(directory: &Path, sessions: &mut Vec<SessionInfo>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let entries = entries.flatten().collect::<Vec<_>>();
    let mut rrd_paths = Vec::new();
    let mut hdf5_paths = Vec::new();
    let mut child_directories = Vec::new();
    let mut artifacts = Vec::new();
    for entry in &entries {
        let path = entry.path();
        if path.is_dir() {
            child_directories.push(path);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        artifacts.push(entry.file_name().to_string_lossy().into_owned());
        let extension = path
            .extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase());
        match extension.as_deref() {
            Some("rrd") => rrd_paths.push(path),
            Some("h5" | "hdf5") => hdf5_paths.push(path),
            _ => {}
        }
    }

    if !rrd_paths.is_empty() || !hdf5_paths.is_empty() {
        rrd_paths.sort();
        hdf5_paths.sort();
        let preferred = |paths: &[PathBuf], name: &str| {
            paths
                .iter()
                .find(|path| path.file_name().is_some_and(|value| value == name))
                .cloned()
                .or_else(|| paths.first().cloned())
        };
        let rrd_path = preferred(&rrd_paths, "session.rrd");
        let hdf5_path = preferred(&hdf5_paths, "session.h5");
        let path = rrd_path
            .as_ref()
            .or(hdf5_path.as_ref())
            .expect("session has at least one representation")
            .clone();
        let format = match (rrd_path.is_some(), hdf5_path.is_some()) {
            (true, true) => "RRD + HDF5",
            (true, false) => "RRD",
            (false, true) => "HDF5",
            (false, false) => unreachable!(),
        };
        let name = directory
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let participant = directory
            .parent()
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "local".to_owned());
        let study = directory
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unassigned".to_owned());
        let representation_metadata = rrd_paths
            .iter()
            .chain(&hdf5_paths)
            .filter_map(|path| fs::metadata(path).ok())
            .collect::<Vec<_>>();
        let size_bytes = representation_metadata.iter().map(fs::Metadata::len).sum();
        let modified = representation_metadata
            .iter()
            .filter_map(|metadata| metadata.modified().ok())
            .max();
        artifacts.sort();
        let annotation_preview = ["annotations.csv", "annotation.csv", "events.csv"]
            .iter()
            .find_map(|name| {
                let annotation_path = directory.join(name);
                fs::read_to_string(annotation_path).ok().map(|raw| {
                    raw.lines()
                        .filter(|line| !line.trim().is_empty())
                        .take(4)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        sessions.push(SessionInfo {
            path,
            rrd_path,
            hdf5_path,
            name,
            study,
            participant,
            format,
            size_bytes,
            modified,
            artifacts,
            annotation_preview,
        });
    }

    for child in child_directories {
        collect_sessions(&child, sessions, depth + 1);
    }
}

fn find_hab_root() -> PathBuf {
    let candidates = std::env::current_dir()
        .into_iter()
        .chain([PathBuf::from(env!("CARGO_MANIFEST_DIR"))]);
    for candidate in candidates {
        for ancestor in candidate.ancestors() {
            if ancestor.join("configs").join("examples").is_dir()
                && ancestor.join("engine_cpp").is_dir()
            {
                return ancestor.to_owned();
            }
        }
    }
    PathBuf::from(r"C:\Dev\Hab")
}

fn pipeline_name_from_file(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| pipeline_name_from_yaml(&raw))
}

fn pipeline_name_from_yaml(raw: &str) -> Option<String> {
    let mut in_module = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == "module:" {
            in_module = true;
            continue;
        }
        if in_module {
            if let Some(name) = trimmed.strip_prefix("name:") {
                return Some(name.trim().trim_matches('"').to_owned());
            }
            if !line.starts_with(' ') && !trimmed.is_empty() {
                in_module = false;
            }
        }
    }
    None
}

pub fn human_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub fn human_age(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return "unknown date".to_owned();
    };
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        .as_secs();
    match age {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{} min ago", age / 60),
        3_600..=86_399 => format!("{} h ago", age / 3_600),
        86_400..=2_591_999 => format!("{} d ago", age / 86_400),
        _ => format!("{} mo ago", age / 2_592_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_shipping_configs() {
        let root = find_hab_root();
        let configs = discover_configs(&root);
        assert!(configs.len() > 5);
        assert_eq!(configs[0].name, "inspector_live.yaml");
    }

    #[test]
    fn parses_pipeline_name() {
        assert_eq!(
            pipeline_name_from_yaml("version: 2\nmodule:\n  name: inspector_live\n"),
            Some("inspector_live".to_owned())
        );
    }

    #[test]
    fn groups_rrd_and_hdf5_as_one_session() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "hab-shell-session-index-{}-{nonce}",
            std::process::id()
        ));
        let session = root
            .join("data")
            .join("study_sessions")
            .join("inspector_live")
            .join("local")
            .join("session-test");
        fs::create_dir_all(&session).expect("test session directory");
        fs::write(session.join("session.rrd"), b"rrd").expect("test RRD");
        fs::write(session.join("session.h5"), b"hdf5").expect("test HDF5");

        let sessions = discover_sessions(&root);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].format, "RRD + HDF5");
        assert_eq!(
            sessions[0].rrd_path.as_deref(),
            Some(session.join("session.rrd").as_path())
        );
        assert_eq!(
            sessions[0].hdf5_path.as_deref(),
            Some(session.join("session.h5").as_path())
        );
        assert_eq!(sessions[0].size_bytes, 7);

        fs::remove_dir_all(&root).expect("remove isolated test directory");
    }

    #[test]
    fn parses_timeline_stream_sample_without_media_payload() {
        let event = parse_timeline_event(
            "grasp_detector.events",
            &json!({
                "produced_timestamp_s": 42.5,
                "data": {
                    "id": "grasp-1",
                    "event_type": "grasp",
                    "timestamp_ns": 42_500_000_000i64,
                    "title": "Grasp detected",
                    "summary": "Object held",
                    "label": "object",
                    "confidence": 0.82,
                    "node": "grasp_detector",
                    "model": {
                        "name": "grasp_v4",
                        "version": "4.1.0",
                        "backend": "cpp_hook",
                        "runtime": "native_cpp",
                        "latency_ms": 3.25
                    },
                    "media": {"payload_base64": "discarded"}
                }
            }),
        )
        .expect("event should parse");
        assert_eq!(event.kind, "grasp");
        assert_eq!(event.title, "Grasp detected");
        assert_eq!(event.clock_time(), "00:00:42.500");
        assert!((event.confidence - 0.82).abs() < f32::EPSILON);
        assert_eq!(event.model_name, "grasp_v4");
        assert_eq!(event.model_version, "4.1.0");
        assert_eq!(event.model_backend, "cpp_hook");
        assert_eq!(event.model_runtime, "native_cpp");
        assert!((event.inference_latency_ms - 3.25).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_pipeline_monitor_metrics() {
        let metrics = parse_pipeline_metrics(&json!({
            "data": {
                "pipeline_status": "running",
                "transform": {
                    "camera": {
                        "type": "middle",
                        "transform_latency_avg_ms": 0.2,
                        "transform_latency_jitter_ms": 0.05
                    }
                },
                "streamer": {
                    "camera.frame": {
                        "source": {"camera": "frame"},
                        "targets": {"rerun": "camera"},
                        "sample_frequency": 30.0,
                        "sample_input_lag_ms": 4.0,
                        "batch_size_avg": 1.0,
                        "total_sample_count": 120
                    }
                }
            }
        }))
        .expect("pipeline metrics should parse");
        assert_eq!(metrics.status, "running");
        assert_eq!(metrics.transforms[0].id, "camera");
        assert_eq!(metrics.streams[0].source, "camera.frame");
        assert_eq!(metrics.streams[0].total_samples, 120);
    }

    #[test]
    fn decodes_bgr_camera_preview_as_rgb() {
        let frame = parse_camera_frame(&json!({
            "timestamp_s": 1.25,
            "data": {
                "sequence": 7,
                "sample_time_ns": 1_250_000_000i64,
                "metadata": {
                    "width": 2,
                    "height": 1,
                    "channels": 3,
                    "encoding": "bgr8"
                },
                "payload_base64": BASE64.encode([0, 10, 255, 100, 50, 0])
            }
        }))
        .expect("camera frame should decode");
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.sequence, 7);
        assert_eq!(frame.rgb, [255, 10, 0, 0, 50, 100]);
    }

    #[test]
    fn computes_audio_preview_from_s16le() {
        let samples = [-32_768_i16, 0, 16_384, 32_767];
        let bytes = samples
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let frame = parse_audio_frame(&json!({
            "timestamp_s": 2.0,
            "data": {
                "sample_time_ns": 2_000_000_000i64,
                "metadata": {
                    "sample_rate_hz": 16_000,
                    "channels": 1,
                    "sample_format": "s16le"
                },
                "payload_base64": BASE64.encode(bytes)
            }
        }))
        .expect("audio frame should decode");
        assert_eq!(frame.sample_rate_hz, 16_000);
        assert_eq!(frame.channels, 1);
        assert_eq!(frame.levels, [frame.rms]);
        assert!(frame.rms > 0.7 && frame.rms < 0.8);
        assert!(frame.peak > 0.99);
        assert_eq!(frame.source, "unknown");
    }
}

//! Native audio worker and a bounded, nonblocking application-side client.
//!
//! Device callbacks never perform project/journal/IPC work. Musical note audio
//! is a procedural audition instrument, not model inference.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub mod live_types;
pub use live_types::*;
pub mod performance_records;
pub mod performance_tap;
pub mod performance_writer;
pub mod waveform_types;
pub use waveform_types::*;
pub mod waveform;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum EngineCommand {
    Devices,
    StartPlayback {
        project_id: String,
        output_device: Option<String>,
    },
    StartLive {
        output_device: Option<String>,
        gui_clock: LiveClock,
    },
    Stop,
    StartCapture {
        input_device: Option<String>,
    },
    FinishCapture,
    Status,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeviceInventory {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureIdentity {
    pub journal_path: PathBuf,
    pub take_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EngineStatus {
    pub state: String,
    pub capture: Option<CaptureIdentity>,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub peak: f32,
    pub xruns: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CapturedTake {
    pub journal_path: PathBuf,
    pub take_id: String,
    pub manifest_sha256: String,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub gap_frames: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "result",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EngineResult {
    Initialized {
        profile: String,
        worker_generation: LiveId,
    },
    LiveStarted(LiveReady),
    LiveStatus(LiveProgress),
    Devices(DeviceInventory),
    Playing(EngineStatus),
    Stopped,
    Capturing(EngineStatus),
    Captured(CapturedTake),
    Status(EngineStatus),
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EngineReply {
    pub request_id: u64,
    pub result: Result<EngineResult, String>,
}

pub mod capture;
pub mod live;
mod live_supervisor;
pub mod realtime;
pub mod render;
mod transport;
mod worker;
pub use transport::EngineClient;
pub use worker::run_worker;

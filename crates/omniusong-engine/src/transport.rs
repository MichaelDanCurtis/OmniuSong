use crate::{EngineCommand, EngineReply, EngineResult};
use omniusong_core::contracts::{
    Architecture, ContractError, Platform, Validate, WirePayload, WorkerRole,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::Duration,
};
const WIRE_BYTES: usize = 64 * 1024;
pub(crate) const BUILD: &str = concat!("omniusong-engine-", env!("CARGO_PKG_VERSION"));
#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    Initialize { store_root: PathBuf },
    Command { command: EngineCommand },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub request_id: u64,
    pub action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    pub reply: EngineReply,
}
fn require(ok: bool, s: &str) -> Result<(), ContractError> {
    if ok {
        Ok(())
    } else {
        Err(ContractError(s.into()))
    }
}
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}
pub(crate) fn command_valid(command: &EngineCommand) -> Result<(), String> {
    let device = match command {
        EngineCommand::StartPlayback {
            project_id,
            output_device,
        } => {
            if !text(project_id, 200) {
                return Err("Invalid project identity".into());
            }
            output_device
        }
        EngineCommand::StartCapture { input_device } => input_device,
        _ => return Ok(()),
    };
    if device.as_ref().is_some_and(|s| !text(s, 1024)) {
        return Err("Invalid device identity".into());
    }
    Ok(())
}
impl Validate for Request {
    fn validate(&self) -> Result<(), ContractError> {
        match &self.action {
            Action::Initialize { store_root } => require(
                store_root.is_absolute() && store_root.to_string_lossy().len() <= 4096,
                "Invalid engine store path",
            ),
            Action::Command { command } => command_valid(command).map_err(ContractError),
        }
    }
}
impl WirePayload for Request {
    const PROTOCOL: &'static str = "omniusong.engine-control.v1";
    const MAX_BYTES: usize = WIRE_BYTES;
}
impl Validate for Response {
    fn validate(&self) -> Result<(), ContractError> {
        match &self.reply.result {
            Err(s) => require(text(s, 16 * 1024), "Invalid engine error"),
            Ok(EngineResult::Devices(d)) => {
                require(
                    d.inputs.len() <= 64 && d.outputs.len() <= 64,
                    "Too many devices",
                )?;
                for item in d.inputs.iter().chain(&d.outputs) {
                    require(
                        text(&item.id, 1024) && text(&item.name, 1024),
                        "Invalid device descriptor",
                    )?;
                }
                Ok(())
            }
            Ok(EngineResult::Status(s) | EngineResult::Playing(s) | EngineResult::Capturing(s)) => {
                require(
                    matches!(
                        s.state.as_str(),
                        "idle"
                            | "playing"
                            | "playback_finished"
                            | "capturing"
                            | "capture_stopped_transfer_overflow"
                            | "capture_fault"
                    ),
                    "Invalid engine state",
                )?;
                require(
                    s.peak.is_finite() && s.peak >= 0.0 && s.frames <= i64::MAX as u64,
                    "Invalid engine meter",
                )?;
                if s.state == "idle" {
                    require(
                        s.sample_rate == 0
                            && s.channels == 0
                            && s.frames == 0
                            && s.peak == 0.0
                            && s.xruns == 0,
                        "Idle status contains active configuration or meters",
                    )?;
                } else {
                    require(
                        (8000..=768000).contains(&s.sample_rate) && (1..=8).contains(&s.channels),
                        "Active status has invalid audio configuration",
                    )?;
                }
                match &self.reply.result {
                    Ok(EngineResult::Playing(_)) => require(
                        matches!(s.state.as_str(), "playing" | "playback_finished"),
                        "Playing result contains a different lifecycle state",
                    )?,
                    Ok(EngineResult::Capturing(_)) => require(
                        s.state.starts_with("captur"),
                        "Capturing result contains a different lifecycle state",
                    )?,
                    _ => {}
                }
                if let Some(c) = &s.capture {
                    require(
                        text(&c.take_id, 96)
                            && c.journal_path.is_absolute()
                            && c.journal_path.to_string_lossy().len() <= 4096,
                        "Invalid capture identity",
                    )?;
                }
                require(
                    s.state.starts_with("captur") == s.capture.is_some(),
                    "Missing or unexpected capture identity",
                )
            }
            Ok(EngineResult::Captured(c)) => require(
                c.journal_path.is_absolute()
                    && c.journal_path.to_string_lossy().len() <= 4096
                    && text(&c.take_id, 96)
                    && c.manifest_sha256.len() == 64
                    && c.manifest_sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                    && c.frames > 0
                    && c.frames <= i64::MAX as u64
                    && c.gap_frames <= c.frames
                    && (8000..=768000).contains(&c.sample_rate)
                    && (1..=8).contains(&c.channels),
                "Invalid captured take receipt",
            ),
            Ok(EngineResult::Stopped) => Ok(()),
        }
    }
}
impl WirePayload for Response {
    const PROTOCOL: &'static str = "omniusong.engine-reply.v1";
    const MAX_BYTES: usize = WIRE_BYTES;
}
fn matches_result(command: &EngineCommand, result: &Result<EngineResult, String>) -> bool {
    match result {
        Err(_) => true,
        Ok(result) => matches!(
            (command, result),
            (EngineCommand::Devices, EngineResult::Devices(_))
                | (
                    EngineCommand::StartPlayback { .. },
                    EngineResult::Playing(_)
                )
                | (EngineCommand::Stop, EngineResult::Stopped)
                | (
                    EngineCommand::StartCapture { .. },
                    EngineResult::Capturing(_)
                )
                | (EngineCommand::FinishCapture, EngineResult::Captured(_))
                | (EngineCommand::Status, EngineResult::Status(_))
        ),
    }
}
struct Pending {
    id: u64,
    command: EngineCommand,
}
enum Event {
    Completed(EngineReply),
    Failed(String),
}
/// One admitted request until its checked response is consumed. Drop never joins.
pub struct EngineClient {
    requests: SyncSender<Pending>,
    replies: Receiver<Event>,
    pending: Option<Pending>,
    next: u64,
    dead: Arc<AtomicBool>,
}
impl EngineClient {
    pub fn spawn(executable: PathBuf, store_root: PathBuf) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let dead = Arc::new(AtomicBool::new(false));
        let actor_dead = dead.clone();
        thread::Builder::new()
            .name("audio-supervisor".into())
            .spawn(move || {
                // Returning retires the owned child before any wait for UI credit.
                let result = actor(executable, store_root, rx, &result_tx);
                actor_dead.store(true, Ordering::Release);
                if let Err(error) = result {
                    // Capacity stays one; retain the terminal cause behind an
                    // already-queued completion. Dropping the UI receiver wakes
                    // this off-UI sender. The UI never joins this thread.
                    let _ = result_tx.send(Event::Failed(error));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            requests: tx,
            replies: result_rx,
            pending: None,
            next: 1,
            dead,
        })
    }
    pub fn try_submit(&mut self, command: EngineCommand) -> Result<(), String> {
        command_valid(&command)?;
        if self.pending.is_some() {
            return Err("Audio engine is busy".into());
        }
        if self.dead.load(Ordering::Acquire) {
            return Err("Audio worker is disconnected; unsealed originals are retained".into());
        }
        let next = self
            .next
            .checked_add(1)
            .ok_or("Engine request sequence exhausted")?;
        let pending = Pending {
            id: self.next,
            command,
        };
        match self.requests.try_send(Pending {
            id: pending.id,
            command: pending.command.clone(),
        }) {
            Ok(()) => {
                self.pending = Some(pending);
                self.next = next;
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err("Audio request queue is full".into()),
            Err(TrySendError::Disconnected(_)) => Err("Audio worker disconnected".into()),
        }
    }
    pub fn try_poll(&mut self) -> Option<EngineReply> {
        match self.replies.try_recv() {
            Ok(Event::Completed(reply)) => {
                let valid = self.pending.as_ref().is_some_and(|p| {
                    p.id == reply.request_id && matches_result(&p.command, &reply.result)
                });
                if valid {
                    self.pending = None;
                    Some(reply)
                } else {
                    self.dead.store(true, Ordering::Release);
                    Some(EngineReply {
                        request_id: self.pending.take().map_or(0, |p| p.id),
                        result: Err("Audio response correlation failed".into()),
                    })
                }
            }
            Ok(Event::Failed(error)) => {
                self.dead.store(true, Ordering::Release);
                Some(EngineReply {
                    request_id: self.pending.take().map_or(0, |p| p.id),
                    result: Err(error),
                })
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(p) = self.pending.take() {
                    Some(EngineReply {
                        request_id: p.id,
                        result: Err(
                            "Audio supervisor disconnected; unsealed originals are retained".into(),
                        ),
                    })
                } else {
                    None
                }
            }
        }
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
}
pub(crate) fn architecture() -> Architecture {
    if cfg!(target_arch = "aarch64") {
        Architecture::Aarch64
    } else {
        Architecture::X86_64
    }
}
pub(crate) fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Windows
    }
}
#[cfg(unix)]
fn actor(
    executable: PathBuf,
    store_root: PathBuf,
    requests: Receiver<Pending>,
    replies: &SyncSender<Event>,
) -> Result<(), String> {
    use omniusong_ipc::{
        FrameCeiling,
        unix::{LaunchConfig, default_worker_limits, spawn_private},
    };
    let mut command = std::process::Command::new(executable);
    command
        .arg("--audio-worker")
        .stdout(std::process::Stdio::null());
    let mut child = spawn_private(
        command,
        LaunchConfig {
            expected_role: WorkerRole::Engine,
            expected_platform: platform(),
            expected_architecture: architecture(),
            expected_build_id: Some(BUILD.into()),
            limits: default_worker_limits(),
            frame_ceiling: FrameCeiling::new(WIRE_BYTES).map_err(|e| e.to_string())?,
            handshake_timeout: Duration::from_secs(5),
        },
    )
    .map_err(|e| e.to_string())?;
    fn exchange(
        child: &mut omniusong_ipc::unix::PrivateChild,
        id: u64,
        action: Action,
    ) -> Result<EngineReply, String> {
        let budget = Duration::from_secs(15);
        let deadline = std::time::Instant::now() + budget;
        child
            .send_payload(
                Request {
                    request_id: id,
                    action,
                },
                budget,
            )
            .map_err(|e| e.to_string())?;
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or("Audio request deadline exhausted")?;
        let response = child
            .receive::<Response>(remaining)
            .map_err(|e| {
                format!(
                    "Audio worker deadline/channel failure: {e}; unsealed originals are retained"
                )
            })?
            .ok_or("Audio worker closed its private channel")?
            .payload
            .reply;
        if response.request_id != id {
            return Err("Audio private response identity mismatch".into());
        }
        Ok(response)
    }
    let init = exchange(&mut child, 0, Action::Initialize { store_root })?;
    match init.result {
        Ok(EngineResult::Stopped) => {}
        Err(e) => return Err(e),
        _ => return Err("Unexpected audio initialization result".into()),
    };
    let mut wire_id = 1u64;
    loop {
        let pending = match requests.recv_timeout(Duration::from_millis(250)) {
            Ok(p) => Some(p),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = exchange(
                    &mut child,
                    wire_id,
                    Action::Command {
                        command: EngineCommand::Stop,
                    },
                );
                return Ok(());
            }
        };
        let command = pending
            .as_ref()
            .map(|p| p.command.clone())
            .unwrap_or(EngineCommand::Status);
        let mut response = exchange(
            &mut child,
            wire_id,
            Action::Command {
                command: command.clone(),
            },
        )?;
        wire_id = wire_id
            .checked_add(1)
            .ok_or("Audio wire sequence exhausted")?;
        if !matches_result(&command, &response.result) {
            return Err("Audio result kind mismatch".into());
        }
        if let Some(p) = pending {
            response.request_id = p.id;
            replies
                .send(Event::Completed(response))
                .map_err(|_| "Audio UI receiver closed")?;
        } else {
            // An unsolicited heartbeat can consume a one-shot device fault.
            // Retire before another status can replace that cause with idle;
            // the supervisor retains this terminal error behind any queued
            // completion until the UI can consume it.
            response.result?;
        }
    }
}
#[cfg(not(unix))]
fn actor(
    _: PathBuf,
    _: PathBuf,
    _: Receiver<Pending>,
    _: &SyncSender<Event>,
) -> Result<(), String> {
    Err("Private audio worker launch is not implemented on this platform".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_and_results_are_strict() {
        assert!(
            serde_json::from_str::<Request>(
                r#"{"request_id":1,"action":"command","command":{"command":"stop","extra":1}}"#
            )
            .is_err()
        );
        assert!(!matches_result(
            &EngineCommand::FinishCapture,
            &Ok(EngineResult::Stopped)
        ));
    }
    fn peer() -> (EngineClient, Receiver<Pending>, SyncSender<Event>) {
        let (tx, rx) = mpsc::sync_channel(1);
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        (
            EngineClient {
                requests: tx,
                replies: reply_rx,
                pending: None,
                next: 1,
                dead: Arc::new(AtomicBool::new(false)),
            },
            rx,
            reply_tx,
        )
    }
    #[test]
    fn admission_remains_busy_after_actor_consumes_request() {
        let (mut client, requests, replies) = peer();
        client.try_submit(EngineCommand::Stop).unwrap();
        let request = requests.try_recv().unwrap();
        assert!(client.try_submit(EngineCommand::Devices).is_err());
        replies
            .send(Event::Completed(EngineReply {
                request_id: request.id,
                result: Ok(EngineResult::Stopped),
            }))
            .unwrap();
        assert!(client.is_busy());
        assert!(client.try_poll().unwrap().result.is_ok());
        assert!(!client.is_busy());
        client.try_submit(EngineCommand::Devices).unwrap();
    }
    #[test]
    fn mismatched_result_retires_client_before_new_admission() {
        let (mut client, requests, replies) = peer();
        client.try_submit(EngineCommand::FinishCapture).unwrap();
        let request = requests.try_recv().unwrap();
        replies
            .send(Event::Completed(EngineReply {
                request_id: request.id,
                result: Ok(EngineResult::Stopped),
            }))
            .unwrap();
        assert!(client.try_poll().unwrap().result.is_err());
        assert!(client.try_submit(EngineCommand::Stop).is_err());
    }
    #[test]
    fn child_failure_returns_pending_identity_once_without_retry() {
        let (mut client, requests, replies) = peer();
        client
            .try_submit(EngineCommand::StartCapture { input_device: None })
            .unwrap();
        let request = requests.try_recv().unwrap();
        replies
            .send(Event::Failed("unsealed original retained".into()))
            .unwrap();
        let reply = client.try_poll().unwrap();
        assert_eq!(reply.request_id, request.id);
        assert!(reply.result.unwrap_err().contains("retained"));
        assert!(client.try_poll().is_none());
        assert!(client.try_submit(EngineCommand::FinishCapture).is_err());
    }
    #[test]
    fn status_configuration_and_result_lifecycle_must_agree() {
        let mut status = crate::EngineStatus {
            state: "idle".into(),
            capture: None,
            sample_rate: 0,
            channels: 0,
            frames: 0,
            peak: 0.0,
            xruns: 0,
        };
        let response = |result| Response {
            reply: EngineReply {
                request_id: 1,
                result: Ok(result),
            },
        };
        assert!(
            response(EngineResult::Status(status.clone()))
                .validate()
                .is_ok()
        );
        assert!(
            response(EngineResult::Playing(status.clone()))
                .validate()
                .is_err()
        );
        status.state = "playing".into();
        assert!(
            response(EngineResult::Playing(status.clone()))
                .validate()
                .is_err()
        );
        status.sample_rate = 48000;
        status.channels = 2;
        assert!(
            response(EngineResult::Playing(status.clone()))
                .validate()
                .is_ok()
        );
        assert!(
            response(EngineResult::Capturing(status.clone()))
                .validate()
                .is_err()
        );
        status.state = "capture_fault".into();
        status.capture = Some(crate::CaptureIdentity {
            journal_path: PathBuf::from("/retained/take.sqlite3"),
            take_id: "take".into(),
        });
        assert!(
            response(EngineResult::Capturing(status.clone()))
                .validate()
                .is_ok()
        );
        assert!(response(EngineResult::Playing(status)).validate().is_err());
    }
}

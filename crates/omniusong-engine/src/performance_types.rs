//! Versioned native recorded-output control values, separate from original codecs.
use crate::{
    CaptureIdentity, CapturedTake, LiveClock, LiveFinishFence, LiveId, LiveProgress, LiveReady,
    performance_tap::TapFinish, performance_writer::WriterProgress,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceStart {
    pub id: LiveId,
    pub output_device: Option<String>,
    pub gui_clock: LiveClock,
}
impl PerformanceStart {
    pub fn is_valid(&self) -> bool {
        self.id.is_valid()
            && self.gui_clock.is_valid()
            && self
                .output_device
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 1024 && !s.contains('\0'))
    }
    pub fn directory_name(&self) -> String {
        self.id.0.iter().map(|b| format!("{b:02x}")).collect()
    }
    pub fn take_id(&self) -> String {
        format!("performance-{}", self.directory_name())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceIdentity {
    pub start_id: LiveId,
    pub capture: CaptureIdentity,
    /// None means preparation/creation may be uncertain, not that no file exists.
    pub descriptor_sha256: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePhase {
    Starting,
    Recording,
    Finishing,
    Stopping,
    Sealing,
    SealUncertain,
    Sealed,
    Interrupted,
    Failed,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRetirementStatus {
    pub pause_succeeded: bool,
    pub renderer_destroyed: bool,
    pub error_destroyed: bool,
    pub producer_destroyed: bool,
}
impl NativeRetirementStatus {
    /// Public observations only. This value is never a normal-seal capability.
    pub fn complete(&self) -> bool {
        self.pause_succeeded
            && self.renderer_destroyed
            && self.error_destroyed
            && self.producer_destroyed
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceStatus {
    pub revision: u64,
    pub identity: PerformanceIdentity,
    pub phase: PerformancePhase,
    /// Immutable initial real readiness; subsequent observations are in live.
    pub ready: Option<LiveReady>,
    /// Actual driver conversion format, separate from original logical F32 PCM.
    pub native_sample_format: Option<String>,
    pub live: Option<LiveProgress>,
    pub writer: Option<WriterProgress>,
    pub finish: Option<LiveFinishFence>,
    pub cut: Option<TapFinish>,
    pub native: NativeRetirementStatus,
    pub sealed: Option<CapturedTake>,
    pub error: Option<String>,
}
fn sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
impl PerformanceStatus {
    pub fn is_valid(&self) -> bool {
        let i = &self.identity;
        self.revision > 0
            && i.start_id.is_valid()
            && i.capture.journal_path.is_absolute()
            && i.capture.journal_path.to_string_lossy().len() <= 4096
            && !i.capture.journal_path.to_string_lossy().contains('\0')
            && i.capture.take_id
                == format!(
                    "performance-{}",
                    i.start_id
                        .0
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                )
            && i.descriptor_sha256.as_ref().is_none_or(|s| sha(s))
            && self
                .error
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 16000 && !s.contains('\0'))
            && self.ready.as_ref().is_none_or(|r| r.is_valid())
            && self
                .native_sample_format
                .as_ref()
                .is_none_or(|f| matches!(f.as_str(), "f32" | "i16" | "u16"))
            && self.live.as_ref().is_none_or(|p| {
                p.is_valid()
                    && self.ready.as_ref().is_some_and(|r| {
                        r.scope.worker_generation == p.scope.worker_generation
                            && r.scope.session_id == p.scope.session_id
                    })
            })
            && self.finish.as_ref().is_none_or(|f| {
                f.id.is_valid()
                    && f.scope.is_valid()
                    && f.accepted_sequence != Some(0)
                    && self.ready.as_ref().is_some_and(|r| {
                        r.scope.worker_generation == f.scope.worker_generation
                            && r.scope.session_id == f.scope.session_id
                    })
                    && self.live.as_ref().is_some_and(|p| {
                        p.scope == f.scope && p.worker_accepted_sequence == f.accepted_sequence
                    })
            })
            && self.cut.as_ref().is_none_or(|c| {
                self.finish == Some(c.fence)
                    && c.callback_index > 0
                    && c.cut_frame > 0
                    && c.voices_cut <= 64
                    && c.release_tails_cut <= c.voices_cut
            })
            && self.writer.as_ref().is_none_or(|w| {
                w.durable_prefixes.len() == 6
                    && w.durable_prefixes
                        .iter()
                        .zip(crate::performance_records::STREAM_IDS)
                        .all(|(p, id)| p.stream_id == id && p.gap_units == 0 && p.gap_chunks == 0)
                    && w.durable_frames <= w.complete_frames
                    && w.durable_callbacks <= w.complete_callbacks
                    && w.checkpoint_sha256.as_ref().is_none_or(|s| sha(s))
                    && w.first_error
                        .as_ref()
                        .is_none_or(|e| !e.is_empty() && e.len() <= 16000 && !e.contains('\0'))
                    && w.finish_cut.is_none_or(|c| {
                        self.finish == Some(c.fence) && self.cut.is_none_or(|ack| ack == c)
                    })
            })
            && (self.phase != PerformancePhase::Recording
                || (self.ready.is_some() && i.descriptor_sha256.is_some() && self.error.is_none()))
            && self.sealed.as_ref().is_none_or(|s| {
                self.phase == PerformancePhase::Sealed
                    && self.native.complete()
                    && self.writer.as_ref().is_some_and(|w| {
                        w.first_error.is_none()
                            && w.capture_fault.is_none()
                            && !w.capture_fault_pending
                            && w.durable_frames == s.frames
                    })
                    && self.cut.is_some_and(|c| c.cut_frame == s.frames)
                    && s.journal_path == i.capture.journal_path
                    && s.take_id == i.capture.take_id
                    && sha(&s.manifest_sha256)
                    && s.gap_frames == 0
                    && self
                        .ready
                        .as_ref()
                        .is_some_and(|r| r.sample_rate == s.sample_rate && r.channels == s.channels)
            })
            && (self.phase != PerformancePhase::Sealed || self.sealed.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceInfo, LiveScope, LiveSourceBinding, LiveSourceKind};
    fn recording() -> PerformanceStatus {
        let start = PerformanceStart {
            id: LiveId::random(),
            output_device: None,
            gui_clock: LiveClock {
                domain: "test.gui".into(),
                epoch: "test.epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
        };
        let scope = LiveScope {
            worker_generation: LiveId::random(),
            session_id: LiveId::random(),
            panic_generation: 1,
        };
        let (admission, _renderer) = crate::live::live_pair(scope, 48000, 2).unwrap();
        let progress = admission.progress();
        let ready = LiveReady {
            scope,
            gui_clock: start.gui_clock.clone(),
            sources: [
                LiveSourceKind::ComputerKeys,
                LiveSourceKind::PointerKeys,
                LiveSourceKind::Pads,
                LiveSourceKind::AccessiblePulse,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| LiveSourceBinding {
                kind,
                handle: i as u8 + 1,
                connection_generation: 1,
            })
            .collect(),
            output: DeviceInfo {
                id: "test-output".into(),
                name: "Software fixture".into(),
            },
            sample_rate: 48000,
            channels: 2,
            progress,
        };
        PerformanceStatus {
            revision: 1,
            identity: PerformanceIdentity {
                start_id: start.id,
                capture: CaptureIdentity {
                    journal_path: std::path::PathBuf::from("/fixture/take.sqlite3"),
                    take_id: start.take_id(),
                },
                descriptor_sha256: Some("a".repeat(64)),
            },
            phase: PerformancePhase::Recording,
            ready: Some(ready),
            native_sample_format: Some("f32".into()),
            live: Some(progress),
            writer: None,
            finish: None,
            cut: None,
            native: NativeRetirementStatus::default(),
            sealed: None,
            error: None,
        }
    }
    #[test]
    fn status_rejects_cross_session_finish_and_identity_substitution() {
        let status = recording();
        assert!(status.is_valid());
        let mut bad = status.clone();
        bad.identity.capture.take_id.push('x');
        assert!(!bad.is_valid());
        let mut bad = status.clone();
        bad.live.as_mut().unwrap().scope.session_id = LiveId::random();
        assert!(!bad.is_valid());
        let mut good = status.clone();
        let scope = good.live.unwrap().scope;
        good.finish = Some(LiveFinishFence {
            id: LiveId::random(),
            scope,
            accepted_sequence: None,
        });
        good.phase = PerformancePhase::Finishing;
        assert!(good.is_valid());
        let mut bad = good.clone();
        bad.finish.as_mut().unwrap().scope.worker_generation = LiveId::random();
        assert!(!bad.is_valid());
        let mut bad = good.clone();
        bad.finish.as_mut().unwrap().accepted_sequence = Some(1);
        assert!(!bad.is_valid());
        let mut bad = status;
        bad.phase = PerformancePhase::Sealed;
        assert!(!bad.is_valid());
    }
    #[test]
    fn strict_wire_and_native_observations_cannot_supply_a_manifest() {
        let status = recording();
        let mut value = serde_json::to_value(&status).unwrap();
        value["native"]["seal_authorized"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<PerformanceStatus>(value).is_err());
        let mut bad = status.clone();
        bad.native_sample_format = Some("python".into());
        assert!(!bad.is_valid());
        let mut bad = status;
        bad.native = NativeRetirementStatus {
            pause_succeeded: true,
            renderer_destroyed: true,
            error_destroyed: true,
            producer_destroyed: true,
        };
        bad.phase = PerformancePhase::Sealed;
        assert!(!bad.is_valid());
    }
}

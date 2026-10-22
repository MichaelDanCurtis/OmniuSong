//! Bounded live-audition interfaces. Queue admission is not rendered delivery.
use serde::{Deserialize, Serialize};

pub const LIVE_PROTOCOL: &str = "omniusong.engine-live.v2";
pub const ENGINE_BUILD_ID: &str =
    concat!("omniusong-engine-", env!("CARGO_PKG_VERSION"), "-live-v2");
pub const LIVE_QUEUE_BATCHES: usize = 16;
pub const LIVE_CALLBACK_PACKET_BUDGET: usize = 4;
pub const LIVE_WIRE_BYTES: usize = 16 * 1024;

pub use omniusong_performance::live_types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveQueued {
    pub scope: LiveScope,
    pub sequence: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseCredit {
    pub scope: LiveScope,
    pub token: LiveId,
    /// None is the initial credit; Some(n) was issued by accepted batch n.
    pub issued_after_sequence: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveState {
    Running,
    Panicking,
    Finishing,
    FinishedCut,
    Faulted,
    Stopped,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveProgress {
    pub scope: LiveScope,
    pub state: LiveState,
    pub worker_accepted_sequence: Option<u64>,
    pub callback_applied_sequence: Option<u64>,
    pub active_voices: u16,
    pub release_tail_voices: u16,
    pub late_releases: u64,
    pub rejected_batches: u64,
    pub overflow_batches: u64,
    pub frames: u64,
    pub peak: f32,
    pub lease_deadline_frame: u64,
    pub lease_credit: Option<LeaseCredit>,
    pub fault: Option<LiveFault>,
}
impl LiveProgress {
    pub fn next_sequence(&self) -> Option<u64> {
        self.worker_accepted_sequence
            .map_or(Some(1), |sequence| sequence.checked_add(1))
    }
    pub fn is_valid(&self) -> bool {
        self.scope.is_valid()
            && self.active_voices as usize <= LIVE_VOICES
            && self.release_tail_voices <= self.active_voices
            && self.peak.is_finite()
            && self.peak >= 0.0
            && self.worker_accepted_sequence != Some(0)
            && self.callback_applied_sequence != Some(0)
            && self.callback_applied_sequence <= self.worker_accepted_sequence
            && self.lease_credit.is_none_or(|credit| {
                credit.scope == self.scope
                    && credit.token.is_valid()
                    && credit.issued_after_sequence != Some(0)
                    && credit.issued_after_sequence <= self.worker_accepted_sequence
            })
            && self.fault.is_none_or(|fault| fault.scope == self.scope)
            && match self.state {
                LiveState::Running => {
                    self.fault.is_none()
                        && self.lease_credit.is_some()
                        && self.lease_deadline_frame > 0
                }
                LiveState::Panicking | LiveState::Finishing => {
                    self.fault.is_none()
                        && self.lease_credit.is_none()
                        && self.lease_deadline_frame > 0
                }
                LiveState::FinishedCut => {
                    self.fault.is_none()
                        && self.lease_credit.is_none()
                        && self.lease_deadline_frame > 0
                        && self.frames > 0
                        && self.frames <= i64::MAX as u64
                        && self.active_voices == 0
                        && self.release_tail_voices == 0
                        && self.callback_applied_sequence == self.worker_accepted_sequence
                }
                LiveState::Faulted => self.fault.is_some() && self.lease_credit.is_none(),
                LiveState::Stopped => self.active_voices == 0 && self.lease_credit.is_none(),
            }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveReady {
    pub scope: LiveScope,
    pub gui_clock: LiveClock,
    pub sources: Vec<LiveSourceBinding>,
    pub output: crate::DeviceInfo,
    pub sample_rate: u32,
    pub channels: u16,
    pub progress: LiveProgress,
}
impl LiveReady {
    pub fn is_valid(&self) -> bool {
        let kinds = [
            LiveSourceKind::ComputerKeys,
            LiveSourceKind::PointerKeys,
            LiveSourceKind::Pads,
            LiveSourceKind::AccessiblePulse,
        ];
        self.scope.is_valid()
            && self.gui_clock.is_valid()
            && (8_000..=768_000).contains(&self.sample_rate)
            && (1..=8).contains(&self.channels)
            && bounded_text(&self.output.id, 1024)
            && bounded_text(&self.output.name, 1024)
            && self.progress.scope == self.scope
            && self.progress.is_valid()
            && self.progress.state == LiveState::Running
            && self.progress.lease_credit.is_some()
            && self.sources.len() == kinds.len()
            && kinds
                .iter()
                .all(|kind| self.sources.iter().filter(|s| s.kind == *kind).count() == 1)
            && self.sources.iter().all(|s| {
                (1..=LIVE_MAX_SOURCES as u8).contains(&s.handle)
                    && s.connection_generation > 0
                    && self
                        .sources
                        .iter()
                        .filter(|other| other.handle == s.handle)
                        .count()
                        == 1
            })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrgentTicket {
    pub worker_generation: LiveId,
    pub id: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopAck {
    pub ticket: UrgentTicket,
    pub cancelled_through_control: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum PanicOutcome {
    Applied { new_scope: LiveScope },
    SupersededByStop { stop_ticket: UrgentTicket },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanicAck {
    pub ticket: UrgentTicket,
    pub requested_scope: LiveScope,
    pub outcome: PanicOutcome,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrgentProgress {
    pub stop: Option<StopAck>,
    pub panic: Option<PanicAck>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineUpdate {
    pub worker_generation: LiveId,
    pub revision: u64,
    pub live: Option<LiveProgress>,
    pub urgent: UrgentProgress,
    /// Retained across Stop until an explicit successful new live session.
    pub first_fault: Option<LiveFault>,
}
impl EngineUpdate {
    pub fn is_valid(&self) -> bool {
        let ticket_valid = |ticket: UrgentTicket| {
            ticket.id > 0 && ticket.worker_generation == self.worker_generation
        };
        self.worker_generation.is_valid()
            && self.revision > 0
            && self.live.is_none_or(|live| {
                live.scope.worker_generation == self.worker_generation && live.is_valid()
            })
            && self.first_fault.is_none_or(|fault| {
                fault.scope.is_valid() && fault.scope.worker_generation == self.worker_generation
            })
            && self.urgent.stop.is_none_or(|ack| ticket_valid(ack.ticket))
            && self.urgent.panic.is_none_or(|ack| {
                ticket_valid(ack.ticket)
                    && ack.requested_scope.is_valid()
                    && ack.requested_scope.worker_generation == self.worker_generation
                    && match ack.outcome {
                        PanicOutcome::Applied { new_scope } => {
                            new_scope.is_valid()
                                && new_scope.worker_generation == self.worker_generation
                                && new_scope.session_id == ack.requested_scope.session_id
                                && ack.requested_scope.panic_generation.checked_add(1)
                                    == Some(new_scope.panic_generation)
                        }
                        PanicOutcome::SupersededByStop { stop_ticket } => {
                            ticket_valid(stop_ticket)
                                && stop_ticket.id >= ack.ticket.id
                                && self
                                    .urgent
                                    .stop
                                    .is_some_and(|stop| stop.ticket.id >= stop_ticket.id)
                        }
                    }
            })
    }
}
fn bounded_text(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> LiveScope {
        LiveScope {
            worker_generation: LiveId([1; 16]),
            session_id: LiveId([2; 16]),
            panic_generation: 1,
        }
    }
    #[test]
    fn fixed_packet_preserves_velocity_zero_and_checks_unused_slots_and_pulse_bounds() {
        let mut batch = LiveBatch::empty(scope(), 1, None);
        let mut event = LiveEvent {
            source: 1,
            connection_generation: 1,
            observed_ticks: 7,
            event: LiveEventKind::NoteOn {
                note: 1,
                voice: 1,
                key: 60,
                velocity16: 0,
                patch: LivePatch::Keys,
            },
        };
        batch.try_push(event).unwrap();
        assert!(batch.validate_shape(48_000).is_ok());
        let encoded = serde_json::to_vec(&batch).unwrap();
        let decoded: LiveBatch = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(batch, decoded);
        batch.events[31] = Some(event);
        assert_eq!(batch.validate_shape(48_000), Err(LiveSubmitError::Invalid));
        batch.events[31] = None;
        event.event = LiveEventKind::Pulse {
            note: 2,
            voice: 2,
            key: 60,
            velocity16: u16::MAX,
            release_velocity16: u16::MAX,
            patch: LivePatch::Keys,
            duration_frames: 24_001,
        };
        batch.events[0] = Some(event);
        assert_eq!(batch.validate_shape(48_000), Err(LiveSubmitError::Invalid));
    }
}

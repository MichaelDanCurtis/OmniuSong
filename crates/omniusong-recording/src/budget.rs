//! One cumulative, sticky governor for all passes over an untrusted original.
use crate::{RecordingError, Result};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionLimits {
    pub max_file_bytes: u64,
    pub max_physical_rows: u64,
    pub max_work_bytes: u64,
    pub max_row_visits: u64,
    pub max_chunk_visits: u64,
    pub max_record_visits: u64,
    pub max_groups: u64,
    pub max_vm_steps: u64,
}
impl Default for InspectionLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 536_870_912,
            max_physical_rows: 262_144,
            max_work_bytes: 1_073_741_824,
            max_row_visits: 4_194_304,
            max_chunk_visits: 524_288,
            max_record_visits: 2_097_152,
            max_groups: 65_536,
            max_vm_steps: 32_000_000,
        }
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionUsage {
    pub file_bytes: u64,
    pub physical_rows: u64,
    pub work_bytes: u64,
    pub row_visits: u64,
    pub chunk_visits: u64,
    pub record_visits: u64,
    pub groups: u64,
    pub vm_steps: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InspectionResource {
    FileBytes,
    PhysicalRows,
    WorkBytes,
    RowVisits,
    ChunkVisits,
    RecordVisits,
    Groups,
    VmSteps,
}
#[derive(Debug, Clone, Copy)]
enum Stop {
    Cancelled,
    Limit {
        resource: InspectionResource,
        limit: u64,
        attempted: u64,
    },
}
impl Stop {
    fn error(self) -> RecordingError {
        match self {
            Self::Cancelled => RecordingError::InspectionCancelled,
            Self::Limit {
                resource,
                limit,
                attempted,
            } => RecordingError::InspectionLimit {
                resource,
                limit,
                attempted,
            },
        }
    }
}
#[derive(Debug, Default)]
struct State {
    usage: InspectionUsage,
    stop: Option<Stop>,
}
#[derive(Debug)]
struct Shared {
    limits: InspectionLimits,
    cancelled: Arc<AtomicBool>,
    state: Mutex<State>,
}
/// Deliberately not Clone. Only recording readers and their SQL hook share it.
#[derive(Debug)]
pub struct InspectionBudget {
    shared: Arc<Shared>,
}
impl InspectionBudget {
    pub fn new(limits: InspectionLimits, cancelled: Arc<AtomicBool>) -> Result<Self> {
        let ceiling = InspectionLimits::default();
        crate::types::require(
            limits.max_file_bytes <= ceiling.max_file_bytes
                && limits.max_physical_rows <= ceiling.max_physical_rows
                && limits.max_work_bytes <= ceiling.max_work_bytes
                && limits.max_row_visits <= ceiling.max_row_visits
                && limits.max_chunk_visits <= ceiling.max_chunk_visits
                && limits.max_record_visits <= ceiling.max_record_visits
                && limits.max_groups <= ceiling.max_groups
                && limits.max_vm_steps <= ceiling.max_vm_steps,
            "Inspection limits exceed compiled ceilings",
        )?;
        Ok(Self {
            shared: Arc::new(Shared {
                limits,
                cancelled,
                state: Mutex::new(State::default()),
            }),
        })
    }
    pub(crate) fn share(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
    pub fn checkpoint(&self) -> Result<()> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stop.is_none() && self.shared.cancelled.load(Ordering::Acquire) {
            state.stop = Some(Stop::Cancelled);
        }
        state.stop.map_or(Ok(()), |stop| Err(stop.error()))
    }
    pub fn usage(&self) -> InspectionUsage {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .usage
    }
    fn charge(&self, resource: InspectionResource, count: u64) -> Result<()> {
        self.checkpoint()?;
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(stop) = state.stop {
            return Err(stop.error());
        }
        let limits = self.shared.limits;
        let (value, limit) = match resource {
            InspectionResource::FileBytes => (&mut state.usage.file_bytes, limits.max_file_bytes),
            InspectionResource::PhysicalRows => {
                (&mut state.usage.physical_rows, limits.max_physical_rows)
            }
            InspectionResource::WorkBytes => (&mut state.usage.work_bytes, limits.max_work_bytes),
            InspectionResource::RowVisits => (&mut state.usage.row_visits, limits.max_row_visits),
            InspectionResource::ChunkVisits => {
                (&mut state.usage.chunk_visits, limits.max_chunk_visits)
            }
            InspectionResource::RecordVisits => {
                (&mut state.usage.record_visits, limits.max_record_visits)
            }
            InspectionResource::Groups => (&mut state.usage.groups, limits.max_groups),
            InspectionResource::VmSteps => (&mut state.usage.vm_steps, limits.max_vm_steps),
        };
        let attempted = if resource == InspectionResource::FileBytes {
            Some((*value).max(count))
        } else {
            value.checked_add(count)
        };
        if attempted.is_none_or(|n| n > limit) {
            let stop = Stop::Limit {
                resource,
                limit,
                attempted: attempted.unwrap_or(u64::MAX),
            };
            state.stop = Some(stop);
            return Err(stop.error());
        }
        *value = attempted.expect("checked");
        Ok(())
    }
    pub fn charge_work(&self, bytes: u64) -> Result<()> {
        self.charge(InspectionResource::WorkBytes, bytes)
    }
    pub fn visit_rows(&self, count: u64) -> Result<()> {
        self.charge(InspectionResource::RowVisits, count)
    }
    pub fn visit_chunks(&self, count: u64) -> Result<()> {
        self.charge(InspectionResource::ChunkVisits, count)
    }
    pub fn visit_records(&self, count: u64) -> Result<()> {
        self.charge(InspectionResource::RecordVisits, count)
    }
    pub fn visit_groups(&self, count: u64) -> Result<()> {
        self.charge(InspectionResource::Groups, count)
    }
    pub(crate) fn file(&self, bytes: u64) -> Result<()> {
        self.charge(InspectionResource::FileBytes, bytes)
    }
    pub(crate) fn physical_row(&self) -> Result<()> {
        self.charge(InspectionResource::PhysicalRows, 1)?;
        self.visit_rows(1)
    }
    pub(crate) fn finish<T>(&self, result: Result<T>) -> Result<T> {
        self.checkpoint()?;
        result
    }
    /// A one-instruction hook accounts cumulatively even for repeated short
    /// statements (SQLite resets larger progress quanta per VDBE). This incurs
    /// off-realtime per-instruction overhead; no latency bound is promised.
    pub(crate) fn install(&self, connection: &rusqlite::Connection) -> Result<()> {
        self.charge(InspectionResource::VmSteps, 1)?;
        let budget = self.share();
        connection.progress_handler(
            1,
            Some(move || budget.charge(InspectionResource::VmSteps, 1).is_err()),
        )?;
        Ok(())
    }
}
pub(crate) fn checkpoint(budget: Option<&InspectionBudget>) -> Result<()> {
    budget.map_or(Ok(()), InspectionBudget::checkpoint)
}
pub(crate) fn work(budget: Option<&InspectionBudget>, bytes: u64) -> Result<()> {
    budget.map_or(Ok(()), |b| b.charge_work(bytes))
}
pub(crate) fn rows(budget: Option<&InspectionBudget>, count: u64) -> Result<()> {
    budget.map_or(Ok(()), |b| b.visit_rows(count))
}
pub(crate) fn finish<T>(budget: Option<&InspectionBudget>, result: Result<T>) -> Result<T> {
    match budget {
        Some(b) => b.finish(result),
        None => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn budget(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        (
            InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
            cancelled,
        )
    }
    #[test]
    fn strict_limits_and_usage_reject_unknown_fields_and_above_ceiling() {
        let mut limits = InspectionLimits::default();
        limits.max_groups += 1;
        assert!(InspectionBudget::new(limits, Arc::new(AtomicBool::new(false))).is_err());
        let mut json = serde_json::to_value(InspectionLimits::default()).unwrap();
        json["extra"] = true.into();
        assert!(serde_json::from_value::<InspectionLimits>(json).is_err());
        assert!(serde_json::from_str::<InspectionUsage>("{\"unexpected\":1}").is_err());
    }
    #[test]
    fn cumulative_private_share_cannot_reset_any_public_counter() {
        let (budget, _) = budget(InspectionLimits {
            max_work_bytes: 3,
            max_row_visits: 3,
            max_chunk_visits: 3,
            max_record_visits: 3,
            max_groups: 3,
            ..InspectionLimits::default()
        });
        let retained = budget.share();
        budget.charge_work(1).unwrap();
        retained.charge_work(2).unwrap();
        budget.visit_rows(1).unwrap();
        retained.visit_rows(2).unwrap();
        budget.visit_chunks(1).unwrap();
        retained.visit_chunks(2).unwrap();
        budget.visit_records(1).unwrap();
        retained.visit_records(2).unwrap();
        budget.visit_groups(1).unwrap();
        retained.visit_groups(2).unwrap();
        assert_eq!(budget.usage(), retained.usage());
        assert!(matches!(
            retained.visit_groups(1),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::Groups,
                limit: 3,
                attempted: 4
            })
        ));
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::Groups,
                ..
            })
        ));
    }
    #[test]
    fn first_refusal_is_sticky_and_checked_arithmetic_never_wraps() {
        let (budget, cancelled) = budget(InspectionLimits::default());
        assert!(matches!(
            budget.charge_work(u64::MAX),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                attempted: u64::MAX,
                ..
            })
        ));
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                ..
            })
        ));
        let (budget, cancelled) = self::budget(InspectionLimits::default());
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionCancelled)
        ));
        cancelled.store(false, Ordering::Release);
        assert!(matches!(
            budget.charge_work(u64::MAX),
            Err(RecordingError::InspectionCancelled)
        ));
    }
    #[test]
    fn separate_short_sql_statements_exhaust_one_cumulative_vm_allowance() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let (budget, _) = budget(InspectionLimits {
            max_vm_steps: 80,
            ..InspectionLimits::default()
        });
        budget.install(&connection).unwrap();
        let mut accepted = 0;
        loop {
            let result = connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .map_err(RecordingError::from);
            match budget.finish(result) {
                Ok(1) => {
                    accepted += 1;
                    assert!(accepted < 80);
                }
                Err(RecordingError::InspectionLimit {
                    resource: InspectionResource::VmSteps,
                    ..
                }) => break,
                other => panic!("Unexpected VM result: {other:?}"),
            }
        }
        assert!(accepted > 1);
        assert_eq!(budget.usage().vm_steps, 80);
    }
    #[test]
    fn sql_interrupt_cannot_replace_typed_cancellation() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let (budget, cancelled) = budget(InspectionLimits::default());
        budget.install(&connection).unwrap();
        cancelled.store(true, Ordering::Release);
        let result = connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .map_err(RecordingError::from);
        assert!(matches!(
            budget.finish(result),
            Err(RecordingError::InspectionCancelled)
        ));
    }
}

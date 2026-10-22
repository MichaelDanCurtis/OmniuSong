//! Cheap bounded dispatch, not source authority. Typed source-aware decoders
//! prepay full metadata work only after this non-materializing ownership probe.
use crate::{Result, StoreError};
use omniusong_core::native::MAX_NATIVE_PROJECT_BYTES;
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde::de::IgnoredAny;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Probe {
    schema_version: u64,
    performance_projections: Option<RegistryProbe>,
}
#[derive(Deserialize)]
struct RegistryProbe {
    entries: Vec<IgnoredAny>,
}
fn owns(raw: &str) -> bool {
    // IgnoredAny is zero-sized. Array entries/unknown document fields are
    // skipped, never materialized as arbitrary Value trees. An incompatible
    // legacy opaque field falls back to the original guarded decoder.
    serde_json::from_str::<Probe>(raw).is_ok_and(|p| {
        p.schema_version == 3
            && p.performance_projections
                .is_some_and(|r| !r.entries.is_empty())
    })
}
fn invalid(s: &str) -> StoreError {
    StoreError::Invalid(s.into())
}
fn credit(budget: &InspectionBudget, length: i64, multiplier: u64) -> Result<()> {
    if !(0..=MAX_NATIVE_PROJECT_BYTES as i64).contains(&length) {
        return Err(invalid("Stored project/history exceeds document bounds"));
    }
    budget.charge_work((length as u64).saturating_mul(multiplier) + 4096)?;
    Ok(())
}
/// Caller holds a catalog transaction across both queries.
pub(crate) fn project(
    connection: &Connection,
    id: &str,
    budget: &InspectionBudget,
) -> Result<bool> {
    budget.checkpoint()?;
    let length: i64 = connection.query_row(
        "SELECT octet_length(payload) FROM projects WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    credit(budget, length, 4)?;
    let raw: Option<String> = connection.query_row(
        "SELECT CASE WHEN octet_length(payload)<=?2 THEN payload END FROM projects WHERE id=?1",
        params![id, MAX_NATIVE_PROJECT_BYTES as i64],
        |r| r.get(0),
    )?;
    Ok(owns(raw.as_deref().ok_or_else(|| {
        invalid("Stored project exceeds document bounds")
    })?))
}
pub(crate) fn history(
    connection: &Connection,
    id: &str,
    redo: bool,
    budget: &InspectionBudget,
) -> Result<bool> {
    budget.checkpoint()?;
    let cursor: i64 =
        connection.query_row("SELECT cursor FROM projects WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    let target = if redo {
        cursor.checked_add(1)
    } else {
        Some(cursor)
    }
    .ok_or_else(|| invalid("Stored edit cursor exhausted"))?;
    let lengths:Option<(i64,i64)>=connection.query_row("SELECT octet_length(before_json),octet_length(after_json) FROM edits WHERE project_id=?1 AND sequence=?2",params![id,target],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((before, after)) = lengths else {
        return Ok(false);
    };
    credit(budget, before, 4)?;
    credit(budget, after, 4)?;
    let (before,after):(Option<String>,Option<String>)=connection.query_row("SELECT CASE WHEN octet_length(before_json)<=?3 THEN before_json END,CASE WHEN octet_length(after_json)<=?3 THEN after_json END FROM edits WHERE project_id=?1 AND sequence=?2",params![id,target,MAX_NATIVE_PROJECT_BYTES as i64],|r|Ok((r.get(0)?,r.get(1)?)))?;
    Ok(owns(
        before
            .as_deref()
            .ok_or_else(|| invalid("Stored history exceeds document bounds"))?,
    ) || owns(
        after
            .as_deref()
            .ok_or_else(|| invalid("Stored history exceeds document bounds"))?,
    ))
}
pub(crate) fn prepay_history(
    connection: &Connection,
    id: &str,
    target: i64,
    budget: &InspectionBudget,
) -> Result<()> {
    let (before,after):(i64,i64)=connection.query_row("SELECT octet_length(before_json),octet_length(after_json) FROM edits WHERE project_id=?1 AND sequence=?2",params![id,target],|r|Ok((r.get(0)?,r.get(1)?)))?;
    credit(budget, before, 64)?;
    credit(budget, after, 64)
}
pub(crate) fn prepay_operation(
    connection: &Connection,
    id: &str,
    budget: &InspectionBudget,
) -> Result<()> {
    let row:Option<(i64,i64)>=connection.query_row("SELECT octet_length(request_json),octet_length(result_json) FROM operations WHERE operation_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((request, result)) = row {
        if !(0..=omniusong_core::contracts::WORKER_MAX_BYTES as i64).contains(&request) {
            return Err(invalid("Stored edit request exceeds wire bounds"));
        }
        budget.charge_work((request as u64).saturating_mul(64) + 4096)?;
        credit(budget, result, 64)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thin_probe_distinguishes_owned_registry_from_old_opaque_fields() {
        assert!(owns(
            r#"{"schemaVersion":3,"performanceProjections":{"entries":[{"hugeUnknown":[1,2,3]}]},"foreign":{"ignored":[[[]]]}}"#
        ));
        for raw in [
            r#"{"schemaVersion":3,"performanceProjections":{"entries":[]}}"#,
            r#"{"schemaVersion":2,"performanceProjections":{"entries":[{}]}}"#,
            r#"{"schemaVersion":1,"performanceProjections":"opaque"}"#,
            r#"{"schemaVersion":3,"performanceProjections":null}"#,
        ] {
            assert!(!owns(raw));
        }
    }
    #[test]
    fn dispatch_credit_refuses_before_full_metadata_decode() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE projects(id TEXT,payload TEXT);")
            .unwrap();
        db.execute(
            "INSERT INTO projects VALUES('p',?1)",
            ["not valid JSON".repeat(1024)],
        )
        .unwrap();
        let budget = InspectionBudget::new(
            omniusong_recording::InspectionLimits {
                max_work_bytes: 4096,
                ..Default::default()
            },
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        assert!(matches!(
            project(&db, "p", &budget),
            Err(StoreError::Recording(
                omniusong_recording::RecordingError::InspectionLimit { .. }
            ))
        ));
    }
}

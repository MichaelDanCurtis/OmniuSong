//! Bounded catalog metadata, never a source-authentication capability.
//!
//! The caller owns one active catalog transaction and its progress hook. These
//! helpers neither open source registrations/files nor acquire a new governor.
//! A healthy summary or returned document means structural metadata only;
//! source-aware loading and every mutation must perform their own admission.
use crate::{ProjectSummary, Result, StoreError};
use omniusong_core::{DocumentError, ProjectDocument, native::MAX_NATIVE_PROJECT_BYTES};
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, Row, params};

pub(crate) const MAX_PROJECT_SUMMARIES: usize = 512;
const MAX_ID_BYTES: usize = 96;
const BOUNDS: &str = "Stored project exceeds its document size limit";
const IDENTITY: &str = "Stored project identity/revision is inconsistent";
const PAYLOAD_TYPE: &str = "Stored project payload is not text";

struct Header {
    rowid: i64,
    id: Option<String>,
    revision: i64,
    length: Option<i64>,
    text: bool,
}
impl Header {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            rowid: row.get(0)?,
            id: row.get(1)?,
            revision: row.get(2)?,
            length: row.get(3)?,
            text: row.get(4)?,
        })
    }
    fn identity(&self) -> Result<&str> {
        self.id.as_deref().ok_or_else(|| {
            StoreError::Invalid(
                "Stored project identity is missing, invalid or exceeds its byte limit".into(),
            )
        })
    }
}

enum Damage {
    Bounds,
    Identity,
    PayloadType,
    Document(DocumentError),
}
impl Damage {
    fn message(self) -> String {
        match self {
            Self::Bounds => BOUNDS.into(),
            Self::Identity => IDENTITY.into(),
            Self::PayloadType => PAYLOAD_TYPE.into(),
            Self::Document(error) => error.0,
        }
    }
    fn into_error(self) -> StoreError {
        match self {
            Self::Document(error) => StoreError::Document(error),
            other => StoreError::Invalid(other.message()),
        }
    }
}
type Decoded = std::result::Result<ProjectDocument, Damage>;

fn active(connection: &Connection, budget: &InspectionBudget) -> Result<()> {
    budget.checkpoint()?;
    if connection.is_autocommit() {
        return Err(StoreError::Invalid(
            "Project metadata view requires an active caller-owned catalog transaction".into(),
        ));
    }
    Ok(())
}
fn step(budget: &InspectionBudget) -> Result<()> {
    // Before preparing/stepping SQL: bounded identity cells, bookkeeping and
    // the fixed structural-summary pass. EOF is charged too, never refunded.
    budget.visit_rows(1)?;
    budget.charge_work(4096)?;
    Ok(())
}
fn finish<T>(budget: &InspectionBudget, result: Result<T>) -> Result<T> {
    match result {
        // A later cancellation cannot replace a live catalog SQL failure.
        Err(error) => Err(error),
        Ok(value) => {
            budget.checkpoint()?;
            Ok(value)
        }
    }
}
fn decode(connection: &Connection, header: &Header, budget: &InspectionBudget) -> Result<Decoded> {
    let id = header.identity()?;
    let Some(length) = header
        .length
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value <= MAX_NATIVE_PROJECT_BYTES)
    else {
        return Ok(Err(Damage::Bounds));
    };
    if !header.text {
        return Ok(Err(Damage::PayloadType));
    }
    // Covers raw materialization, full Value decoding/structural validation,
    // the core decoder's bounded serialization and temporary tree copies.
    budget.charge_work((length as u64).saturating_mul(64).saturating_add(4096))?;
    step(budget)?;
    // The exact prepaid byte length is also a SQL predicate. Even accidental
    // same-connection changes cannot materialize a larger unpaid document.
    let (raw, revision): (Option<String>, i64) = connection.query_row(
        "SELECT CASE WHEN typeof(payload)='text' AND octet_length(payload)=?3 AND octet_length(payload)<=?4 THEN payload END,revision FROM projects WHERE rowid=?1 AND id=?2",
        params![header.rowid, id, length as i64, MAX_NATIVE_PROJECT_BYTES as i64],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let raw = raw.ok_or_else(|| {
        StoreError::Invalid("Stored project metadata changed during its held snapshot".into())
    })?;
    if revision != header.revision || raw.len() != length {
        return Err(StoreError::Invalid(
            "Stored project metadata changed during its held snapshot".into(),
        ));
    }
    budget.checkpoint()?;
    let document = match ProjectDocument::from_json(raw.as_bytes()) {
        Ok(document) => document,
        Err(error) => return Ok(Err(Damage::Document(error))),
    };
    if document.id() != id || Some(document.revision()) != u64::try_from(header.revision).ok() {
        return Ok(Err(Damage::Identity));
    }
    Ok(Ok(document))
}

fn summary(header: Header, decoded: Decoded, budget: &InspectionBudget) -> Result<ProjectSummary> {
    let id = header.id.expect("bounded identity checked before decoding");
    let vector_credit = (2 * std::mem::size_of::<ProjectSummary>()) as u64 + 64;
    match decoded {
        Ok(project) => {
            // This flag describes retained metadata claims. Neither value is a
            // source proof, and old opaque lookalikes never acquire this role.
            let source_authentication_required = project.schema_version() == 3
                && project.value()["performanceProjections"]["entries"]
                    .as_array()
                    .is_some_and(|entries| !entries.is_empty());
            budget.charge_work(vector_credit + id.len() as u64 + project.name().len() as u64)?;
            Ok(ProjectSummary {
                id,
                name: project.name().into(),
                revision: project.revision(),
                track_count: project.tracks().len(),
                note_count: project
                    .tracks()
                    .iter()
                    .filter(|track| track["kind"] != "recording")
                    .map(|track| track["notes"].as_array().map_or(0, Vec::len))
                    .sum(),
                problem: None,
                source_authentication_required,
            })
        }
        Err(damage) => {
            let problem = damage.message();
            let name = "Damaged session · recovery needed";
            budget.charge_work(
                vector_credit + id.len() as u64 + name.len() as u64 + problem.len() as u64,
            )?;
            Ok(ProjectSummary {
                id,
                name: name.into(),
                revision: u64::try_from(header.revision).unwrap_or(0),
                track_count: 0,
                note_count: 0,
                problem: Some(problem),
                source_authentication_required: false,
            })
        }
    }
}

/// Metadata-only library in the existing descending insertion order. A caller
/// must add paging before exposing more than this finite collection bound.
pub(crate) fn list(
    connection: &Connection,
    budget: &InspectionBudget,
) -> Result<Vec<ProjectSummary>> {
    finish(
        budget,
        (|| {
            active(connection, budget)?;
            step(budget)?;
            let mut statement = connection.prepare(
            "SELECT rowid,CASE WHEN typeof(id)='text' AND octet_length(id) BETWEEN 1 AND ?1 THEN id END,revision,octet_length(payload),typeof(payload)='text' FROM projects ORDER BY rowid DESC",
        )?;
            let mut rows = statement.query([MAX_ID_BYTES as i64])?;
            let mut output = Vec::new();
            loop {
                step(budget)?;
                let Some(row) = rows.next()? else {
                    break;
                };
                if output.len() >= MAX_PROJECT_SUMMARIES {
                    return Err(StoreError::Invalid(
                        "Project metadata library exceeds 512 summaries; paging is required".into(),
                    ));
                }
                let header = Header::read(row)?;
                header.identity()?;
                let decoded = decode(connection, &header, budget)?;
                let value = summary(header, decoded, budget)?;
                // Output and capacity growth were credited by summary(), before
                // any name copy or collection allocation. Never silently truncate.
                output.try_reserve(1).map_err(|_| {
                    StoreError::Invalid("Cannot reserve bounded project metadata summaries".into())
                })?;
                output.push(value);
            }
            Ok(output)
        })(),
    )
}

/// Structural identity/revision admission only, without source registrations,
/// source files, history/replay validation or publication authority.
pub(crate) fn load(
    connection: &Connection,
    id: &str,
    budget: &InspectionBudget,
) -> Result<ProjectDocument> {
    finish(
        budget,
        (|| {
            active(connection, budget)?;
            if id.is_empty() || id.len() > MAX_ID_BYTES {
                return Err(StoreError::Invalid(
                    "Invalid project metadata identity".into(),
                ));
            }
            step(budget)?;
            let header = connection.query_row(
            "SELECT rowid,CASE WHEN typeof(id)='text' AND octet_length(id) BETWEEN 1 AND ?2 THEN id END,revision,octet_length(payload),typeof(payload)='text' FROM projects WHERE id=?1",
            params![id, MAX_ID_BYTES as i64], Header::read,
        )?;
            decode(connection, &header, budget)?.map_err(Damage::into_error)
        })(),
    )
}

#[cfg(test)]
mod tests;

//! Bounded opaque source-choice history. Stored choices and review bytes grant
//! no authority to recover, copy, attach, release, or change an original.
use crate::{NativeStore, Result, StoreError, checksum};
use rusqlite::{Connection, Row, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PERFORMANCE_SOURCE_ACTIONS: usize = 32;
pub const MAX_PERFORMANCE_SOURCE_ACTION_REVISIONS: u64 = 16;
pub const MAX_PERFORMANCE_SOURCE_ACTION_BYTES: usize = 16_384;
pub const MAX_PERFORMANCE_SOURCE_REVIEW_BYTES: usize = 65_536;
const MAX_HISTORY_ROWS: usize =
    MAX_PERFORMANCE_SOURCE_ACTIONS * MAX_PERFORMANCE_SOURCE_ACTION_REVISIONS as usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPerformanceSourceReview {
    pub id: String,
    pub payload: Vec<u8>,
    pub payload_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPerformanceSourceAction {
    pub id: String,
    pub revision: u64,
    pub review_blob_sha256: String,
    pub payload: Vec<u8>,
    pub payload_sha256: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerformanceSourceActionHistory {
    pub reviews: Vec<StoredPerformanceSourceReview>,
    pub revisions: Vec<StoredPerformanceSourceAction>,
}

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::Invalid(message.into())
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && id.bytes().any(|c| c != b'0')
}

fn validate_object(payload: &[u8], limit: usize, kind: &str) -> Result<()> {
    if payload.is_empty() || payload.len() > limit {
        return Err(invalid(format!(
            "Performance source {kind} must contain 1..={limit} bytes"
        )));
    }
    let value: serde_json::Value = serde_json::from_slice(payload).map_err(|_| {
        invalid(format!(
            "Performance source {kind} must be a valid UTF-8 JSON object"
        ))
    })?;
    if !value.is_object() {
        return Err(invalid(format!(
            "Performance source {kind} must be a JSON object"
        )));
    }
    Ok(())
}

// All callers select CASE-guarded values. NULL or a decoding failure denotes a
// damaged stored row, rather than permission to convert/coerce its SQL type.
fn bounded_text(row: &Row<'_>, index: usize) -> Result<String> {
    row.get::<_, Option<String>>(index)
        .map_err(|_| invalid("Stored performance source text is not valid bounded UTF-8"))?
        .ok_or_else(|| invalid("Stored performance source text exceeds its byte or type bounds"))
}

fn bounded_blob(row: &Row<'_>, index: usize) -> Result<Vec<u8>> {
    row.get::<_, Option<Vec<u8>>>(index)
        .map_err(|_| invalid("Stored performance source payload is not a bounded BLOB"))?
        .ok_or_else(|| invalid("Stored performance source payload exceeds its byte or type bounds"))
}

/// One consistent transaction must surround this complete bounded read. SQL
/// guards prevent oversized or incorrectly typed fields from reaching Rust.
fn load_history(connection: &Connection) -> Result<PerformanceSourceActionHistory> {
    let mut history = PerformanceSourceActionHistory::default();
    let mut review_indexes = BTreeMap::new();
    {
        let mut statement = connection.prepare(
            "SELECT CASE WHEN typeof(id)='text' AND octet_length(id)=32 THEN id END,
                    CASE WHEN typeof(payload)='blob' AND octet_length(payload) BETWEEN 1 AND 65536 THEN payload END,
                    CASE WHEN typeof(payload_sha256)='text' AND octet_length(payload_sha256)=64 THEN payload_sha256 END
             FROM performance_source_reviews ORDER BY id COLLATE BINARY LIMIT 33",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            if history.reviews.len() >= MAX_PERFORMANCE_SOURCE_ACTIONS {
                return Err(invalid(
                    "Stored performance source reviews exceed the retained limit of 32 identities",
                ));
            }
            let id = bounded_text(row, 0)?;
            let payload = bounded_blob(row, 1)?;
            let payload_sha256 = bounded_text(row, 2)?;
            if !valid_id(&id) {
                return Err(invalid(
                    "Stored performance source review identity is invalid",
                ));
            }
            validate_object(&payload, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES, "review")?;
            if checksum(&payload) != payload_sha256 {
                return Err(invalid(
                    "Stored performance source review hash differs from its exact bytes",
                ));
            }
            if review_indexes
                .insert(id.clone(), history.reviews.len())
                .is_some()
            {
                return Err(invalid(
                    "Stored performance source review identity is repeated",
                ));
            }
            history.reviews.push(StoredPerformanceSourceReview {
                id,
                payload,
                payload_sha256,
            });
        }
    }
    let mut referenced = BTreeSet::new();
    {
        let mut statement = connection.prepare(
            "SELECT CASE WHEN typeof(id)='text' AND octet_length(id)=32 THEN id END,
                    CASE WHEN typeof(revision)='integer' AND revision BETWEEN 1 AND 16 THEN revision END,
                    CASE WHEN typeof(review_blob_sha256)='text' AND octet_length(review_blob_sha256)=64 THEN review_blob_sha256 END,
                    CASE WHEN typeof(payload)='blob' AND octet_length(payload) BETWEEN 1 AND 16384 THEN payload END,
                    CASE WHEN typeof(payload_sha256)='text' AND octet_length(payload_sha256)=64 THEN payload_sha256 END
             FROM performance_source_actions ORDER BY id COLLATE BINARY,revision LIMIT 513",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            if history.revisions.len() >= MAX_HISTORY_ROWS {
                return Err(invalid(
                    "Stored performance source action history exceeds its bounded row limit",
                ));
            }
            let id = bounded_text(row, 0)?;
            let revision = row
                .get::<_, Option<i64>>(1)
                .map_err(|_| {
                    invalid("Stored performance source action revision has an invalid type")
                })?
                .ok_or_else(|| {
                    invalid("Stored performance source action revision is outside 1..=16")
                })?;
            let review_blob_sha256 = bounded_text(row, 2)?;
            let payload = bounded_blob(row, 3)?;
            let payload_sha256 = bounded_text(row, 4)?;
            if !valid_id(&id) {
                return Err(invalid(
                    "Stored performance source action identity is invalid",
                ));
            }
            validate_object(&payload, MAX_PERFORMANCE_SOURCE_ACTION_BYTES, "action")?;
            if checksum(&payload) != payload_sha256 {
                return Err(invalid(
                    "Stored performance source action hash differs from its exact bytes",
                ));
            }
            let Some(&review_index) = review_indexes.get(&id) else {
                return Err(invalid(
                    "Stored performance source action has no immutable review",
                ));
            };
            if history.reviews[review_index].payload_sha256 != review_blob_sha256 {
                return Err(invalid(
                    "Stored performance source action references a different immutable review",
                ));
            }
            let expected = history
                .revisions
                .last()
                .filter(|previous| previous.id == id)
                .map_or(1, |previous| previous.revision + 1);
            if revision as u64 != expected {
                return Err(invalid(
                    "Stored performance source action history is missing or repeats a revision",
                ));
            }
            referenced.insert(id.clone());
            history.revisions.push(StoredPerformanceSourceAction {
                id,
                revision: revision as u64,
                review_blob_sha256,
                payload,
                payload_sha256,
            });
        }
    }
    if referenced.len() != history.reviews.len() {
        return Err(invalid(
            "Stored performance source review has no action history",
        ));
    }
    Ok(history)
}

impl NativeStore {
    /// Returns each immutable review once alongside every bounded revision.
    /// Opaque integrity does not establish typed choice or source authority.
    pub fn list_performance_source_actions(&self) -> Result<PerformanceSourceActionHistory> {
        let tx = self.connection.unchecked_transaction()?;
        let history = load_history(&tx)?;
        tx.commit()?;
        Ok(history)
    }

    /// Appends a choice while retaining its first exact review bytes. Saving
    /// metadata performs no filesystem, recovery, recording, or project action.
    pub fn save_performance_source_action(
        &mut self,
        id: &str,
        expected_revision: Option<u64>,
        review: &[u8],
        payload: &[u8],
    ) -> Result<StoredPerformanceSourceAction> {
        self.save_performance_source_action_inner(id, expected_revision, review, payload, None)
    }

    /// Fences a caller's complete typed-history validation against concurrent
    /// catalog writers. Even exact replay must observe that same history.
    pub fn save_performance_source_action_if_history_matches(
        &mut self,
        id: &str,
        expected_revision: Option<u64>,
        review: &[u8],
        payload: &[u8],
        expected_history: &PerformanceSourceActionHistory,
    ) -> Result<StoredPerformanceSourceAction> {
        self.save_performance_source_action_inner(
            id,
            expected_revision,
            review,
            payload,
            Some(expected_history),
        )
    }

    fn save_performance_source_action_inner(
        &mut self,
        id: &str,
        expected_revision: Option<u64>,
        review: &[u8],
        payload: &[u8],
        expected_history: Option<&PerformanceSourceActionHistory>,
    ) -> Result<StoredPerformanceSourceAction> {
        if !valid_id(id) {
            return Err(invalid(
                "Performance source action ID must be exactly 32 lowercase hexadecimal digits and nonzero",
            ));
        }
        validate_object(review, MAX_PERFORMANCE_SOURCE_REVIEW_BYTES, "review")?;
        validate_object(payload, MAX_PERFORMANCE_SOURCE_ACTION_BYTES, "action")?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut history = load_history(&tx)?;
        if expected_history.is_some_and(|expected| expected != &history) {
            return Err(StoreError::Conflict);
        }
        let existing_review = history.reviews.iter().find(|row| row.id == id);
        if existing_review.is_some_and(|stored| stored.payload != review) {
            return Err(invalid(
                "Performance source action requires its original exact immutable review bytes",
            ));
        }
        let previous = history.revisions.iter().rev().find(|row| row.id == id);
        // Only CURRENT exact-byte replay may bypass stale/None CAS or a full
        // ledger. Review equality and the entire retained history were checked.
        if let Some(previous) = previous.filter(|stored| stored.payload == payload) {
            let receipt = previous.clone();
            tx.commit()?;
            return Ok(receipt);
        }
        let revision = match previous {
            None if expected_revision.is_none() => {
                if history.reviews.len() >= MAX_PERFORMANCE_SOURCE_ACTIONS {
                    return Err(invalid(
                        "All 32 retained performance source action identities are occupied",
                    ));
                }
                1
            }
            Some(previous) if expected_revision == Some(previous.revision) => {
                if previous.revision >= MAX_PERFORMANCE_SOURCE_ACTION_REVISIONS {
                    return Err(invalid(
                        "Performance source action reached its 16 retained revision limit",
                    ));
                }
                previous.revision + 1
            }
            _ => return Err(StoreError::Conflict),
        };
        let review_blob_sha256 = checksum(review);
        let receipt = StoredPerformanceSourceAction {
            id: id.into(),
            revision,
            review_blob_sha256: review_blob_sha256.clone(),
            payload: payload.to_vec(),
            payload_sha256: checksum(payload),
        };
        if existing_review.is_none() {
            tx.execute(
                "INSERT INTO performance_source_reviews(id,payload,payload_sha256) VALUES(?1,?2,?3)",
                params![id, review, &review_blob_sha256],
            )?;
            history.reviews.push(StoredPerformanceSourceReview {
                id: id.into(),
                payload: review.to_vec(),
                payload_sha256: review_blob_sha256,
            });
            history.reviews.sort_by(|a, b| a.id.cmp(&b.id));
        }
        tx.execute(
            "INSERT INTO performance_source_actions(id,revision,review_blob_sha256,payload,payload_sha256) VALUES(?1,?2,?3,?4,?5)",
            params![id, revision as i64, &receipt.review_blob_sha256, payload, &receipt.payload_sha256],
        )?;
        history.revisions.push(receipt.clone());
        history
            .revisions
            .sort_by(|a, b| a.id.cmp(&b.id).then(a.revision.cmp(&b.revision)));
        // Authenticate the complete postimage and compare exact rows, including
        // unrelated identities and reviews. Trigger-created surplus or edits
        // must roll back with the first review and action append.
        if load_history(&tx)? != history {
            return Err(invalid(
                "Performance source action insertion changed retained history; transaction rolled back",
            ));
        }
        tx.commit()?;
        Ok(receipt)
    }
}

#[cfg(test)]
#[path = "performance_source_actions_review.rs"]
mod review;

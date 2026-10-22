//! Bounded local lifecycle metadata. These rows are not take originals, portable
//! song content, engine side effects, or permission to resume a recording.
use crate::{NativeStore, Result, StoreError, checksum};
use rusqlite::{Connection, TransactionBehavior, params};
use std::collections::BTreeSet;

pub const MAX_PERFORMANCE_INTENTS: usize = 32;
pub const MAX_PERFORMANCE_INTENT_REVISIONS: u64 = 16;
pub const MAX_PERFORMANCE_INTENT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPerformanceIntent {
    pub id: String,
    pub revision: u64,
    pub payload: Vec<u8>,
    pub payload_sha256: String,
}
fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}
fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && id.bytes().any(|c| c != b'0')
}
fn validate_payload(payload: &[u8]) -> Result<()> {
    if payload.is_empty() || payload.len() > MAX_PERFORMANCE_INTENT_BYTES {
        return Err(invalid("Performance intent must contain 1..=16384 bytes"));
    }
    let value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|_| invalid("Performance intent must be a valid UTF-8 JSON object"))?;
    if !value.is_object() {
        return Err(invalid("Performance intent must be a JSON object"));
    }
    Ok(())
}
pub(crate) fn load_history(connection: &Connection) -> Result<Vec<StoredPerformanceIntent>> {
    let mut statement=connection.prepare("SELECT CASE WHEN typeof(id)='text' AND octet_length(id)=32 THEN id END, revision, CASE WHEN typeof(payload)='blob' AND octet_length(payload) BETWEEN 1 AND 16384 THEN payload END, CASE WHEN typeof(payload_sha256)='text' AND octet_length(payload_sha256)=64 THEN payload_sha256 END FROM recording_intents ORDER BY id,revision LIMIT 513")?;
    let mut rows = statement.query([])?;
    let mut result: Vec<StoredPerformanceIntent> = Vec::new();
    let mut ids = BTreeSet::new();
    while let Some(row) = rows.next()? {
        if result.len() >= MAX_PERFORMANCE_INTENTS * MAX_PERFORMANCE_INTENT_REVISIONS as usize {
            return Err(invalid(
                "Stored performance intent history exceeds its bounded row limit",
            ));
        }
        let id: Option<String> = row.get(0)?;
        let revision: i64 = row.get(1)?;
        let payload: Option<Vec<u8>> = row.get(2)?;
        let sha: Option<String> = row.get(3)?;
        let (Some(id), Some(payload), Some(payload_sha256)) = (id, payload, sha) else {
            return Err(invalid(
                "Stored performance intent identity/payload exceeds its byte or type bounds",
            ));
        };
        if !valid_id(&id) || !(1..=MAX_PERFORMANCE_INTENT_REVISIONS as i64).contains(&revision) {
            return Err(invalid(
                "Stored performance intent identity/revision is invalid",
            ));
        }
        validate_payload(&payload)?;
        if checksum(&payload) != payload_sha256 {
            return Err(invalid(
                "Stored performance intent payload hash differs from its exact bytes",
            ));
        }
        let expected = if let Some(previous) = result.last().filter(|previous| previous.id == id) {
            previous.revision + 1
        } else {
            1
        };
        if revision as u64 != expected {
            return Err(invalid(
                "Stored performance intent history is missing or reorders a revision",
            ));
        }
        ids.insert(id.clone());
        if ids.len() > MAX_PERFORMANCE_INTENTS {
            return Err(invalid(
                "Stored performance intent identities exceed the retained limit of32",
            ));
        }
        result.push(StoredPerformanceIntent {
            id,
            revision: revision as u64,
            payload,
            payload_sha256,
        });
    }
    Ok(result)
}
impl NativeStore {
    /// Returns all bounded history rows. Callers must validate their owned typed
    /// payload and transitions before offering current restart-review choices.
    pub fn list_performance_intents(&self) -> Result<Vec<StoredPerformanceIntent>> {
        let tx = self.connection.unchecked_transaction()?;
        let rows = load_history(&tx)?;
        tx.commit()?;
        Ok(rows)
    }
    pub fn save_performance_intent(
        &mut self,
        id: &str,
        expected_revision: Option<u64>,
        payload: &[u8],
    ) -> Result<StoredPerformanceIntent> {
        if !valid_id(id) {
            return Err(invalid(
                "Performance intent ID must be exactly32 lowercase hexadecimal digits and nonzero",
            ));
        }
        validate_payload(payload)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut history = load_history(&tx)?;
        let previous = history.iter().rev().find(|row| row.id == id);
        // Exact CURRENT-byte replay precedes CAS, including an initial/stale
        // expectation after a lost reply. It never republishes an old revision.
        if let Some(previous) = previous.filter(|row| row.payload == payload) {
            let receipt = previous.clone();
            tx.commit()?;
            return Ok(receipt);
        }
        let revision = match previous {
            None if expected_revision.is_none() => {
                if history
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<BTreeSet<_>>()
                    .len()
                    >= MAX_PERFORMANCE_INTENTS
                {
                    return Err(invalid(
                        "All32 retained performance intent identities are occupied; no prior intention was removed",
                    ));
                }
                1
            }
            Some(previous) if expected_revision == Some(previous.revision) => {
                if previous.revision >= MAX_PERFORMANCE_INTENT_REVISIONS {
                    return Err(invalid(
                        "Performance intent reached its16 retained revision limit",
                    ));
                }
                previous.revision + 1
            }
            _ => return Err(StoreError::Conflict),
        };
        let receipt = StoredPerformanceIntent {
            id: id.into(),
            revision,
            payload: payload.to_vec(),
            payload_sha256: checksum(payload),
        };
        tx.execute(
            "INSERT INTO recording_intents(id,revision,payload,payload_sha256) VALUES(?1,?2,?3,?4)",
            params![id, revision as i64, payload, &receipt.payload_sha256],
        )?;
        history.push(receipt.clone());
        history.sort_by(|a, b| a.id.cmp(&b.id).then(a.revision.cmp(&b.revision)));
        if load_history(&tx)? != history {
            return Err(invalid(
                "Performance intent insertion changed retained history; transaction rolled back",
            ));
        }
        tx.commit()?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/performance-intent-store-tests")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn id(index: u64) -> String {
        format!("{index:032x}")
    }
    #[test]
    fn exact_current_replay_precedes_cas_and_history_survives_reopen() {
        let f = Fixture::new();
        let mut s = NativeStore::open(&f.0).unwrap();
        let first = s
            .save_performance_intent(&id(1), None, br#"{"version":1,"raw":1}"#)
            .unwrap();
        assert_eq!(
            s.save_performance_intent(&id(1), Some(999), &first.payload)
                .unwrap(),
            first
        );
        let second = s
            .save_performance_intent(&id(1), Some(1), br#"{"version":1,"raw":2}"#)
            .unwrap();
        assert_eq!(
            s.save_performance_intent(&id(1), None, &second.payload)
                .unwrap(),
            second
        );
        assert!(matches!(
            s.save_performance_intent(&id(1), Some(1), &first.payload),
            Err(StoreError::Conflict)
        ));
        assert_eq!(
            s.list_performance_intents().unwrap(),
            vec![first.clone(), second.clone()]
        );
        drop(s);
        let s = NativeStore::open_existing(&f.0).unwrap();
        assert_eq!(s.list_performance_intents().unwrap(), vec![first, second]);
    }
    #[test]
    fn invalid_ids_payloads_and_retained_caps_never_evict_prior_rows() {
        let f = Fixture::new();
        let mut s = NativeStore::open(&f.0).unwrap();
        for bad in ["", &"0".repeat(32), &"A".repeat(32), "../capture"] {
            assert!(s.save_performance_intent(bad, None, b"{}").is_err());
        }
        for bad in [
            b"[]".as_slice(),
            b"null",
            b"{",
            &[0xff],
            &vec![b' '; MAX_PERFORMANCE_INTENT_BYTES + 1],
        ] {
            assert!(s.save_performance_intent(&id(1), None, bad).is_err());
        }
        for index in 1..=32 {
            s.save_performance_intent(&id(index), None, b"{}").unwrap();
        }
        assert!(s.save_performance_intent(&id(33), None, b"{}").is_err());
        for revision in 2..=16 {
            s.save_performance_intent(
                &id(1),
                Some(revision - 1),
                format!("{{\"revision\":{revision}}}").as_bytes(),
            )
            .unwrap();
        }
        assert!(
            s.save_performance_intent(&id(1), Some(16), b"{\"revision\":17}")
                .is_err()
        );
        assert_eq!(s.list_performance_intents().unwrap().len(), 47);
    }
    #[test]
    fn corrupted_history_and_trigger_insertions_fail_closed_without_publication() {
        let f = Fixture::new();
        let mut s = NativeStore::open(&f.0).unwrap();
        s.save_performance_intent(&id(1), None, b"{}").unwrap();
        s.connection.execute_batch("CREATE TRIGGER injected_intent AFTER INSERT ON recording_intents WHEN NEW.revision=2 BEGIN INSERT INTO recording_intents(id,revision,payload,payload_sha256) VALUES(NEW.id,3,NEW.payload,NEW.payload_sha256); END;").unwrap();
        let before = s.list_performance_intents().unwrap();
        assert!(
            s.save_performance_intent(&id(1), Some(1), b"{\"x\":1}")
                .is_err()
        );
        assert_eq!(s.list_performance_intents().unwrap(), before);
        s.connection
            .execute_batch(
                "DROP TRIGGER injected_intent; DROP TRIGGER immutable_recording_intent_update;",
            )
            .unwrap();
        s.connection
            .execute(
                "UPDATE recording_intents SET payload_sha256=?1",
                ["0".repeat(64)],
            )
            .unwrap();
        assert!(s.list_performance_intents().is_err());
        assert!(s.save_performance_intent(&id(2), None, b"{}").is_err());
        let count: i64 = s
            .connection
            .query_row("SELECT count(*) FROM recording_intents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
    #[test]
    fn independent_writers_cannot_both_append_against_the_same_observed_revision() {
        let f = Fixture::new();
        let mut store = NativeStore::open(&f.0).unwrap();
        store.save_performance_intent(&id(1), None, b"{}").unwrap();
        drop(store);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|choice| {
                let root = f.0.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = NativeStore::open_existing(root).unwrap();
                    barrier.wait();
                    store.save_performance_intent(
                        &id(1),
                        Some(1),
                        format!("{{\"choice\":{choice}}}").as_bytes(),
                    )
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(StoreError::Conflict)))
                .count(),
            1
        );
        assert_eq!(
            NativeStore::open_existing(&f.0)
                .unwrap()
                .list_performance_intents()
                .unwrap()
                .len(),
            2
        );
    }
}

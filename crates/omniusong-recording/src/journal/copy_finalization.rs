//! Recovered-only writes to an already owned, authenticated private working copy.
//!
//! The caller retains the working file/owner and source/evidence guards for this
//! entire call. This module neither obtains normal-seal authority nor opens an
//! existing action as a retry. An error after entry may follow a committed write;
//! the outer copy protocol must retain that uncertainty and every artifact.

use super::*;
use crate::InspectionBudget;
use serde::Serialize;
use std::io::{self, Write};

const MAX_WORKING_BYTES: u64 = 536_870_912;
const MAX_SCHEMA_ROWS: usize = 64;

#[derive(Debug, PartialEq, Eq)]
struct SchemaRow {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}

fn schema_rows(connection: &Connection, budget: &InspectionBudget) -> Result<Vec<SchemaRow>> {
    let mut statement = connection.prepare(
        "SELECT CASE WHEN typeof(type)='text' AND octet_length(type) BETWEEN 1 AND 16 THEN type END,
                CASE WHEN typeof(name)='text' AND octet_length(name) BETWEEN 1 AND 256 THEN name END,
                CASE WHEN typeof(tbl_name)='text' AND octet_length(tbl_name) BETWEEN 1 AND 256 THEN tbl_name END,
                CASE WHEN typeof(sql)='text' AND octet_length(sql)<=262144 THEN sql END,
                sql IS NULL OR (typeof(sql)='text' AND octet_length(sql)<=262144),
                CASE WHEN typeof(sql)='text' AND octet_length(sql)<=262144 THEN octet_length(sql) ELSE 0 END
         FROM sqlite_schema LIMIT 65",
    )?;
    let mut rows = statement.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        budget.checkpoint()?;
        require(
            result.len() < MAX_SCHEMA_ROWS,
            "Recovery copy schema exceeds its bounded object count",
        )?;
        budget.physical_row()?;
        budget.visit_rows(6)?;
        let sql_length = read_unsigned(row, 5)?;
        budget.charge_work(528 + sql_length.saturating_mul(2))?;
        require(
            row.get::<_, bool>(4)?,
            "Recovery copy schema SQL is invalid or oversized",
        )?;
        let text = |column| -> Result<String> {
            row.get::<_, Option<String>>(column)?.ok_or_else(|| {
                RecordingError::Invalid(
                    "Recovery copy schema identity is invalid or oversized".into(),
                )
            })
        };
        result.push(SchemaRow {
            kind: text(0)?,
            name: text(1)?,
            table: text(2)?,
            sql: row.get(3)?,
        });
    }
    // Bound materialization before sorting; SQL ORDER BY would sort the entire
    // untrusted schema before its LIMIT could reject an excessive object count.
    result.sort_by(|left, right| (&left.kind, &left.name).cmp(&(&right.kind, &right.name)));
    Ok(result)
}

/// The read-only caller runs this before its first writable open; recovery
/// repeats it under IMMEDIATE. Unknown extensions remain in the evidence image
/// but are not admitted for derived writes. Root-page numbers are physical
/// allocation details, not schema programs, and intentionally are not compared.
pub(crate) fn validate_copy_schema(
    connection: &Connection,
    budget: &InspectionBudget,
) -> Result<()> {
    let result = (|| {
        budget.install(connection)?;
        budget.visit_rows(1)?;
        budget.charge_work(4)?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        require(
            version == JOURNAL_VERSION,
            "Recovery copy schema version is unsupported",
        )?;
        let actual = schema_rows(connection, budget)?;
        let canonical = Connection::open_in_memory()?;
        budget.install(&canonical)?;
        budget.charge_work((SCHEMA.len() as u64).saturating_mul(2))?;
        canonical.execute_batch(SCHEMA)?;
        require(
            actual == schema_rows(&canonical, budget)?,
            "Recovery copy requires the canonical journal-v1 schema; extensions or altered triggers were retained without recovery",
        )
    })();
    budget.finish(result)
}

struct MetadataWriter<'a> {
    bytes: Vec<u8>,
    limit: usize,
    budget: &'a InspectionBudget,
}

impl Write for MetadataWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "Recovery copy metadata exceeds its byte limit",
            ));
        }
        self.budget
            .charge_work(bytes.len() as u64)
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn metadata<T: Serialize>(value: &T, limit: usize, budget: &InspectionBudget) -> Result<Vec<u8>> {
    let mut writer = MetadataWriter {
        bytes: Vec::new(),
        limit,
        budget,
    };
    let result = serde_json::to_writer(&mut writer, value)
        .map_err(|error| RecordingError::Invalid(error.to_string()))
        .map(|()| writer.bytes);
    budget.finish(result)
}

fn charged_digest(domain: &[u8], bytes: &[u8], budget: &InspectionBudget) -> Result<String> {
    budget.charge_work((domain.len() as u64).saturating_add(bytes.len() as u64))?;
    Ok(digest(domain, bytes))
}

fn derived_counts(connection: &Connection, budget: &InspectionBudget) -> Result<(u64, u64, u64)> {
    // preflight_connection has already bounded every physical row.
    budget.visit_rows(3)?;
    budget.charge_work(24)?;
    Ok(connection.query_row(
        "SELECT (SELECT count(*) FROM interruptions),
                (SELECT count(*) FROM recovery_index),
                (SELECT count(*) FROM sealed_manifests)",
        [],
        |row| {
            Ok((
                read_unsigned(row, 0)?,
                read_unsigned(row, 1)?,
                read_unsigned(row, 2)?,
            ))
        },
    )?)
}

fn cap_working_pages(connection: &Connection, budget: &InspectionBudget) -> Result<u64> {
    budget.visit_rows(3)?;
    budget.charge_work(24)?;
    let page_size: u64 =
        connection.pragma_query_value(None, "page_size", |row| read_unsigned(row, 0))?;
    let page_count: u64 =
        connection.pragma_query_value(None, "page_count", |row| read_unsigned(row, 0))?;
    require(
        (512..=65_536).contains(&page_size) && page_size.is_power_of_two(),
        "Recovery copy has an unsupported SQLite page size",
    )?;
    let maximum = MAX_WORKING_BYTES / page_size;
    require(
        page_count > 0 && page_count <= maximum,
        "Recovery copy exceeds its working-file ceiling",
    )?;
    let applied: u64 =
        connection.query_row(&format!("PRAGMA max_page_count={maximum}"), [], |row| {
            read_unsigned(row, 0)
        })?;
    require(
        applied == maximum,
        "SQLite did not apply the recovery working-file ceiling",
    )?;
    Ok(page_size)
}

/// The only write mode here is recovered. The selected prefix has already been
/// semantically admitted by the performance layer; it is independently checked
/// against the entire held generic snapshot and retained interruption index.
pub(crate) fn recover_copy_bounded(
    path: &Path,
    expected: &crate::JournalInspection,
    selection: &RecoveryPrefix,
    operation_id: &str,
    budget: &InspectionBudget,
) -> Result<SealedManifest> {
    let result = (|| {
        budget.checkpoint()?;
        identifier(operation_id)?;
        selection.validate()?;
        require(
            expected.path == path
                && expected.manifest.is_none()
                && matches!(
                    expected.report.state,
                    TakeState::Recording | TakeState::Interrupted
                )
                && expected.report.unknown_tail
                && selection.chunk_count > 0
                && selection.chunk_count <= expected.report.verified_chunks,
            "Recovery copy requires the exact unsealed working snapshot and a positive selected prefix",
        )?;
        let canonical_path = crate::inspection::preflight_path(path, budget)?;
        require(
            canonical_path == path,
            "Recovery working path differs from its canonical admission",
        )?;
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        budget.install(&connection)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        // Do not assign journal_mode: the caller and path preflight already
        // require a sidecar-free rollback image. These settings are local.
        connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;")?;
        budget.visit_rows(3)?;
        budget.charge_work(24)?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        let synchronous: u32 =
            connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
        require(
            version == JOURNAL_VERSION && mode.eq_ignore_ascii_case("delete") && synchronous == 2,
            "Recovery copy requires journal-v1 rollback mode and synchronous FULL",
        )?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_copy_schema(&tx, budget)?;
        crate::inspection::preflight_connection(&tx, budget)?;
        let descriptor = load_descriptor_bounded(&tx, Some(budget))?;
        let current =
            crate::inspection::inspect_connection_bounded(&tx, path, &descriptor, Some(budget))?;
        require(
            current == *expected,
            "Recovery working copy changed after its held review",
        )?;
        let page_size = cap_working_pages(&tx, budget)?;
        let report = scan_bounded(&tx, &descriptor, Some(selection.chunk_count), Some(budget))?;
        require(
            report.problems.is_empty()
                && report.verified_chunks == selection.chunk_count
                && report.chunk_index_sha256 == selection.chunk_index_sha256
                && report
                    .streams
                    .iter()
                    .zip(&descriptor.streams)
                    .any(|(prefix, stream)| {
                        matches!(stream.kind, StreamKind::Audio { .. }) && prefix.recorded_units > 0
                    }),
            "Selected recovery prefix differs from verified original audio",
        )?;
        let counts = derived_counts(&tx, budget)?;
        let interrupted = match current.report.state {
            TakeState::Recording => {
                require(
                    counts == (0, 0, 0),
                    "Recording copy contains unexpected derived metadata",
                )?;
                let mut full_report = current.report.clone();
                full_report.state = TakeState::Interrupted;
                full_report.unknown_tail = true;
                let receipt_id = uuid::Uuid::new_v4().to_string();
                let bytes = metadata(
                    &(receipt_id.clone(), &full_report),
                    MAX_METADATA_BYTES,
                    budget,
                )?;
                let sha256 = charged_digest(b"OmniuSong.interrupted-prefix.v1\0", &bytes, budget)?;
                let receipt = InterruptedReceipt {
                    receipt_id,
                    report: full_report,
                    sha256,
                };
                let encoded = metadata(&receipt, MAX_METADATA_BYTES, budget)?;
                // Account for the selected SQL read and inserted index fields;
                // SQLite's shared progress hook also governs the INSERT SELECT.
                budget.visit_rows(receipt.report.verified_chunks.saturating_mul(2))?;
                budget.charge_work(receipt.report.verified_chunks.saturating_mul(144))?;
                let inserted = tx.execute(
                    "INSERT INTO recovery_index(ordinal,sha256) SELECT ordinal,sha256 FROM chunks WHERE ordinal<=?1 ORDER BY ordinal",
                    [sql_integer(receipt.report.verified_chunks)?],
                )?;
                require(
                    inserted as u64 == receipt.report.verified_chunks,
                    "Interrupted copy index is incomplete",
                )?;
                budget.visit_rows(2)?;
                budget.charge_work(encoded.len() as u64 + 64 + 12)?;
                require(
                    tx.execute(
                        "INSERT INTO interruptions(singleton,receipt,sha256) VALUES(1,?1,?2)",
                        params![encoded, receipt.sha256],
                    )? == 1,
                    "Recovery copy did not retain its interruption receipt",
                )?;
                require(
                    tx.execute(
                        "UPDATE take_meta SET state='interrupted' WHERE singleton=1",
                        [],
                    )? == 1,
                    "Recovery copy did not enter interrupted mode",
                )?;
                require(
                    load_interrupted_bounded(&tx, Some(budget))? == receipt,
                    "Recovery copy interruption differs from the retained request",
                )?;
                receipt
            }
            TakeState::Interrupted => {
                let receipt = load_interrupted_bounded(&tx, Some(budget))?;
                require(
                    counts == (1, receipt.report.verified_chunks, 0),
                    "Interrupted copy contains surplus or missing derived metadata",
                )?;
                receipt
            }
            TakeState::Sealed => {
                return Err(RecordingError::Conflict(
                    "A sealed working copy is not an initial recovery request".into(),
                ));
            }
        };
        require(
            report.verified_chunks <= interrupted.report.verified_chunks
                && report.chunk_index_sha256
                    == interrupted_prefix_hash_bounded(
                        &tx,
                        &descriptor,
                        report.verified_chunks,
                        Some(budget),
                    )?,
            "Selected recovery prefix differs from the retained interrupted index",
        )?;
        let interrupted_chunks = interrupted.report.verified_chunks;
        let body = ManifestBody {
            version: JOURNAL_VERSION,
            manifest_id: uuid::Uuid::new_v4().to_string(),
            operation_id: operation_id.into(),
            descriptor,
            descriptor_sha256: current.descriptor_sha256.clone(),
            streams: report.streams,
            chunk_count: report.verified_chunks,
            payload_bytes: report.verified_payload_bytes,
            chunk_index_sha256: report.chunk_index_sha256,
            interrupted: Some(interrupted),
        };
        let body_bytes = metadata(&body, MAX_METADATA_BYTES * 2, budget)?;
        let sha256 = charged_digest(b"OmniuSong.sealed-take.v1\0", &body_bytes, budget)?;
        let manifest = SealedManifest { body, sha256 };
        let encoded = metadata(&manifest, MAX_METADATA_BYTES * 2, budget)?;
        budget.visit_rows(2)?;
        budget.charge_work(encoded.len() as u64 + operation_id.len() as u64 + 64 + 6)?;
        require(tx.execute("INSERT INTO sealed_manifests(singleton,operation_id,body,sha256) VALUES(1,?1,?2,?3)",
            params![operation_id, encoded, manifest.sha256])? == 1,
            "Recovery copy did not retain its recovered manifest")?;
        require(
            tx.execute("UPDATE take_meta SET state='sealed' WHERE singleton=1", [])? == 1,
            "Recovery copy did not enter sealed mode",
        )?;
        require(
            crate::inspection::original_rows_hash_bounded(&tx, Some(budget))?
                == expected.original_rows_sha256,
            "Recovery changed original rows; working transaction rolled back",
        )?;
        require(
            verify_sealed_bounded(&tx, &manifest.body.descriptor, Some(budget))? == manifest
                && manifest
                    .body
                    .interrupted
                    .as_ref()
                    .is_some_and(|receipt| receipt.report.unknown_tail)
                && derived_counts(&tx, budget)? == (1, interrupted_chunks, 1),
            "Recovery copy postimage differs from its recovered-only manifest",
        )?;
        budget.visit_rows(1)?;
        budget.charge_work(8)?;
        let page_count: u64 =
            tx.pragma_query_value(None, "page_count", |row| read_unsigned(row, 0))?;
        let working_bytes = page_count
            .checked_mul(page_size)
            .ok_or_else(|| RecordingError::Invalid("Recovery working-file size overflow".into()))?;
        require(
            working_bytes <= MAX_WORKING_BYTES,
            "Recovery working-file ceiling was exceeded",
        )?;
        // A caller may choose a smaller inspection ceiling. Check transaction
        // growth before COMMIT as well as the fixed SQLite page ceiling.
        budget.file(working_bytes)?;
        budget.checkpoint()?;
        tx.commit()?;
        Ok(manifest)
    })();
    // This may report a sticky stop after COMMIT. The caller must conservatively
    // retain the working artifact; this is never a claim of rollback or retry.
    budget.finish(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool};

    #[test]
    fn schema_authorization_refuses_extensions_altered_triggers_and_future_versions() {
        for change in [
            None,
            Some("ALTER TABLE chunks ADD COLUMN vendor_evidence BLOB"),
            Some("CREATE INDEX vendor_index ON chunks(sequence)"),
            Some(
                "CREATE TRIGGER vendor_side_effect AFTER UPDATE ON take_meta BEGIN DELETE FROM sealed_manifests; END",
            ),
            Some("DROP TRIGGER immutable_descriptor"),
            Some("PRAGMA user_version=2"),
        ] {
            let connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(SCHEMA).unwrap();
            if let Some(sql) = change {
                connection.execute_batch(sql).unwrap();
            }
            let budget = InspectionBudget::new(
                crate::InspectionLimits::default(),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
            let before = schema_rows(&connection, &budget).unwrap();
            let accepted = validate_copy_schema(&connection, &budget).is_ok();
            assert_eq!(accepted, change.is_none());
            assert_eq!(schema_rows(&connection, &budget).unwrap(), before);
            assert!(budget.usage().vm_steps > 0);
        }
    }

    #[test]
    fn schema_authorization_keeps_cancellation_and_vm_exhaustion_typed_and_sticky() {
        for cancelled in [false, true] {
            let connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(SCHEMA).unwrap();
            let budget = InspectionBudget::new(
                crate::InspectionLimits {
                    max_vm_steps: 1,
                    ..Default::default()
                },
                Arc::new(AtomicBool::new(cancelled)),
            )
            .unwrap();
            let error = validate_copy_schema(&connection, &budget).unwrap_err();
            if cancelled {
                assert!(matches!(error, RecordingError::InspectionCancelled));
                assert!(matches!(
                    budget.checkpoint(),
                    Err(RecordingError::InspectionCancelled)
                ));
            } else {
                assert!(matches!(
                    error,
                    RecordingError::InspectionLimit {
                        resource: crate::InspectionResource::VmSteps,
                        ..
                    }
                ));
                assert!(matches!(
                    budget.checkpoint(),
                    Err(RecordingError::InspectionLimit {
                        resource: crate::InspectionResource::VmSteps,
                        ..
                    })
                ));
            }
        }
    }
}

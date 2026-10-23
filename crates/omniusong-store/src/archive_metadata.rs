//! Bounded traversal of untrusted retained metadata, never source admission.
//!
//! The caller owns a stable source transaction and its SQLite progress hook.
//! Every pass uses the caller's one cumulative governor. A syntactically valid
//! projection registry remains a claim until the separate source/history
//! closure authenticates it; legacy guarded consumers are unchanged.
use crate::{
    Result,
    archive::{self, Bundle, Destination, Totals, add, digest_valid, ensure, invalid, unsigned},
    archive_boundary::{ArchiveSqlOrigin, sql_error},
    archive_operation::ArchiveOperation,
    checksum, projection_publication,
    takes::{TakeObject, validate_object},
};
use omniusong_core::{
    MAX_NATIVE_PROJECT_BYTES, MAX_PROJECT_BYTES, ProjectDocument,
    contracts::{ProjectId, TakeId, Validate, WORKER_MAX_BYTES},
};
use omniusong_recording::{InspectionBudget, SealedManifest};
use rusqlite::{Connection, OptionalExtension, params, types::ValueRef as SqlCell};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Write};

// The ordinary archive's existing ceilings, not a second governor.
const MAX_EDITS: u64 = 10_000;
const MAX_OPERATIONS: u64 = 100_000;
const MAX_MANIFEST_BYTES: usize = 512 * 1024;
const MAX_ID_BYTES: usize = 96;

fn sql<T>(origin: ArchiveSqlOrigin, result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|error| sql_error(origin, error))
}

fn finish<T>(budget: &InspectionBudget, result: Result<T>) -> Result<T> {
    // Origin tagging happens at each handle, before this final checkpoint.
    // Preserve every primary failure, including private-source damage and
    // nested main-catalog taint. The enclosing archive boundary records a
    // simultaneous governor stop separately instead of replacing this error.
    match result {
        Err(error) => Err(error),
        Ok(value) => {
            budget.checkpoint()?;
            Ok(value)
        }
    }
}

fn step(budget: &InspectionBudget) -> Result<()> {
    budget.visit_rows(1)?;
    // Covers bounded fixed cells, statement/row bookkeeping and ID copies.
    budget.charge_work(8192)?;
    Ok(())
}

fn bounded_length(length: Option<i64>, maximum: usize) -> Result<usize> {
    length
        .and_then(|length| usize::try_from(length).ok())
        .filter(|length| *length <= maximum)
        .ok_or_else(|| invalid("Archive metadata has an invalid type or exceeds its byte limit"))
}

fn prepay(budget: &InspectionBudget, length: usize) -> Result<()> {
    budget.charge_work((length as u64).saturating_mul(64).saturating_add(4096))?;
    Ok(())
}

fn text(value: Option<String>) -> Result<String> {
    value.ok_or_else(|| invalid("Archive metadata is missing or exceeds its byte limit"))
}

fn count(
    connection: &Connection,
    query: &str,
    project_id: &str,
    maximum: u64,
    origin: ArchiveSqlOrigin,
    budget: &InspectionBudget,
) -> Result<u64> {
    step(budget)?;
    let count = unsigned(
        sql(
            origin,
            connection.query_row(query, [project_id], |row| row.get(0)),
        )?,
        "metadata row count",
    )?;
    ensure(
        count <= maximum,
        "Archive metadata row count exceeds its explicit budget",
    )?;
    Ok(count)
}

/// A bounded pretty encoding matching `ProjectDocument::to_json()` exactly.
/// Measuring the already decoded tree does not clone or serialize it. String
/// scans and the fixed number formatting are prepaid before inspection; the
/// complete encoding/allocation is prepaid before the serializer is invoked.
pub(crate) fn document_json(
    document: &ProjectDocument,
    budget: &InspectionBudget,
) -> Result<String> {
    finish(
        budget,
        (|| {
            budget.checkpoint()?;
            let maximum = document.metadata_limit();
            let length = encoded_length(document.value(), 0, maximum, budget)?;
            prepay(budget, length)?;
            let mut output = BoundedJson {
                bytes: Vec::new(),
                maximum: length,
                budget,
            };
            output
                .bytes
                .try_reserve_exact(length)
                .map_err(|_| invalid("Cannot reserve bounded archive document encoding"))?;
            let encoded = serde_json::to_writer_pretty(&mut output, document.value());
            budget.checkpoint()?;
            encoded.map_err(|error| invalid(error.to_string()))?;
            ensure(
                output.bytes.len() == length,
                "Archive document encoding length differs",
            )?;
            String::from_utf8(output.bytes).map_err(|error| invalid(error.to_string()))
        })(),
    )
}

fn length_add(total: &mut usize, amount: usize, maximum: usize) -> Result<()> {
    *total = total
        .checked_add(amount)
        .filter(|length| *length <= maximum)
        .ok_or_else(|| invalid("Archive document encoding exceeds its byte limit"))?;
    Ok(())
}

fn string_length(value: &str, maximum: usize, budget: &InspectionBudget) -> Result<usize> {
    budget.charge_work(value.len() as u64 + 64)?;
    let mut length = 2;
    for byte in value.bytes() {
        length_add(
            &mut length,
            match byte {
                b'"' | b'\\' | b'\x08' | b'\x0c' | b'\n' | b'\r' | b'\t' => 2,
                0..=0x1f => 6,
                _ => 1,
            },
            maximum,
        )?;
    }
    Ok(length)
}

struct NumberLength<'b> {
    length: usize,
    maximum: usize,
    budget: &'b InspectionBudget,
}
impl std::fmt::Write for NumberLength<'_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.budget
            .charge_work(value.len() as u64 + 64)
            .map_err(|_| std::fmt::Error)?;
        self.length = self
            .length
            .checked_add(value.len())
            .filter(|length| *length <= self.maximum)
            .ok_or(std::fmt::Error)?;
        Ok(())
    }
}

fn encoded_length(
    value: &Value,
    depth: usize,
    maximum: usize,
    budget: &InspectionBudget,
) -> Result<usize> {
    budget.charge_work(128)?;
    ensure(
        depth <= 128,
        "Archive document nesting exceeds its bounded decoder",
    )?;
    let mut length = match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        // Count primitive formatting without allocating a temporary String.
        // The counter also handles a dependency enabling arbitrary precision.
        Value::Number(number) => {
            let mut counter = NumberLength {
                length: 0,
                maximum,
                budget,
            };
            let measured = std::fmt::write(&mut counter, format_args!("{number}"));
            budget.checkpoint()?;
            measured.map_err(|_| invalid("Archive JSON number exceeds its byte limit"))?;
            counter.length
        }
        Value::String(value) => return string_length(value, maximum, budget),
        Value::Array(values) if values.is_empty() => 2,
        Value::Object(values) if values.is_empty() => 2,
        Value::Array(values) => {
            let mut length = 3 + depth * 2;
            for (index, child) in values.iter().enumerate() {
                length_add(
                    &mut length,
                    (depth + 1) * 2 + 1 + usize::from(index + 1 < values.len()),
                    maximum,
                )?;
                length_add(
                    &mut length,
                    encoded_length(child, depth + 1, maximum, budget)?,
                    maximum,
                )?;
            }
            length
        }
        Value::Object(values) => {
            let mut length = 3 + depth * 2;
            for (index, (key, child)) in values.iter().enumerate() {
                length_add(
                    &mut length,
                    (depth + 1) * 2 + 3 + usize::from(index + 1 < values.len()),
                    maximum,
                )?;
                length_add(&mut length, string_length(key, maximum, budget)?, maximum)?;
                length_add(
                    &mut length,
                    encoded_length(child, depth + 1, maximum, budget)?,
                    maximum,
                )?;
            }
            length
        }
    };
    // Also bound primitive roots (the project itself is always an object).
    length_add(&mut length, 0, maximum)?;
    Ok(length)
}

struct BoundedJson<'b> {
    bytes: Vec<u8>,
    maximum: usize,
    budget: &'b InspectionBudget,
}
impl Write for BoundedJson<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.budget
            .checkpoint()
            .map_err(|_| std::io::Error::other("Archive document governor stopped encoding"))?;
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "Archive document encoding exceeds its byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Validate row/request consistency only. This does not prove an original,
/// immutable forward history, a publication preimage, or source availability.
pub(crate) fn validate_request(
    request: &str,
    operation_id: &str,
    project_id: &str,
    previous: u64,
    result: &ProjectDocument,
    budget: &InspectionBudget,
) -> Result<()> {
    finish(
        budget,
        validate_request_inner(
            request,
            operation_id,
            project_id,
            previous,
            result,
            None,
            budget,
        ),
    )
}

fn validate_request_inner(
    request: &str,
    operation_id: &str,
    project_id: &str,
    previous: u64,
    result: &ProjectDocument,
    retained_result_sha: Option<&str>,
    budget: &InspectionBudget,
) -> Result<()> {
    ensure(
        request.len() <= WORKER_MAX_BYTES,
        "Archive operation exceeds its byte limit",
    )?;
    prepay(budget, request.len())?;
    // Covers bounded postimage extraction and the publication validator's
    // canonical-result serialization before either can clone metadata.
    let result_json = document_json(result, budget)?;
    let operation = ArchiveOperation::parse(request)?;
    operation.validate_identity(operation_id, project_id, previous)?;
    match operation {
        ArchiveOperation::Projection(intent) => {
            let canonical_sha = checksum(result_json.as_bytes());
            projection_publication::validate_publication_result(
                &intent,
                request,
                result,
                retained_result_sha.unwrap_or(&canonical_sha),
                budget,
            )?;
            Ok(())
        }
        ArchiveOperation::Edit(_) | ArchiveOperation::Take(_) => {
            archive::validate_request(request, operation_id, project_id, previous, result)
        }
    }
}

/// Preserve the ordinary OSAR logical index, totals, row order and exact raw
/// opaque bytes while widening only the untrusted metadata decoder.
pub(crate) fn copy_metadata(
    source: &Connection,
    destination: Option<Destination<'_>>,
    project_id: &str,
    origin: ArchiveSqlOrigin,
    budget: &InspectionBudget,
) -> Result<Bundle> {
    finish(
        budget,
        (|| {
            ensure(
                project_id.len() <= MAX_ID_BYTES,
                "Project identity exceeds its limit",
            )?;
            prepay(budget, project_id.len())?;
            ProjectId(project_id.into())
                .validate()
                .map_err(|error| invalid(error.to_string()))?;
            step(budget)?;
            let (length, revision, revision_type, cursor, cursor_type): (Option<i64>, i64, String, i64, String) = sql(origin, source.query_row(
            "SELECT CASE WHEN typeof(payload)='text' THEN octet_length(payload) END,revision,typeof(revision),cursor,typeof(cursor) FROM projects WHERE id=?1",
            [project_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ))?;
            ensure(
                revision_type == "integer" && cursor_type == "integer",
                "Archive project revision/cursor has an invalid type",
            )?;
            let length = bounded_length(length, MAX_NATIVE_PROJECT_BYTES)?;
            let revision_u64 = unsigned(revision, "revision")?;
            let cursor = unsigned(cursor, "cursor")?;
            let history_count = count(
                source,
                "SELECT count(*) FROM edits WHERE project_id=?1",
                project_id,
                MAX_EDITS,
                origin,
                budget,
            )?;
            let operation_count = count(
                source,
                "SELECT count(*) FROM operations WHERE project_id=?1",
                project_id,
                MAX_OPERATIONS,
                origin,
                budget,
            )?;
            let import_count = count(
                source,
                "SELECT count(*) FROM imports WHERE project_id=?1",
                project_id,
                1,
                origin,
                budget,
            )?;
            ensure(
                cursor <= history_count,
                "Archive cursor exceeds retained history",
            )?;
            // The exact payload and row scalars below come from the same query and
            // are retained for hashing/insertion. No reserialized substitute.
            prepay(budget, length + project_id.len() + 16)?;
            step(budget)?;
            let (payload, actual_revision, actual_cursor): (Option<String>, i64, i64) = sql(origin, source.query_row(
            "SELECT CASE WHEN typeof(payload)='text' AND octet_length(payload)<=?2 THEN payload END,revision,cursor FROM projects WHERE id=?1",
            params![project_id, MAX_NATIVE_PROJECT_BYTES as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ))?;
            let payload = text(payload)?;
            ensure(
                payload.len() == length
                    && actual_revision == revision
                    && u64::try_from(actual_cursor).ok() == Some(cursor),
                "Archive current metadata changed during traversal",
            )?;
            budget.visit_records(1)?;
            let project = ProjectDocument::from_json(payload.as_bytes())?;
            ensure(
                project.id() == project_id && project.revision() == revision_u64,
                "Stored project identity/revision is inconsistent",
            )?;
            let mut bundle = Bundle {
                project: project.clone(),
                assets: BTreeSet::new(),
                takes: BTreeSet::new(),
                totals: Totals::default(),
                index: Sha256::new(),
            };
            bundle.index.update(b"OmniuSong.project-archive.v1\0");
            bundle.metadata(payload.len() + project_id.len() + 16)?;
            bundle.references_untrusted(&project)?;
            bundle.record(
                b"project",
                &[
                    project_id.as_bytes(),
                    payload.as_bytes(),
                    &revision.to_le_bytes(),
                    &cursor.to_le_bytes(),
                ],
            );
            if let Some(target) = destination {
                match target {
                    Destination::ArchiveExpected(connection, _) => {
                        let sha = checksum(payload.as_bytes());
                        target.expect(
                            "projects",
                            &[
                                SqlCell::Text(project_id.as_bytes()),
                                SqlCell::Text(payload.as_bytes()),
                                SqlCell::Integer(revision),
                                SqlCell::Integer(cursor as i64),
                                SqlCell::Text(sha.as_bytes()),
                            ],
                        )?;
                        step(budget)?;
                        target.sql(connection.execute(
                            "INSERT INTO projects VALUES(?1,?2,?3,?4,?5)",
                            params![project_id, payload, revision, cursor as i64, sha],
                        ))?;
                    }
                    Destination::Store(connection) | Destination::PlanCatalog(connection) => {
                        step(budget)?;
                        target.sql(connection.execute(
                            "INSERT INTO projects(id,payload,revision,cursor) VALUES(?1,?2,?3,?4)",
                            params![project_id, payload, revision, cursor as i64],
                        ))?;
                    }
                }
            }

            step(budget)?;
            let mut statement = sql(origin, source.prepare("SELECT sequence,typeof(sequence),CASE WHEN typeof(before_json)='text' THEN octet_length(before_json) END,CASE WHEN typeof(after_json)='text' THEN octet_length(after_json) END FROM edits WHERE project_id=?1 ORDER BY sequence"))?;
            let mut rows = sql(origin, statement.query([project_id]))?;
            loop {
                step(budget)?;
                let Some(row) = sql(origin, rows.next())? else {
                    break;
                };
                add(&mut bundle.totals.edits, 1, MAX_EDITS, "history row count")?;
                ensure(
                    sql(origin, row.get::<_, String>(1))? == "integer",
                    "Archive history sequence has an invalid type",
                )?;
                let sequence = unsigned(sql(origin, row.get(0))?, "history sequence")?;
                ensure(
                    sequence == bundle.totals.edits,
                    "Archive history sequences are not contiguous",
                )?;
                let before_length =
                    bounded_length(sql(origin, row.get(2))?, MAX_NATIVE_PROJECT_BYTES)?;
                let after_length =
                    bounded_length(sql(origin, row.get(3))?, MAX_NATIVE_PROJECT_BYTES)?;
                bundle.metadata(before_length + after_length + project_id.len() + 8)?;
                prepay(budget, before_length + after_length + project_id.len() + 8)?;
                step(budget)?;
                let (before, after): (Option<String>, Option<String>) = sql(origin, source.query_row(
                "SELECT CASE WHEN typeof(before_json)='text' AND octet_length(before_json)<=?3 THEN before_json END,CASE WHEN typeof(after_json)='text' AND octet_length(after_json)<=?3 THEN after_json END FROM edits WHERE project_id=?1 AND sequence=?2",
                params![project_id, sequence as i64, MAX_NATIVE_PROJECT_BYTES as i64], |row| Ok((row.get(0)?, row.get(1)?)),
            ))?;
                let (before, after) = (text(before)?, text(after)?);
                ensure(
                    before.len() == before_length && after.len() == after_length,
                    "Archive history metadata changed during traversal",
                )?;
                budget.visit_records(2)?;
                let before_doc = ProjectDocument::from_json(before.as_bytes())?;
                let after_doc = ProjectDocument::from_json(after.as_bytes())?;
                crate::projection_boundary::reject_opaque_promotion(&before_doc, &after_doc)?;
                ensure(
                    before_doc.id() == project_id
                        && after_doc.id() == project_id
                        && before_doc.revision().checked_add(1) == Some(after_doc.revision())
                        && after_doc.revision() <= project.revision(),
                    "Archive history identity/revision mismatch",
                )?;
                bundle.references_untrusted(&before_doc)?;
                bundle.references_untrusted(&after_doc)?;
                bundle.record(
                    b"edit",
                    &[&sequence.to_le_bytes(), before.as_bytes(), after.as_bytes()],
                );
                if let Some(target) = destination {
                    match target {
                        Destination::ArchiveExpected(connection, _) => {
                            let before_sha = checksum(before.as_bytes());
                            let after_sha = checksum(after.as_bytes());
                            target.expect(
                                "edits",
                                &[
                                    SqlCell::Text(project_id.as_bytes()),
                                    SqlCell::Integer(sequence as i64),
                                    SqlCell::Text(before.as_bytes()),
                                    SqlCell::Text(after.as_bytes()),
                                    SqlCell::Text(before_sha.as_bytes()),
                                    SqlCell::Text(after_sha.as_bytes()),
                                ],
                            )?;
                            step(budget)?;
                            target.sql(connection.execute(
                                "INSERT INTO edits VALUES(?1,?2,?3,?4,?5,?6)",
                                params![
                                    project_id,
                                    sequence as i64,
                                    before,
                                    after,
                                    before_sha,
                                    after_sha
                                ],
                            ))?;
                        }
                        Destination::Store(connection) | Destination::PlanCatalog(connection) => {
                            step(budget)?;
                            target.sql(connection.execute("INSERT INTO edits(project_id,sequence,before_json,after_json) VALUES(?1,?2,?3,?4)", params![project_id, sequence as i64, before, after]))?;
                        }
                    }
                }
            }
            ensure(
                bundle.totals.edits == history_count,
                "Archive history row count changed during traversal",
            )?;

            step(budget)?;
            let mut statement = sql(origin, source.prepare("SELECT CASE WHEN typeof(operation_id)='text' THEN octet_length(operation_id) END,CASE WHEN typeof(request_json)='text' THEN octet_length(request_json) END,CASE WHEN typeof(result_json)='text' THEN octet_length(result_json) END,CASE WHEN typeof(request_sha256)='text' THEN octet_length(request_sha256) END,CASE WHEN typeof(result_sha256)='text' THEN octet_length(result_sha256) END,typeof(previous_revision),typeof(committed_revision),CASE WHEN typeof(operation_id)='text' AND octet_length(operation_id)<=96 THEN operation_id END FROM operations WHERE project_id=?1 ORDER BY operation_id"))?;
            let mut rows = sql(origin, statement.query([project_id]))?;
            loop {
                step(budget)?;
                let Some(row) = sql(origin, rows.next())? else {
                    break;
                };
                add(
                    &mut bundle.totals.operations,
                    1,
                    MAX_OPERATIONS,
                    "operation row count",
                )?;
                let id_length = bounded_length(sql(origin, row.get(0))?, MAX_ID_BYTES)?;
                let request_length = bounded_length(sql(origin, row.get(1))?, WORKER_MAX_BYTES)?;
                let result_length =
                    bounded_length(sql(origin, row.get(2))?, MAX_NATIVE_PROJECT_BYTES)?;
                ensure(
                    bounded_length(sql(origin, row.get(3))?, 64)? == 64
                        && bounded_length(sql(origin, row.get(4))?, 64)? == 64,
                    "Archive operation digest has an invalid size",
                )?;
                ensure(
                    sql(origin, row.get::<_, String>(5))? == "integer"
                        && sql(origin, row.get::<_, String>(6))? == "integer",
                    "Archive operation revision has an invalid type",
                )?;
                // Only this <=96-byte key is materialized in the length cursor;
                // fixed work was prepaid before advancing it.
                let id = text(sql(origin, row.get(7))?)?;
                ensure(
                    id.len() == id_length,
                    "Archive operation identity length differs",
                )?;
                bundle
                    .metadata(request_length + result_length + id.len() + project_id.len() + 144)?;
                prepay(
                    budget,
                    request_length + result_length + id.len() + project_id.len() + 144,
                )?;
                step(budget)?;
                let (request, request_sha, previous, committed, result, result_sha): (Option<String>, Option<String>, i64, i64, Option<String>, Option<String>) = sql(origin, source.query_row(
                "SELECT CASE WHEN typeof(request_json)='text' AND octet_length(request_json)<=?3 THEN request_json END,CASE WHEN typeof(request_sha256)='text' AND octet_length(request_sha256)=64 THEN request_sha256 END,previous_revision,committed_revision,CASE WHEN typeof(result_json)='text' AND octet_length(result_json)<=?4 THEN result_json END,CASE WHEN typeof(result_sha256)='text' AND octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE operation_id=?1 AND project_id=?2",
                params![id, project_id, WORKER_MAX_BYTES as i64, MAX_NATIVE_PROJECT_BYTES as i64], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
            ))?;
                let (request, request_sha, result, result_sha) = (
                    text(request)?,
                    text(request_sha)?,
                    text(result)?,
                    text(result_sha)?,
                );
                let previous = unsigned(previous, "previous revision")?;
                let committed = unsigned(committed, "committed revision")?;
                ensure(
                    request.len() == request_length
                        && result.len() == result_length
                        && checksum(request.as_bytes()) == request_sha
                        && checksum(result.as_bytes()) == result_sha,
                    "Archived operation checksum mismatch",
                )?;
                budget.visit_records(1)?;
                let document = ProjectDocument::from_json(result.as_bytes())?;
                ensure(
                    document.id() == project_id
                        && document.revision() == committed
                        && previous.checked_add(1) == Some(committed)
                        && committed <= project.revision(),
                    "Archived operation result identity/revision mismatch",
                )?;
                validate_request_inner(
                    &request,
                    &id,
                    project_id,
                    previous,
                    &document,
                    Some(&result_sha),
                    budget,
                )?;
                bundle.references_untrusted(&document)?;
                bundle.record(
                    b"operation",
                    &[
                        id.as_bytes(),
                        request.as_bytes(),
                        request_sha.as_bytes(),
                        &previous.to_le_bytes(),
                        &committed.to_le_bytes(),
                        result.as_bytes(),
                        result_sha.as_bytes(),
                    ],
                );
                if let Some(target) = destination {
                    if matches!(target, Destination::Store(_) | Destination::PlanCatalog(_)) {
                        crate::source_attachment::reject_reserved_operation(&id)?;
                    }
                    target.expect(
                        "operations",
                        &[
                            SqlCell::Text(id.as_bytes()),
                            SqlCell::Text(project_id.as_bytes()),
                            SqlCell::Text(request.as_bytes()),
                            SqlCell::Text(request_sha.as_bytes()),
                            SqlCell::Integer(previous as i64),
                            SqlCell::Integer(committed as i64),
                            SqlCell::Text(result.as_bytes()),
                            SqlCell::Text(result_sha.as_bytes()),
                        ],
                    )?;
                    step(budget)?;
                    target.sql(target.connection().execute(
                        "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![
                            id,
                            project_id,
                            request,
                            request_sha,
                            previous as i64,
                            committed as i64,
                            result,
                            result_sha
                        ],
                    ))?;
                }
            }
            ensure(
                bundle.totals.operations == operation_count,
                "Archive operation row count changed during traversal",
            )?;

            step(budget)?;
            let import_lengths: Option<(Option<i64>, Option<i64>)> = sql(origin, source.query_row(
            "SELECT CASE WHEN typeof(source_sha256)='text' THEN octet_length(source_sha256) END,CASE WHEN typeof(original_json)='blob' THEN octet_length(original_json) END FROM imports WHERE project_id=?1",
            [project_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional())?;
            ensure(
                u64::from(import_lengths.is_some()) == import_count,
                "Archive import row count changed during traversal",
            )?;
            if let Some((sha_length, bytes_length)) = import_lengths {
                ensure(
                    bounded_length(sha_length, 64)? == 64,
                    "Archive import digest has an invalid size",
                )?;
                let bytes_length = bounded_length(bytes_length, MAX_PROJECT_BYTES)?;
                bundle.metadata(bytes_length + 64 + project_id.len())?;
                prepay(budget, bytes_length + 64 + project_id.len())?;
                step(budget)?;
                let (sha, bytes): (Option<String>, Option<Vec<u8>>) = sql(origin, source.query_row(
                "SELECT CASE WHEN typeof(source_sha256)='text' AND octet_length(source_sha256)=64 THEN source_sha256 END,CASE WHEN typeof(original_json)='blob' AND octet_length(original_json)<=?2 THEN original_json END FROM imports WHERE project_id=?1",
                params![project_id, MAX_PROJECT_BYTES as i64], |row| Ok((row.get(0)?, row.get(1)?)),
            ))?;
                let sha = text(sha)?;
                let bytes = bytes
                    .ok_or_else(|| invalid("Archive original import exceeds its byte limit"))?;
                ensure(
                    bytes.len() == bytes_length && checksum(&bytes) == sha,
                    "Archived original import checksum mismatch",
                )?;
                budget.visit_records(1)?;
                let original = ProjectDocument::from_json(&bytes)?;
                ensure(
                    original.schema_version() == 1,
                    "Original legacy import must use musical schema 1",
                )?;
                bundle.references_untrusted(&original)?;
                bundle.totals.imports = 1;
                bundle.record(b"import", &[sha.as_bytes(), &bytes]);
                if let Some(target) = destination {
                    target.expect(
                        "imports",
                        &[
                            SqlCell::Text(project_id.as_bytes()),
                            SqlCell::Text(sha.as_bytes()),
                            SqlCell::Blob(&bytes),
                        ],
                    )?;
                    step(budget)?;
                    target.sql(target.connection().execute(
                        "INSERT INTO imports VALUES(?1,?2,?3)",
                        params![project_id, sha, bytes],
                    ))?;
                }
            }
            Ok(bundle)
        })(),
    )
}

/// Read a sealed take registration and its exact canonical manifest together.
/// No owned pathname is opened and no retained recording is authenticated.
pub(crate) fn read_take_metadata(
    connection: &Connection,
    id: &str,
    origin: ArchiveSqlOrigin,
    budget: &InspectionBudget,
) -> Result<(TakeObject, String)> {
    type RegistrationLengths = (Option<i64>, Option<i64>, Option<i64>, String);
    finish(
        budget,
        (|| {
            ensure(
                id.len() <= MAX_ID_BYTES,
                "Archive take identity exceeds its limit",
            )?;
            prepay(budget, id.len())?;
            let take_id = TakeId(id.into());
            take_id
                .validate()
                .map_err(|error| invalid(error.to_string()))?;
            step(budget)?;
            let lengths: Option<RegistrationLengths> = sql(origin, connection.query_row(
            "SELECT CASE WHEN typeof(manifest_sha256)='text' THEN octet_length(manifest_sha256) END,CASE WHEN typeof(object_sha256)='text' THEN octet_length(object_sha256) END,CASE WHEN typeof(manifest_json)='text' THEN octet_length(manifest_json) END,typeof(byte_length) FROM native_takes WHERE take_id=?1",
            [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).optional())?;
            let (manifest_sha_length, object_sha_length, manifest_length, byte_type) =
                lengths.ok_or_else(|| invalid(format!("Native take is not registered: {id}")))?;
            ensure(
                bounded_length(manifest_sha_length, 64)? == 64
                    && bounded_length(object_sha_length, 64)? == 64
                    && byte_type == "integer",
                "Archive take registration has an invalid type or identity",
            )?;
            let manifest_length = bounded_length(manifest_length, MAX_MANIFEST_BYTES)?;
            prepay(budget, manifest_length + id.len() + 136)?;
            step(budget)?;
            let (manifest_sha256, object_sha256, byte_length, json): (Option<String>, Option<String>, i64, Option<String>) = sql(origin, connection.query_row(
            "SELECT CASE WHEN typeof(manifest_sha256)='text' AND octet_length(manifest_sha256)=64 THEN manifest_sha256 END,CASE WHEN typeof(object_sha256)='text' AND octet_length(object_sha256)=64 THEN object_sha256 END,byte_length,CASE WHEN typeof(manifest_json)='text' AND octet_length(manifest_json)<=?2 THEN manifest_json END FROM native_takes WHERE take_id=?1",
            params![id, MAX_MANIFEST_BYTES as i64], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ))?;
            let json = text(json)?;
            ensure(
                json.len() == manifest_length,
                "Archive take metadata changed during traversal",
            )?;
            budget.visit_records(1)?;
            let manifest: SealedManifest = serde_json::from_str(&json)
                .map_err(|error| invalid(format!("Invalid registered manifest: {error}")))?;
            let object = TakeObject {
                take_id,
                manifest_sha256: text(manifest_sha256)?,
                object_sha256: text(object_sha256)?,
                byte_length: unsigned(byte_length, "take byte length")?,
                manifest,
            };
            validate_object(&object)?;
            ensure(
                object.manifest.canonical_bytes()? == json.as_bytes(),
                "Registered manifest is not canonical",
            )?;
            ensure(
                digest_valid(&object.object_sha256)
                    && object.byte_length > 0
                    && object.byte_length <= archive::ARCHIVE_PAYLOAD_BYTES,
                "Invalid archive take object identity/size",
            )?;
            Ok((object, json))
        })(),
    )
}

/// Read only a bounded schema-1 asset BLOB, with its stored type/length checked
/// and its complete materialization/hash work credited before requesting it.
pub(crate) fn read_asset(
    connection: &Connection,
    id: &str,
    origin: ArchiveSqlOrigin,
    budget: &InspectionBudget,
) -> Result<Vec<u8>> {
    finish(
        budget,
        (|| {
            ensure(
                omniusong_core::valid_asset_id(id),
                "Invalid archive audio asset identity",
            )?;
            step(budget)?;
            let length: Option<Option<i64>> = sql(origin, connection.query_row(
            "SELECT CASE WHEN typeof(payload)='blob' THEN octet_length(payload) END FROM assets WHERE id=?1",
            [id], |row| row.get(0),
        ).optional())?;
            let length = bounded_length(length.flatten(), crate::MAX_ASSET_BYTES)?;
            ensure(length > 0, "Archive audio asset is empty")?;
            prepay(budget, length + id.len())?;
            step(budget)?;
            let bytes: Option<Vec<u8>> = sql(origin, connection.query_row(
            "SELECT CASE WHEN typeof(payload)='blob' AND octet_length(payload)<=?2 THEN payload END FROM assets WHERE id=?1",
            params![id, crate::MAX_ASSET_BYTES as i64], |row| row.get(0),
        ))?;
            let bytes = bytes.ok_or_else(|| {
                invalid("Project audio is missing or exceeds the schema-1 size limit")
            })?;
            ensure(
                bytes.len() == length,
                "Archive audio asset changed during traversal",
            )?;
            crate::check_asset(id, &bytes)?;
            Ok(bytes)
        })(),
    )
}

#[cfg(test)]
mod tests;

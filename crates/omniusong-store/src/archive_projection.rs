//! Live-catalog source/publication/history closure diagnostic only.
//! The caller holds its real CatalogTransaction::deferred and retained-file
//! lease throughout. This grants no private-archive SQL error-domain, schema,
//! index, export, restore, or publication authority.
use crate::{
    Result, StoreError, archive,
    archive_operation::ArchiveOperation,
    checksum, projection_publication,
    projection_sources::{ProjectionProofs, RegisteredProjectionSources, RegisteredSourceSet},
};
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, Validate, WORKER_MAX_BYTES},
    native::MAX_NATIVE_PROJECT_BYTES,
    performance_projection::PerformanceProjectionBinding,
};
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
};

const MAX_HISTORY: u64 = 10_000;
const MAX_OPERATIONS: u64 = 100_000;
const MAX_OBJECTS: usize = 4096;
const MAX_PROOFS: usize = 256;
const MAX_PROOF_BYTES: usize = MAX_NATIVE_PROJECT_BYTES + 32 * 1024;
const MAX_PROOF_MAPPINGS: u64 = 100_000;
const MAX_MAPPINGS: u64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedProjectionClosureInspection {
    pub version: u32,
    pub project_id: String,
    pub snapshots: u64,
    pub history_rows: u64,
    pub operation_rows: u64,
    pub lineage_proof_rows: u64,
    pub distinct_bindings: u64,
}
fn invalid(message: impl ToString) -> StoreError {
    StoreError::Invalid(message.to_string())
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(invalid(message)) }
}
fn finish<T>(budget: &InspectionBudget, result: Result<T>) -> Result<T> {
    // Only a real live catalog is accepted by this entry point. Preserve its
    // actual fatal domain even if cancellation arrives after SQLite failed.
    if matches!(&result,Err(StoreError::Database(error)) if error.sqlite_error().is_some_and(|c| matches!(c.extended_code & 0xff,10|11|26)))
    {
        return result;
    }
    budget.checkpoint()?;
    result
}
fn step(budget: &InspectionBudget) -> Result<()> {
    // Prepaid before advancing rows or materializing their CASE-bounded fixed
    // fields: a mapping has at most 448 raw UTF-8 bytes plus bounded ID copies.
    // Variable documents/requests/proofs receive their separate length credit.
    budget.visit_rows(1)?;
    budget.charge_work(4096)?;
    Ok(())
}
fn bounded_length(value: i64, maximum: usize) -> Result<usize> {
    usize::try_from(value)
        .ok()
        .filter(|v| *v <= maximum)
        .ok_or_else(|| invalid("Retained closure row exceeds its metadata bounds"))
}
fn optional_text(value: Option<String>) -> Result<String> {
    value.ok_or_else(|| invalid("Retained closure row is missing or exceeds its metadata bounds"))
}
struct BoundedEncoding<'b> {
    budget: &'b InspectionBudget,
    bytes: Vec<u8>,
    maximum: usize,
}
impl Write for BoundedEncoding<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "Retained closure encoding exceeds its byte limit",
            ));
        }
        self.budget
            .charge_work((bytes.len() as u64).saturating_mul(2))
            .map_err(|_| std::io::Error::other("Retained closure governor stopped encoding"))?;
        if bytes.len() > self.bytes.capacity().saturating_sub(self.bytes.len()) {
            let required = self.bytes.len() + bytes.len();
            let capacity = required.max(
                self.bytes
                    .capacity()
                    .saturating_mul(2)
                    .max(256)
                    .min(self.maximum),
            );
            // Geometric growth avoids repeated whole-buffer copying for highly
            // escaped strings. Prepay the new allocation and existing copy.
            self.budget
                .charge_work(capacity as u64 + self.bytes.len() as u64)
                .map_err(|_| {
                    std::io::Error::other("Retained closure governor stopped allocation")
                })?;
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|_| std::io::Error::other("Cannot reserve retained closure encoding"))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize, maximum: usize, budget: &InspectionBudget) -> Result<Vec<u8>> {
    let mut output = BoundedEncoding {
        budget,
        bytes: Vec::new(),
        maximum,
    };
    let result = serde_json::to_writer(&mut output, value);
    budget.checkpoint()?;
    result.map_err(invalid)?;
    Ok(output.bytes)
}

#[derive(Debug, PartialEq, Eq)]
struct WalkResult {
    snapshots: u64,
    history: u64,
    operations: u64,
    proofs: u64,
    metadata: u64,
    sha256: String,
}
struct Scan<'b> {
    budget: &'b InspectionBudget,
    metadata: u64,
    snapshots: u64,
    hash: Sha256,
}
impl<'b> Scan<'b> {
    fn new(budget: &'b InspectionBudget) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.retained-projection-closure.v1\0");
        Self {
            budget,
            metadata: 0,
            snapshots: 0,
            hash,
        }
    }
    fn prepay(&mut self, length: i64, maximum: usize) -> Result<usize> {
        let length = bounded_length(length, maximum)?;
        archive::add(
            &mut self.metadata,
            length as u64,
            archive::ARCHIVE_METADATA_BYTES,
            "closure metadata",
        )?;
        self.budget
            .charge_work((length as u64).saturating_mul(64).saturating_add(4096))?;
        Ok(length)
    }
    fn record(&mut self, kind: &[u8], fields: &[&[u8]]) {
        self.hash.update((kind.len() as u64).to_le_bytes());
        self.hash.update(kind);
        self.hash.update((fields.len() as u64).to_le_bytes());
        for field in fields {
            self.hash.update((field.len() as u64).to_le_bytes());
            self.hash.update(field);
        }
    }
    fn document(&mut self, document: &ProjectDocument, visitor: &mut impl Visitor) -> Result<()> {
        self.budget.visit_records(1)?;
        self.snapshots = self
            .snapshots
            .checked_add(1)
            .ok_or_else(|| invalid("Retained snapshot count overflow"))?;
        visitor.document(document)
    }
}
trait Visitor {
    fn document(&mut self, document: &ProjectDocument) -> Result<()>;
    fn forward(
        &mut self,
        _connection: &Connection,
        _before: &ProjectDocument,
        _after: &ProjectDocument,
    ) -> Result<()> {
        Ok(())
    }
    fn operation(
        &mut self,
        _operation: &ArchiveOperation,
        _request: &str,
        _result_sha: &str,
        _document: &ProjectDocument,
    ) -> Result<()> {
        Ok(())
    }
}
struct Collect<'b> {
    set: RegisteredSourceSet<'b>,
    takes: BTreeSet<omniusong_core::contracts::TakeId>,
    assets: BTreeSet<String>,
    budget: &'b InspectionBudget,
}
impl Visitor for Collect<'_> {
    fn document(&mut self, document: &ProjectDocument) -> Result<()> {
        self.set.add_document(document)?;
        for reference in document.referenced_takes()? {
            self.budget.charge_work(1024)?;
            self.takes.insert(reference.take_id);
            require(
                self.takes.len() <= MAX_OBJECTS,
                "Retained closure exceeds its distinct original limit",
            )?;
        }
        for id in document.referenced_assets()? {
            self.budget.charge_work(1024)?;
            self.assets.insert(id);
            require(
                self.assets.len() <= MAX_OBJECTS,
                "Retained closure exceeds its distinct audio asset limit",
            )?;
        }
        Ok(())
    }
}
struct ValidateClosure<'p, 'a> {
    proofs: &'p ProjectionProofs<'a>,
    budget: &'a InspectionBudget,
    bindings: BTreeMap<String, Vec<u8>>,
    memo_bytes: u64,
}
impl Visitor for ValidateClosure<'_, '_> {
    fn document(&mut self, document: &ProjectDocument) -> Result<()> {
        self.proofs.validate_document_geometry(document)?;
        if let Some(registry) = document.performance_projections()? {
            for binding in registry.entries {
                let bytes = encode(&binding, MAX_NATIVE_PROJECT_BYTES, self.budget)?;
                if let Some(previous) = self.bindings.get(&binding.id) {
                    require(
                        previous == &bytes,
                        "Retained snapshots reuse a projection identity with different original binding",
                    )?;
                } else {
                    self.budget.visit_groups(1)?;
                    archive::add(
                        &mut self.memo_bytes,
                        bytes.len() as u64,
                        archive::ARCHIVE_METADATA_BYTES,
                        "binding memo bytes",
                    )?;
                    self.proofs
                        .performance(&binding.source)?
                        .validate_binding(&binding)?;
                    self.budget.charge_work(binding.id.len() as u64 + 1024)?;
                    self.bindings.insert(binding.id, bytes);
                }
            }
        }
        self.budget.checkpoint()?;
        Ok(())
    }
    fn forward(
        &mut self,
        connection: &Connection,
        before: &ProjectDocument,
        after: &ProjectDocument,
    ) -> Result<()> {
        projection_publication::validate_forward_transition(connection, before, after, self.budget)
    }
    fn operation(
        &mut self,
        operation: &ArchiveOperation,
        request: &str,
        result_sha: &str,
        document: &ProjectDocument,
    ) -> Result<()> {
        if let ArchiveOperation::Projection(intent) = operation {
            projection_publication::validate_publication_result(
                intent,
                request,
                document,
                result_sha,
                self.budget,
            )?;
        }
        Ok(())
    }
}

fn count(
    connection: &Connection,
    sql: &str,
    project: &str,
    maximum: u64,
    budget: &InspectionBudget,
) -> Result<u64> {
    step(budget)?;
    let count = archive::unsigned(
        connection.query_row(sql, [project], |r| r.get(0))?,
        "closure row count",
    )?;
    require(
        count <= maximum,
        "Retained closure row count exceeds its explicit bound",
    )?;
    Ok(count)
}
fn walk(
    connection: &Connection,
    project: &str,
    budget: &InspectionBudget,
    visitor: &mut impl Visitor,
) -> Result<WalkResult> {
    let mut scan = Scan::new(budget);
    step(budget)?;
    let (length, revision, cursor): (i64, i64, i64) = connection.query_row(
        "SELECT octet_length(payload),revision,cursor FROM projects WHERE id=?1",
        [project],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    scan.prepay(length, MAX_NATIVE_PROJECT_BYTES)?;
    scan.prepay(project.len() as i64 + 16, 112)?;
    step(budget)?;
    let payload: Option<String> = connection.query_row(
        "SELECT CASE WHEN octet_length(payload)<=?2 THEN payload END FROM projects WHERE id=?1",
        params![project, MAX_NATIVE_PROJECT_BYTES as i64],
        |r| r.get(0),
    )?;
    let payload = optional_text(payload)?;
    let current = ProjectDocument::from_json(payload.as_bytes())?;
    let revision = archive::unsigned(revision, "project revision")?;
    let cursor = archive::unsigned(cursor, "project cursor")?;
    require(
        current.id() == project && current.revision() == revision,
        "Retained current project identity/revision differs",
    )?;
    scan.record(
        b"current",
        &[
            project.as_bytes(),
            payload.as_bytes(),
            &revision.to_le_bytes(),
            &cursor.to_le_bytes(),
        ],
    );
    scan.document(&current, visitor)?;
    drop(current);
    drop(payload);

    let history = count(
        connection,
        "SELECT count(*) FROM edits WHERE project_id=?1",
        project,
        MAX_HISTORY,
        budget,
    )?;
    require(
        cursor <= history,
        "Retained cursor exceeds its complete history",
    )?;
    let mut statement = connection.prepare("SELECT sequence,octet_length(before_json),octet_length(after_json) FROM edits WHERE project_id=?1 ORDER BY sequence")?;
    let mut rows = statement.query([project])?;
    let mut seen = 0;
    loop {
        step(budget)?;
        let Some(row) = rows.next()? else {
            break;
        };
        archive::add(&mut seen, 1, MAX_HISTORY, "closure history rows")?;
        let sequence = archive::unsigned(row.get(0)?, "history sequence")?;
        require(
            sequence == seen,
            "Retained history sequences are not contiguous",
        )?;
        scan.prepay(row.get(1)?, MAX_NATIVE_PROJECT_BYTES)?;
        scan.prepay(row.get(2)?, MAX_NATIVE_PROJECT_BYTES)?;
        scan.prepay(project.len() as i64 + 8, 104)?;
        step(budget)?;
        let (before,after): (Option<String>,Option<String>) = connection.query_row("SELECT CASE WHEN octet_length(before_json)<=?3 THEN before_json END,CASE WHEN octet_length(after_json)<=?3 THEN after_json END FROM edits WHERE project_id=?1 AND sequence=?2",params![project,sequence as i64,MAX_NATIVE_PROJECT_BYTES as i64],|r| Ok((r.get(0)?,r.get(1)?)))?;
        let (before, after) = (optional_text(before)?, optional_text(after)?);
        let before_doc = ProjectDocument::from_json(before.as_bytes())?;
        let after_doc = ProjectDocument::from_json(after.as_bytes())?;
        require(
            before_doc.id() == project
                && after_doc.id() == project
                && before_doc.revision().checked_add(1) == Some(after_doc.revision())
                && after_doc.revision() <= revision,
            "Retained forward history identity/revision differs",
        )?;
        scan.record(
            b"history",
            &[&sequence.to_le_bytes(), before.as_bytes(), after.as_bytes()],
        );
        scan.document(&before_doc, visitor)?;
        scan.document(&after_doc, visitor)?;
        visitor.forward(connection, &before_doc, &after_doc)?;
    }
    require(seen == history, "Retained history changed during its scan")?;
    drop(rows);
    drop(statement);

    let operations = count(
        connection,
        "SELECT count(*) FROM operations WHERE project_id=?1",
        project,
        MAX_OPERATIONS,
        budget,
    )?;
    let mut statement = connection.prepare("SELECT CASE WHEN octet_length(operation_id)<=96 THEN operation_id END,octet_length(request_json),octet_length(result_json),previous_revision,committed_revision,CASE WHEN octet_length(request_sha256)=64 THEN request_sha256 END,CASE WHEN octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE project_id=?1 ORDER BY operation_id")?;
    let mut rows = statement.query([project])?;
    let mut seen = 0;
    loop {
        step(budget)?;
        let Some(row) = rows.next()? else {
            break;
        };
        archive::add(&mut seen, 1, MAX_OPERATIONS, "closure operation rows")?;
        let id = optional_text(row.get(0)?)?;
        let request_length = scan.prepay(row.get(1)?, WORKER_MAX_BYTES)?;
        let result_length = scan.prepay(row.get(2)?, MAX_NATIVE_PROJECT_BYTES)?;
        scan.prepay((id.len() + project.len() + 144) as i64, 336)?;
        let previous = archive::unsigned(row.get(3)?, "operation previous revision")?;
        let committed = archive::unsigned(row.get(4)?, "operation committed revision")?;
        let request_sha = optional_text(row.get(5)?)?;
        let result_sha = optional_text(row.get(6)?)?;
        require(
            archive::digest_valid(&request_sha) && archive::digest_valid(&result_sha),
            "Retained operation digest is malformed",
        )?;
        step(budget)?;
        let (request,result):(Option<String>,Option<String>) = connection.query_row("SELECT CASE WHEN octet_length(request_json)<=?3 THEN request_json END,CASE WHEN octet_length(result_json)<=?4 THEN result_json END FROM operations WHERE operation_id=?1 AND project_id=?2",params![id,project,WORKER_MAX_BYTES as i64,MAX_NATIVE_PROJECT_BYTES as i64],|r| Ok((r.get(0)?,r.get(1)?)))?;
        let (request, result) = (optional_text(request)?, optional_text(result)?);
        require(
            request.len() == request_length
                && result.len() == result_length
                && checksum(request.as_bytes()) == request_sha
                && checksum(result.as_bytes()) == result_sha,
            "Retained operation bytes/hash differ",
        )?;
        let document = ProjectDocument::from_json(result.as_bytes())?;
        require(
            document.to_json()? == result.as_bytes()
                && document.id() == project
                && document.revision() == committed
                && previous.checked_add(1) == Some(committed)
                && committed <= revision,
            "Retained operation result is not its canonical identity/revision snapshot",
        )?;
        let operation = ArchiveOperation::parse(&request)?;
        operation.validate_identity(&id, project, previous)?;
        if !matches!(operation, ArchiveOperation::Projection(_)) {
            archive::validate_request(&request, &id, project, previous, &document)?;
        }
        scan.record(
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
        scan.document(&document, visitor)?;
        visitor.operation(&operation, &request, &result_sha, &document)?;
    }
    require(
        seen == operations,
        "Retained operations changed during their scan",
    )?;
    drop(rows);
    drop(statement);
    let proofs = walk_lineage(connection, project, revision, &mut scan, visitor)?;
    budget.charge_work(64)?;
    Ok(WalkResult {
        snapshots: scan.snapshots,
        history,
        operations,
        proofs,
        metadata: scan.metadata,
        sha256: format!("{:x}", scan.hash.finalize()),
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentProof {
    version: u32,
    operation_id: OperationId,
    source_archive_sha256: String,
    source_archive_byte_length: u64,
    source_project_id: String,
    destination_project_id: String,
    mapping_count: u64,
    mapping_sha256: String,
    ancestors: Vec<String>,
    result: Value,
    result_sha256: String,
}
struct ParentInfo {
    source: String,
    destination: String,
    ancestors: Vec<String>,
}

fn walk_lineage(
    connection: &Connection,
    project: &str,
    revision: u64,
    scan: &mut Scan<'_>,
    visitor: &mut impl Visitor,
) -> Result<u64> {
    let budget = scan.budget;
    let count = count(
        connection,
        "SELECT count(*) FROM archive_project_lineage WHERE project_id=?1",
        project,
        MAX_PROOFS as u64,
        budget,
    )?;
    let mut ids = BTreeSet::new();
    let mut statement=connection.prepare("SELECT CASE WHEN octet_length(proof_sha256)=64 THEN proof_sha256 END FROM archive_project_lineage WHERE project_id=?1 ORDER BY proof_sha256")?;
    let mut rows = statement.query([project])?;
    loop {
        step(budget)?;
        let Some(row) = rows.next()? else {
            break;
        };
        require(
            ids.len() < MAX_PROOFS,
            "Retained lineage exceeds its proof ceiling",
        )?;
        let id = optional_text(row.get(0)?)?;
        require(
            archive::digest_valid(&id),
            "Invalid retained lineage identity",
        )?;
        ids.insert(id);
    }
    require(
        ids.len() as u64 == count,
        "Retained lineage associations are not unique",
    )?;
    drop(rows);
    drop(statement);
    step(budget)?;
    let live:Option<(Option<String>,Option<String>)>=connection.query_row("SELECT CASE WHEN octet_length(operation_id)<=96 THEN operation_id END,CASE WHEN octet_length(proof_sha256)=64 THEN proof_sha256 END FROM archive_copies WHERE project_id=?1",[project],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let live: Option<(String, String)> = live
        .map(|(id, sha)| -> Result<_> { Ok((optional_text(id)?, optional_text(sha)?)) })
        .transpose()?;
    require(
        ids.is_empty() == live.is_none(),
        "Retained copy admission and lineage are incomplete",
    )?;
    if let Some((id, sha)) = &live {
        require(
            ids.contains(sha),
            "Retained active copy proof is not associated",
        )?;
        OperationId(id.clone()).validate().map_err(invalid)?;
        step(budget)?;
        let collision: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
            [id],
            |r| r.get(0),
        )?;
        require(
            !collision,
            "Retained copy admission collides with an edit operation",
        )?;
        scan.prepay((id.len() + sha.len() + project.len()) as i64, 256)?;
        scan.record(
            b"copy-admission",
            &[id.as_bytes(), project.as_bytes(), sha.as_bytes()],
        );
    }
    let mut parents = BTreeMap::new();
    let mut total_mappings = 0;
    for sha in &ids {
        step(budget)?;
        let length: i64 = connection.query_row(
            "SELECT octet_length(receipt_json) FROM archive_copy_proofs WHERE proof_sha256=?1",
            [sha],
            |r| r.get(0),
        )?;
        scan.prepay(length, MAX_PROOF_BYTES)?;
        scan.prepay((sha.len() + project.len()) as i64, 160)?;
        step(budget)?;
        let raw:Option<String>=connection.query_row("SELECT CASE WHEN octet_length(receipt_json)<=?2 THEN receipt_json END FROM archive_copy_proofs WHERE proof_sha256=?1",params![sha,MAX_PROOF_BYTES as i64],|r|r.get(0))?;
        let raw = optional_text(raw)?;
        require(
            checksum(raw.as_bytes()) == *sha,
            "Retained parent proof checksum differs",
        )?;
        let proof: ParentProof = serde_json::from_str(&raw).map_err(invalid)?;
        require(
            encode(&proof, MAX_PROOF_BYTES, budget)? == raw.as_bytes(),
            "Retained parent proof is noncanonical",
        )?;
        proof.operation_id.validate().map_err(invalid)?;
        ProjectId(proof.source_project_id.clone())
            .validate()
            .map_err(invalid)?;
        ProjectId(proof.destination_project_id.clone())
            .validate()
            .map_err(invalid)?;
        require(
            proof.version == 1
                && archive::digest_valid(&proof.source_archive_sha256)
                && proof.source_archive_byte_length > 0
                && proof.source_archive_byte_length <= archive::MAX_ARCHIVE_FILE_BYTES
                && proof.source_project_id != proof.destination_project_id
                && proof.mapping_count <= MAX_PROOF_MAPPINGS
                && archive::digest_valid(&proof.mapping_sha256)
                && archive::digest_valid(&proof.result_sha256)
                && proof.ancestors.len() < MAX_PROOFS
                && proof.ancestors.windows(2).all(|p| p[0] < p[1])
                && proof.ancestors.iter().all(|p| p != sha && ids.contains(p)),
            "Invalid retained parent proof envelope/linkage",
        )?;
        let is_live = live.as_ref().is_some_and(|(_, id)| id == sha);
        if is_live {
            require(
                live.as_ref().unwrap().0 == proof.operation_id.0
                    && proof.destination_project_id == project
                    && proof.ancestors.iter().collect::<BTreeSet<_>>()
                        == ids.iter().filter(|p| *p != sha).collect(),
                "Retained active copy does not own complete ancestry",
            )?;
        }
        scan.record(b"parent-proof", &[sha.as_bytes(), raw.as_bytes()]);
        let document = ProjectDocument::from_value(proof.result)?;
        require(
            document.id() == proof.destination_project_id
                && checksum(&document.to_json()?) == proof.result_sha256
                && (!is_live || document.revision() <= revision),
            "Retained parent result identity/hash differs",
        )?;
        scan.document(&document, visitor)?;
        walk_mappings(
            connection,
            project,
            sha,
            (proof.mapping_count, &proof.mapping_sha256),
            is_live,
            &mut total_mappings,
            scan,
        )?;
        parents.insert(
            sha.clone(),
            ParentInfo {
                source: proof.source_project_id,
                destination: proof.destination_project_id,
                ancestors: proof.ancestors,
            },
        );
    }
    for (sha, info) in &parents {
        for parent in &info.ancestors {
            // Covers bounded map lookup and binary search on 64-byte hashes.
            budget.charge_work(4096)?;
            let prior = &parents[parent];
            require(
                prior.ancestors.binary_search(sha).is_err()
                    && sorted_subset(&prior.ancestors, &info.ancestors, budget)?,
                "Retained ancestry is cyclic or not transitively closed",
            )?;
        }
        if info.ancestors.is_empty() {
            continue;
        }
        let mut immediate = None;
        for parent in &info.ancestors {
            budget.charge_work(2048)?;
            if parents[parent].destination == info.source {
                require(
                    immediate.is_none(),
                    "Retained ancestry has no unique source-project edge",
                )?;
                immediate = Some(parent);
            }
        }
        let parent = immediate
            .ok_or_else(|| invalid("Retained ancestry has no unique source-project edge"))?;
        budget.charge_work(
            ((parents[parent].ancestors.len() + info.ancestors.len()) as u64).saturating_mul(256)
                + 4096,
        )?;
        require(
            parents[parent]
                .ancestors
                .iter()
                .eq(info.ancestors.iter().filter(|p| *p != parent)),
            "Retained ancestry lost the complete source lineage",
        )?;
        step(budget)?;
        let broken:i64=connection.query_row("SELECT count(*) FROM archive_copy_proof_operations p LEFT JOIN archive_copy_proof_operations c ON c.proof_sha256=?1 AND c.source_operation_id=p.destination_operation_id WHERE p.proof_sha256=?2 AND (c.source_operation_id IS NULL OR c.source_request_sha256!=p.destination_request_sha256 OR c.source_result_sha256!=p.destination_result_sha256)",params![sha,parent],|r|r.get(0))?;
        require(
            broken == 0,
            "Retained ancestry mapping lost a prior copied operation",
        )?;
    }
    Ok(count)
}

/// Both inputs are strictly sorted, validated 64-byte proof hashes. Merge
/// comparison avoids repeated linear membership scans through dense ancestry;
/// each comparison prepays its bounded string work and observes sticky stops.
fn sorted_subset(
    required: &[String],
    available: &[String],
    budget: &InspectionBudget,
) -> Result<bool> {
    let mut next = 0;
    for id in required {
        loop {
            budget.charge_work(256)?;
            let Some(candidate) = available.get(next) else {
                return Ok(false);
            };
            match candidate.cmp(id) {
                std::cmp::Ordering::Less => next += 1,
                std::cmp::Ordering::Equal => {
                    next += 1;
                    break;
                }
                std::cmp::Ordering::Greater => return Ok(false),
            }
        }
    }
    budget.checkpoint()?;
    Ok(true)
}

fn walk_mappings(
    connection: &Connection,
    project: &str,
    sha: &str,
    expected: (u64, &str),
    is_live: bool,
    total: &mut u64,
    scan: &mut Scan<'_>,
) -> Result<()> {
    let (expected, expected_sha) = expected;
    let budget = scan.budget;
    let mut statement=connection.prepare("SELECT CASE WHEN octet_length(source_operation_id)<=96 THEN source_operation_id END,CASE WHEN octet_length(destination_operation_id)<=96 THEN destination_operation_id END,CASE WHEN octet_length(source_request_sha256)=64 THEN source_request_sha256 END,CASE WHEN octet_length(source_result_sha256)=64 THEN source_result_sha256 END,CASE WHEN octet_length(destination_request_sha256)=64 THEN destination_request_sha256 END,CASE WHEN octet_length(destination_result_sha256)=64 THEN destination_result_sha256 END FROM archive_copy_proof_operations WHERE proof_sha256=?1 ORDER BY source_operation_id")?;
    let mut rows = statement.query([sha])?;
    let mut seen = 0;
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.copy-mapping.v1\0");
    loop {
        step(budget)?;
        let Some(row) = rows.next()? else {
            break;
        };
        budget.visit_records(1)?;
        archive::add(&mut seen, 1, MAX_PROOF_MAPPINGS, "parent mapping rows")?;
        archive::add(total, 1, MAX_MAPPINGS, "lineage mapping rows")?;
        let fields: [String; 6] = [
            optional_text(row.get(0)?)?,
            optional_text(row.get(1)?)?,
            optional_text(row.get(2)?)?,
            optional_text(row.get(3)?)?,
            optional_text(row.get(4)?)?,
            optional_text(row.get(5)?)?,
        ];
        OperationId(fields[0].clone()).validate().map_err(invalid)?;
        OperationId(fields[1].clone()).validate().map_err(invalid)?;
        require(
            fields[0] != fields[1] && fields[2..].iter().all(|s| archive::digest_valid(s)),
            "Invalid retained operation mapping",
        )?;
        scan.prepay(
            (64 + fields.iter().map(String::len).sum::<usize>()) as i64,
            640,
        )?;
        for field in &fields {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        scan.record(
            b"parent-map",
            &[
                sha.as_bytes(),
                fields[0].as_bytes(),
                fields[1].as_bytes(),
                fields[2].as_bytes(),
                fields[3].as_bytes(),
                fields[4].as_bytes(),
                fields[5].as_bytes(),
            ],
        );
        if is_live {
            step(budget)?;
            let current:Option<(Option<String>,Option<String>)>=connection.query_row("SELECT CASE WHEN octet_length(request_sha256)=64 THEN request_sha256 END,CASE WHEN octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE operation_id=?1 AND project_id=?2",params![fields[1],project],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            require(
                current.is_some_and(|(request, result)| {
                    request.as_deref() == Some(fields[4].as_str())
                        && result.as_deref() == Some(fields[5].as_str())
                }),
                "Retained live operation differs from its immutable copy mapping",
            )?;
        }
    }
    require(
        seen == expected && format!("{:x}", hash.finalize()) == expected_sha,
        "Retained mapping count/digest differs",
    )
}

fn validate_links(
    connection: &Connection,
    project: &str,
    validator: &ValidateClosure<'_, '_>,
) -> Result<()> {
    let budget = validator.budget;
    for (id, bytes) in &validator.bindings {
        budget.charge_work((bytes.len() as u64).saturating_mul(32) + 4096)?;
        let binding: PerformanceProjectionBinding =
            serde_json::from_slice(bytes).map_err(invalid)?;
        let operation = crate::projection_operation_id(&ProjectId(project.into()), id)?;
        step(budget)?;
        let lengths:Option<(i64,i64,i64)>=connection.query_row("SELECT octet_length(request_json),octet_length(result_json),octet_length(project_id) FROM operations WHERE operation_id=?1",[&operation.0],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let (request_length, result_length, project_length) = lengths
            .ok_or_else(|| invalid("Retained binding has no live deterministic publication row"))?;
        let request_length = bounded_length(request_length, WORKER_MAX_BYTES)?;
        let result_length = bounded_length(result_length, MAX_NATIVE_PROJECT_BYTES)?;
        bounded_length(project_length, 96)?;
        budget.charge_work(
            (request_length as u64 + result_length as u64).saturating_mul(64) + 4096,
        )?;
        step(budget)?;
        let request:Option<String>=connection.query_row("SELECT CASE WHEN octet_length(request_json)<=?3 THEN request_json END FROM operations WHERE operation_id=?1 AND project_id=?2",params![operation.0,project,WORKER_MAX_BYTES as i64],|r|r.get(0)).optional()?.flatten();
        let request = optional_text(request)?;
        let ArchiveOperation::Projection(intent) = ArchiveOperation::parse(&request)? else {
            return Err(invalid(
                "Retained binding linkage is not a typed projection publication",
            ));
        };
        require(
            intent.project_id.0 == project && intent.projection_id == *id,
            "Retained binding publication belongs to a different live namespace",
        )?;
        let receipt = projection_publication::recorded_commit_with_budget(
            connection, &intent, &request, budget,
        )?
        .ok_or_else(|| invalid("Retained binding publication receipt is absent"))?;
        require(
            receipt.projection_id == *id
                && receipt.review_sha256 == binding.review.review_sha256
                && receipt.source == binding.source,
            "Retained binding publication receipt differs from the authenticated original binding",
        )?;
    }
    Ok(())
}

fn authenticate_assets(
    connection: &Connection,
    ids: &BTreeSet<String>,
    budget: &InspectionBudget,
) -> Result<()> {
    // The caller's consistent catalog snapshot protects these immutable blobs;
    // repeated references in any snapshot never repeat their byte admission.
    for id in ids {
        step(budget)?;
        let length: i64 = connection.query_row(
            "SELECT octet_length(payload) FROM assets WHERE id=?1",
            [id],
            |r| r.get(0),
        )?;
        require(
            (1..=crate::MAX_ASSET_BYTES as i64).contains(&length),
            "Retained audio asset exceeds its bounded size",
        )?;
        budget.charge_work((length as u64).saturating_mul(4) + 4096)?;
        step(budget)?;
        crate::read_asset(connection, id)?;
    }
    budget.checkpoint()?;
    Ok(())
}

/// Source-semantic diagnostic for the actual live catalog only. The caller must
/// retain its catalog transaction/lease across this whole operation; a summary
/// is not a reusable source proof or permission to activate an archive.
pub(crate) fn inspect(
    connection: &Connection,
    root: &Path,
    project_id: &str,
    budget: &InspectionBudget,
) -> Result<RetainedProjectionClosureInspection> {
    let result = (|| {
        budget.checkpoint()?;
        require(
            !connection.is_autocommit(),
            "Retained closure inspection requires its live catalog transaction",
        )?;
        require(
            project_id.len() <= 96,
            "Retained closure project identity exceeds its byte bound",
        )?;
        budget.charge_work(project_id.len() as u64 + 4096)?;
        ProjectId(project_id.into()).validate().map_err(invalid)?;
        let mut collect = Collect {
            set: RegisteredSourceSet::new(budget),
            takes: BTreeSet::new(),
            assets: BTreeSet::new(),
            budget,
        };
        let first = walk(connection, project_id, budget, &mut collect)?;
        authenticate_assets(connection, &collect.assets, budget)?;
        let sources = RegisteredProjectionSources::open_collected(root, connection, collect.set)?;
        let proofs = sources.authenticate(budget)?;
        let mut validator = ValidateClosure {
            proofs: &proofs,
            budget,
            bindings: BTreeMap::new(),
            memo_bytes: 0,
        };
        let second = walk(connection, project_id, budget, &mut validator)?;
        require(
            first == second,
            "Retained closure changed between its source collection and semantic passes",
        )?;
        validate_links(connection, project_id, &validator)?;
        proofs.verify_registrations(connection)?;
        proofs.verify_unchanged()?;
        budget.checkpoint()?;
        budget.charge_work(project_id.len() as u64 + 128)?;
        Ok(RetainedProjectionClosureInspection {
            version: 1,
            project_id: project_id.into(),
            snapshots: second.snapshots,
            history_rows: second.history,
            operation_rows: second.operations,
            lineage_proof_rows: second.proofs,
            distinct_bindings: validator.bindings.len() as u64,
        })
    })();
    finish(budget, result)
}

#[cfg(test)]
mod tests;

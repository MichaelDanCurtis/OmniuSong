//! Hash-bound archive acquisition, typed copies and immutable lineage proofs.
//! Storage-thread only. A proof's historical IDs never become live identities.

use crate::{
    NativeStore, Result, StoreError,
    archive::{self, Bundle},
    archive_boundary::{
        self as boundary, ArchiveAction, ArchiveCompletion, ArchiveContext, ArchiveFailure,
        ArchiveOutcome, ArchivePhase, ArchiveSqlOrigin,
    },
    archive_postimage::{Layout, PostimageLedger},
    catalog_transaction::CatalogTransaction,
    checksum, load_document,
    takes::verify_owned_take,
};
use archive::{ensure, invalid};
use omniusong_core::{
    MAX_NATIVE_PROJECT_BYTES, ProjectDocument,
    contracts::{OperationId, ProjectId, Validate, WORKER_MAX_BYTES},
};
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::fs;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_LINEAGE_PROOFS: usize = 256;
pub const MAX_LINEAGE_MAPPINGS: u64 = 1_000_000;
const MAX_PROOF_MAPPINGS: u64 = 100_000;
// Compact nested JSON, not an escaped JSON string. Leaves room for bounded ancestry.
const MAX_PROOF_BYTES: usize = MAX_NATIVE_PROJECT_BYTES + 32 * 1024;

fn tagged_sql<T>(origin: ArchiveSqlOrigin, result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|error| boundary::sql_error(origin, error))
}
fn tagged<T>(origin: ArchiveSqlOrigin, result: Result<T>) -> Result<T> {
    match origin {
        ArchiveSqlOrigin::Input => boundary::input(result),
        ArchiveSqlOrigin::PrivateOutput => boundary::output(result),
        ArchiveSqlOrigin::LiveCatalog => boundary::live(result),
    }
}

pub(crate) const LINEAGE_TABLES: &[(&str, &str)] = &[
    (
        "archive_copy_proofs",
        "CREATE TABLE archive_copy_proofs(proof_sha256 TEXT PRIMARY KEY NOT NULL,receipt_json TEXT NOT NULL)",
    ),
    (
        "archive_copy_proof_operations",
        "CREATE TABLE archive_copy_proof_operations(proof_sha256 TEXT NOT NULL REFERENCES archive_copy_proofs(proof_sha256),source_operation_id TEXT NOT NULL,destination_operation_id TEXT NOT NULL,source_request_sha256 TEXT NOT NULL,source_result_sha256 TEXT NOT NULL,destination_request_sha256 TEXT NOT NULL,destination_result_sha256 TEXT NOT NULL,PRIMARY KEY(proof_sha256,source_operation_id),UNIQUE(proof_sha256,destination_operation_id))",
    ),
    (
        "archive_project_lineage",
        "CREATE TABLE archive_project_lineage(project_id TEXT NOT NULL REFERENCES projects(id),proof_sha256 TEXT NOT NULL REFERENCES archive_copy_proofs(proof_sha256),PRIMARY KEY(project_id,proof_sha256))",
    ),
    (
        "archive_copies",
        "CREATE TABLE archive_copies(operation_id TEXT PRIMARY KEY NOT NULL,project_id TEXT UNIQUE NOT NULL REFERENCES projects(id),proof_sha256 TEXT UNIQUE NOT NULL REFERENCES archive_copy_proofs(proof_sha256))",
    ),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveInspection {
    pub sha256: String,
    pub byte_length: u64,
    pub project_id: String,
    pub name: String,
    pub track_count: usize,
    pub history_rows: u64,
    pub operation_rows: u64,
    pub asset_count: u64,
    pub take_count: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RestoreCopyIntent {
    pub operation_id: OperationId,
    pub expected_archive_sha256: String,
}
impl RestoreCopyIntent {
    pub fn validate(&self) -> Result<()> {
        self.operation_id
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        ensure(
            archive::digest_valid(&self.expected_archive_sha256),
            "Copy requires a lowercase SHA-256 archive fingerprint",
        )
    }
}
#[derive(Clone, Debug)]
pub struct RestoreCopyReceipt {
    pub operation_id: OperationId,
    pub source_project_id: String,
    pub source_archive_sha256: String,
    pub document: ProjectDocument,
    pub replayed: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
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
impl Proof {
    fn document(&self) -> Result<ProjectDocument> {
        let document = ProjectDocument::from_value(self.result.clone())?;
        super::projection_boundary::reject_unverified_projection(&document)?;
        Ok(document)
    }
    fn validate(&self) -> Result<()> {
        self.operation_id
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        ProjectId(self.source_project_id.clone())
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        ProjectId(self.destination_project_id.clone())
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        let document = self.document()?;
        ensure(
            self.version == 1
                && archive::digest_valid(&self.source_archive_sha256)
                && self.source_archive_byte_length > 0
                && self.source_archive_byte_length <= archive::MAX_ARCHIVE_FILE_BYTES
                && self.source_project_id != self.destination_project_id
                && document.id() == self.destination_project_id
                && checksum(&document.to_json()?) == self.result_sha256
                && self.mapping_count <= MAX_PROOF_MAPPINGS
                && archive::digest_valid(&self.mapping_sha256)
                && self.ancestors.len() < MAX_LINEAGE_PROOFS
                && self.ancestors.iter().all(|s| archive::digest_valid(s))
                && self.ancestors.windows(2).all(|p| p[0] < p[1]),
            "Invalid copy lineage proof",
        )
    }
    fn json(&self) -> Result<String> {
        self.validate()?;
        let json = serde_json::to_string(self).map_err(|e| invalid(e.to_string()))?;
        ensure(
            json.len() <= MAX_PROOF_BYTES,
            "Copy proof exceeds its metadata row ceiling",
        )?;
        Ok(json)
    }
    fn receipt(&self, replayed: bool) -> Result<RestoreCopyReceipt> {
        Ok(RestoreCopyReceipt {
            operation_id: self.operation_id.clone(),
            source_project_id: self.source_project_id.clone(),
            source_archive_sha256: self.source_archive_sha256.clone(),
            document: self.document()?,
            replayed,
        })
    }

    // These methods inventory untrusted retained metadata. Unlike the legacy
    // methods above they do not confer projection authority: every caller is
    // fenced by the held source/publication/history validator before commit or
    // a bounded recorded replay can return.
    fn metadata_document(&self, budget: &InspectionBudget) -> Result<ProjectDocument> {
        let length = encoded_length(&self.result, MAX_NATIVE_PROJECT_BYTES, budget)?;
        prepay_metadata(budget, length)?;
        Ok(ProjectDocument::from_value(self.result.clone())?)
    }
    fn validate_metadata(&self, budget: &InspectionBudget) -> Result<()> {
        copy_step(Some(budget))?;
        self.operation_id
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        ProjectId(self.source_project_id.clone())
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        ProjectId(self.destination_project_id.clone())
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        let document = self.metadata_document(budget)?;
        let json = crate::archive_metadata::document_json(&document, budget)?;
        budget.charge_work(json.len() as u64)?;
        ensure(
            self.version == 1
                && archive::digest_valid(&self.source_archive_sha256)
                && self.source_archive_byte_length > 0
                && self.source_archive_byte_length <= archive::MAX_ARCHIVE_FILE_BYTES
                && self.source_project_id != self.destination_project_id
                && document.id() == self.destination_project_id
                && checksum(json.as_bytes()) == self.result_sha256
                && self.mapping_count <= MAX_PROOF_MAPPINGS
                && archive::digest_valid(&self.mapping_sha256)
                && self.ancestors.len() < MAX_LINEAGE_PROOFS
                && self.ancestors.iter().all(|s| archive::digest_valid(s))
                && self.ancestors.windows(2).all(|p| p[0] < p[1]),
            "Invalid copy lineage proof",
        )?;
        budget.checkpoint()?;
        Ok(())
    }
    fn metadata_json(&self, budget: &InspectionBudget) -> Result<String> {
        self.validate_metadata(budget)?;
        bounded_encoding(self, MAX_PROOF_BYTES, budget)
    }
    fn metadata_receipt(
        &self,
        replayed: bool,
        budget: &InspectionBudget,
    ) -> Result<RestoreCopyReceipt> {
        copy_step(Some(budget))?;
        Ok(RestoreCopyReceipt {
            operation_id: self.operation_id.clone(),
            source_project_id: self.source_project_id.clone(),
            source_archive_sha256: self.source_archive_sha256.clone(),
            document: self.metadata_document(budget)?,
            replayed,
        })
    }
}

fn copy_step(budget: Option<&InspectionBudget>) -> Result<()> {
    if let Some(budget) = budget {
        budget.visit_rows(1)?;
        // Covers bounded IDs, hashes, map fields and their fixed-size copies.
        // Variable JSON is credited from its exact byte length separately.
        budget.charge_work(4096)?;
    }
    Ok(())
}
fn prepay_metadata(budget: &InspectionBudget, length: usize) -> Result<()> {
    budget.charge_work(
        (length as u64)
            .checked_mul(64)
            .and_then(|value| value.checked_add(4096))
            .ok_or_else(|| invalid("Copy metadata work overflow"))?,
    )?;
    Ok(())
}
fn metadata_length(length: i64, maximum: usize) -> Result<usize> {
    usize::try_from(length)
        .ok()
        .filter(|length| *length <= maximum)
        .ok_or_else(|| invalid("Copy metadata exceeds its bounded row ceiling"))
}
struct EncodingLength<'b> {
    budget: &'b InspectionBudget,
    length: usize,
    maximum: usize,
}
impl Write for EncodingLength<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.length) {
            return Err(std::io::Error::other(
                "Copy metadata encoding exceeds its ceiling",
            ));
        }
        self.budget
            .charge_work(bytes.len() as u64)
            .map_err(|_| std::io::Error::other("Copy metadata governor stopped encoding"))?;
        self.length += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encoded_length(
    value: &impl Serialize,
    maximum: usize,
    budget: &InspectionBudget,
) -> Result<usize> {
    let mut output = EncodingLength {
        budget,
        length: 0,
        maximum,
    };
    let result = serde_json::to_writer(&mut output, value).map_err(|e| invalid(e.to_string()));
    budget.checkpoint()?;
    result?;
    Ok(output.length)
}
fn bounded_encoding(
    value: &impl Serialize,
    maximum: usize,
    budget: &InspectionBudget,
) -> Result<String> {
    let length = encoded_length(value, maximum, budget)?;
    // Prepay output allocation, serialization, checksum and bounded copies.
    prepay_metadata(budget, length)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| invalid("Cannot reserve copy metadata encoding"))?;
    serde_json::to_writer(&mut bytes, value).map_err(|e| invalid(e.to_string()))?;
    ensure(
        bytes.len() == length,
        "Copy metadata changed while encoding",
    )?;
    budget.checkpoint()?;
    String::from_utf8(bytes).map_err(|e| invalid(e.to_string()))
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Mapping {
    source: String,
    destination: String,
    source_request: String,
    source_result: String,
    destination_request: String,
    destination_result: String,
}
impl Mapping {
    fn fields(&self) -> [&[u8]; 6] {
        [
            self.source.as_bytes(),
            self.destination.as_bytes(),
            self.source_request.as_bytes(),
            self.source_result.as_bytes(),
            self.destination_request.as_bytes(),
            self.destination_result.as_bytes(),
        ]
    }
    fn validate(&self) -> Result<()> {
        OperationId(self.source.clone())
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        OperationId(self.destination.clone())
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        ensure(
            self.source != self.destination
                && [
                    &self.source_request,
                    &self.source_result,
                    &self.destination_request,
                    &self.destination_result,
                ]
                .iter()
                .all(|s| archive::digest_valid(s)),
            "Invalid copy operation mapping",
        )
    }
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source: row.get(0)?,
            destination: row.get(1)?,
            source_request: row.get(2)?,
            source_result: row.get(3)?,
            destination_request: row.get(4)?,
            destination_result: row.get(5)?,
        })
    }
}
const MAP_SELECT: &str = "SELECT CASE WHEN octet_length(source_operation_id)<=96 THEN source_operation_id END,CASE WHEN octet_length(destination_operation_id)<=96 THEN destination_operation_id END,CASE WHEN octet_length(source_request_sha256)=64 THEN source_request_sha256 END,CASE WHEN octet_length(source_result_sha256)=64 THEN source_result_sha256 END,CASE WHEN octet_length(destination_request_sha256)=64 THEN destination_request_sha256 END,CASE WHEN octet_length(destination_result_sha256)=64 THEN destination_result_sha256 END FROM archive_copy_proof_operations WHERE proof_sha256=?1 ORDER BY source_operation_id";
fn mapping_hash() -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.copy-mapping.v1\0");
    hash
}
fn hash_mapping(hash: &mut Sha256, mapping: &Mapping) {
    for field in mapping.fields() {
        hash.update((field.len() as u64).to_le_bytes());
        hash.update(field);
    }
}
fn has_lineage(connection: &Connection) -> Result<bool> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='archive_copy_proofs')",[],|r|r.get(0))?)
}
fn read_proof(connection: &Connection, sha: &str) -> Result<(Proof, String)> {
    ensure(archive::digest_valid(sha), "Invalid copy proof identity")?;
    let json:String=connection.query_row("SELECT CASE WHEN octet_length(receipt_json)<=?2 THEN receipt_json END FROM archive_copy_proofs WHERE proof_sha256=?1",params![sha,MAX_PROOF_BYTES as i64],|r|r.get(0))?;
    ensure(
        checksum(json.as_bytes()) == sha,
        "Copy proof checksum mismatch",
    )?;
    let proof: Proof = serde_json::from_str(&json).map_err(|e| invalid(e.to_string()))?;
    ensure(proof.json()? == json, "Noncanonical copy proof")?;
    Ok((proof, json))
}
fn read_proof_bounded(
    connection: &Connection,
    sha: &str,
    budget: &InspectionBudget,
) -> Result<(Proof, String)> {
    copy_step(Some(budget))?;
    ensure(archive::digest_valid(sha), "Invalid copy proof identity")?;
    let length: i64 = connection.query_row(
        "SELECT octet_length(receipt_json) FROM archive_copy_proofs WHERE proof_sha256=?1",
        [sha],
        |row| row.get(0),
    )?;
    let length = metadata_length(length, MAX_PROOF_BYTES)?;
    prepay_metadata(budget, length)?;
    copy_step(Some(budget))?;
    let json: String = connection.query_row(
        "SELECT CASE WHEN octet_length(receipt_json)<=?2 THEN receipt_json END FROM archive_copy_proofs WHERE proof_sha256=?1",
        params![sha, MAX_PROOF_BYTES as i64], |row| row.get(0),
    )?;
    ensure(
        json.len() == length && checksum(json.as_bytes()) == sha,
        "Copy proof checksum mismatch",
    )?;
    let proof: Proof = serde_json::from_str(&json).map_err(|e| invalid(e.to_string()))?;
    ensure(
        proof.metadata_json(budget)? == json,
        "Noncanonical copy proof",
    )?;
    budget.checkpoint()?;
    Ok((proof, json))
}
fn retained_proof(
    connection: &Connection,
    sha: &str,
    budget: Option<&InspectionBudget>,
) -> Result<(Proof, String)> {
    match budget {
        Some(budget) => read_proof_bounded(connection, sha, budget),
        None => read_proof(connection, sha),
    }
}
fn retained_document(proof: &Proof, budget: Option<&InspectionBudget>) -> Result<ProjectDocument> {
    match budget {
        Some(budget) => proof.metadata_document(budget),
        None => proof.document(),
    }
}
fn insert_mapping(connection: &Connection, sha: &str, m: &Mapping) -> Result<()> {
    connection.execute(
        "INSERT INTO archive_copy_proof_operations VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            sha,
            m.source,
            m.destination,
            m.source_request,
            m.source_result,
            m.destination_request,
            m.destination_result
        ],
    )?;
    Ok(())
}
fn validate_live_mapping(connection: &Connection, project: &str, m: &Mapping) -> Result<()> {
    validate_live_mapping_in(connection, project, m, None)
}
fn validate_live_mapping_in(
    connection: &Connection,
    project: &str,
    m: &Mapping,
    budget: Option<&InspectionBudget>,
) -> Result<()> {
    copy_step(budget)?;
    if let Some(budget) = budget {
        let (request, result): (i64, i64) = connection.query_row(
            "SELECT octet_length(request_json),octet_length(result_json) FROM operations WHERE operation_id=?1 AND project_id=?2",
            params![m.destination, project], |row| Ok((row.get(0)?,row.get(1)?)),
        )?;
        prepay_metadata(budget, metadata_length(request, WORKER_MAX_BYTES)?)?;
        prepay_metadata(budget, metadata_length(result, MAX_NATIVE_PROJECT_BYTES)?)?;
    }
    copy_step(budget)?;
    let (request,request_sha,previous,committed,result,result_sha):(String,String,i64,i64,String,String)=connection.query_row("SELECT CASE WHEN octet_length(request_json)<=?3 THEN request_json END,CASE WHEN octet_length(request_sha256)=64 THEN request_sha256 END,previous_revision,committed_revision,CASE WHEN octet_length(result_json)<=?4 THEN result_json END,CASE WHEN octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE operation_id=?1 AND project_id=?2",params![m.destination,project,WORKER_MAX_BYTES as i64,MAX_NATIVE_PROJECT_BYTES as i64],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    let doc = ProjectDocument::from_json(result.as_bytes())?;
    let previous = archive::unsigned(previous, "mapped previous revision")?;
    ensure(
        request_sha == m.destination_request
            && result_sha == m.destination_result
            && checksum(request.as_bytes()) == request_sha
            && checksum(result.as_bytes()) == result_sha
            && doc.id() == project
            && doc.revision() == archive::unsigned(committed, "mapped committed revision")?
            && previous.checked_add(1) == Some(doc.revision()),
        "Copied live operation differs from its immutable mapping",
    )?;
    match budget {
        Some(budget) => crate::archive_metadata::validate_request(
            &request,
            &m.destination,
            project,
            previous,
            &doc,
            budget,
        ),
        None => archive::validate_request(&request, &m.destination, project, previous, &doc),
    }
}

/// Validates and streams immutable proofs. Historical IDs are copied verbatim;
/// only the association's project ID can change. No history is collected in RAM.
pub(super) fn read_lineage(
    source: &Connection,
    destination: Option<(&Connection, &str, bool, ArchiveSqlOrigin)>,
    project_id: &str,
    bundle: &mut Bundle,
    source_is_archive: bool,
) -> Result<BTreeSet<String>> {
    read_lineage_in(
        source,
        destination,
        project_id,
        bundle,
        source_is_archive,
        None,
    )
}

/// Untrusted bounded metadata traversal. A successful walk is not a source or
/// publication proof; bounded routes must subsequently use RetainedValidator.
pub(super) fn read_lineage_bounded(
    source: &Connection,
    destination: Option<(&Connection, &str, bool, ArchiveSqlOrigin)>,
    project_id: &str,
    bundle: &mut Bundle,
    source_is_archive: bool,
    budget: &InspectionBudget,
) -> Result<BTreeSet<String>> {
    budget.checkpoint()?;
    ensure(
        project_id.len() <= 96,
        "Copy lineage project identity exceeds its byte ceiling",
    )?;
    copy_step(Some(budget))?;
    ProjectId(project_id.into())
        .validate()
        .map_err(|e| invalid(e.to_string()))?;
    read_lineage_in(
        source,
        destination,
        project_id,
        bundle,
        source_is_archive,
        Some(budget),
    )
}
fn read_lineage_in(
    source: &Connection,
    destination: Option<(&Connection, &str, bool, ArchiveSqlOrigin)>,
    project_id: &str,
    bundle: &mut Bundle,
    source_is_archive: bool,
    budget: Option<&InspectionBudget>,
) -> Result<BTreeSet<String>> {
    copy_step(budget)?;
    if !has_lineage(source)? {
        if let Some(budget) = budget {
            budget.checkpoint()?;
        }
        return Ok(BTreeSet::new());
    }
    copy_step(budget)?;
    let lineage_count: i64 = source.query_row(
        "SELECT count(*) FROM archive_project_lineage WHERE project_id=?1",
        [project_id],
        |r| r.get(0),
    )?;
    ensure(
        (0..=MAX_LINEAGE_PROOFS as i64).contains(&lineage_count),
        "Copy ancestry exceeds its proof ceiling",
    )?;
    let mut statement=source.prepare("SELECT CASE WHEN octet_length(proof_sha256)=64 THEN proof_sha256 END FROM archive_project_lineage WHERE project_id=?1 ORDER BY proof_sha256")?;
    let mut rows = statement.query([project_id])?;
    let mut ids = BTreeSet::new();
    while let Some(row) = {
        copy_step(budget)?;
        rows.next()?
    } {
        ensure(
            ids.len() < MAX_LINEAGE_PROOFS,
            "Copy ancestry exceeds its proof ceiling",
        )?;
        ids.insert(row.get::<_, String>(0)?);
    }
    ensure(
        ids.len() as i64 == lineage_count,
        "Copy ancestry changed during traversal",
    )?;
    drop(rows);
    drop(statement);
    copy_step(budget)?;
    let live:Option<(String,String)>=source.query_row("SELECT CASE WHEN octet_length(operation_id)<=96 THEN operation_id END,CASE WHEN octet_length(proof_sha256)=64 THEN proof_sha256 END FROM archive_copies WHERE project_id=?1",[project_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    ensure(
        ids.is_empty() == live.is_none(),
        "Project copy admission and lineage are incomplete",
    )?;
    let mut ancestors = BTreeMap::new();
    let mut identities = BTreeMap::new();
    let mut total_maps = 0;
    for sha in &ids {
        copy_step(budget)?;
        let (proof, json) = retained_proof(source, sha, budget)?;
        ensure(
            !proof.ancestors.contains(sha) && proof.ancestors.iter().all(|p| ids.contains(p)),
            "Copy ancestry has a missing or self-referential proof",
        )?;
        let is_live = live.as_ref().is_some_and(|(_, p)| p == sha);
        if is_live {
            let (operation, _) = live.as_ref().expect("is_live checked");
            ensure(
                proof.operation_id.0 == *operation
                    && proof.destination_project_id == project_id
                    && proof.ancestors.iter().cloned().collect::<BTreeSet<_>>()
                        == ids.iter().filter(|p| *p != sha).cloned().collect()
                    && retained_document(&proof, budget)?.revision() <= bundle.project.revision(),
                "Active copy receipt does not own its complete ancestry",
            )?;
            let collision: bool = source.query_row(
                "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
                [operation],
                |r| r.get(0),
            )?;
            ensure(!collision, "Copy admission collides with an edit operation")?;
        }
        ancestors.insert(sha.clone(), proof.ancestors.clone());
        identities.insert(
            sha.clone(),
            (
                proof.source_project_id.clone(),
                proof.destination_project_id.clone(),
            ),
        );
        bundle.metadata(json.len() + sha.len())?;
        let document = retained_document(&proof, budget)?;
        if budget.is_some() {
            bundle.references_untrusted(&document)?;
        } else {
            bundle.references(&document)?;
        }
        bundle.record(b"copy-proof", &[sha.as_bytes(), json.as_bytes()]);
        let reuse = if let Some((dest, _, _, origin)) = destination {
            if let Some(budget) = budget {
                copy_step(Some(budget))?;
                let length: Option<i64> = tagged_sql(origin, dest.query_row(
                    "SELECT octet_length(receipt_json) FROM archive_copy_proofs WHERE proof_sha256=?1",
                    [sha], |row| row.get(0),
                ).optional())?;
                if let Some(length) = length {
                    prepay_metadata(budget, metadata_length(length, MAX_PROOF_BYTES)?)?;
                }
            }
            copy_step(budget)?;
            let old:Option<String>=tagged_sql(origin,dest.query_row("SELECT CASE WHEN octet_length(receipt_json)<=?2 THEN receipt_json END FROM archive_copy_proofs WHERE proof_sha256=?1",params![sha,MAX_PROOF_BYTES as i64],|r|r.get(0)).optional())?;
            if let Some(old) = old {
                ensure(old == json, "Existing lineage proof differs")?;
                true
            } else {
                tagged_sql(
                    origin,
                    dest.execute(
                        "INSERT INTO archive_copy_proofs VALUES(?1,?2)",
                        params![sha, json],
                    ),
                )?;
                false
            }
        } else {
            false
        };
        let mut maps = source.prepare(MAP_SELECT)?;
        let mut rows = maps.query([sha])?;
        let mut count = 0;
        let mut hash = mapping_hash();
        while let Some(row) = {
            copy_step(budget)?;
            rows.next()?
        } {
            archive::add(&mut count, 1, MAX_PROOF_MAPPINGS, "proof mapping count")?;
            archive::add(
                &mut total_maps,
                1,
                MAX_LINEAGE_MAPPINGS,
                "lineage mapping count",
            )?;
            let mapping = Mapping::from_row(row)?;
            mapping.validate()?;
            hash_mapping(&mut hash, &mapping);
            if is_live {
                match budget {
                    Some(budget) => {
                        validate_live_mapping_in(source, project_id, &mapping, Some(budget))?
                    }
                    None => validate_live_mapping(source, project_id, &mapping)?,
                }
            }
            bundle.metadata(sha.len() + mapping.fields().iter().map(|s| s.len()).sum::<usize>())?;
            bundle.record(
                b"copy-map",
                &[
                    sha.as_bytes(),
                    mapping.fields()[0],
                    mapping.fields()[1],
                    mapping.fields()[2],
                    mapping.fields()[3],
                    mapping.fields()[4],
                    mapping.fields()[5],
                ],
            );
            if let Some((dest, _, _, origin)) = destination {
                if reuse {
                    copy_step(budget)?;
                    let old: Mapping = tagged_sql(
                        origin,
                        dest.query_row(
                            &MAP_SELECT
                                .replace(" ORDER BY", " AND source_operation_id=?2 ORDER BY"),
                            params![sha, mapping.source],
                            Mapping::from_row,
                        ),
                    )?;
                    ensure(old == mapping, "Existing lineage operation mapping differs")?;
                } else {
                    tagged(origin, insert_mapping(dest, sha, &mapping))?;
                }
            }
        }
        ensure(
            count == proof.mapping_count
                && format!("{:x}", hash.finalize()) == proof.mapping_sha256,
            "Copy mapping count or digest mismatch",
        )?;
        if let Some((dest, dest_project, _, origin)) = destination {
            copy_step(budget)?;
            let n: i64 = tagged_sql(
                origin,
                dest.query_row(
                    "SELECT count(*) FROM archive_copy_proof_operations WHERE proof_sha256=?1",
                    [sha],
                    |r| r.get(0),
                ),
            )?;
            ensure(
                n == count as i64,
                "Shared copy proof has extraneous mapping rows",
            )?;
            tagged_sql(
                origin,
                dest.execute(
                    "INSERT INTO archive_project_lineage(project_id,proof_sha256) VALUES(?1,?2)",
                    params![dest_project, sha],
                ),
            )?;
        }
        bundle.metadata(project_id.len() + sha.len())?;
        bundle.record(b"copy-owner", &[project_id.as_bytes(), sha.as_bytes()]);
    }
    for (sha, parents) in &ancestors {
        for parent in parents {
            copy_step(budget)?;
            let grandparents = ancestors
                .get(parent)
                .ok_or_else(|| invalid("Missing copy ancestor"))?;
            ensure(
                grandparents.iter().all(|p| parents.contains(p)) && !grandparents.contains(sha),
                "Copy ancestry is cyclic or not transitively closed",
            )?;
        }
    }
    // Every non-root edge must extend precisely its source project's prior
    // edge, retaining that edge's mapping as part of the new operation map.
    for (sha, parents) in &ancestors {
        copy_step(budget)?;
        if parents.is_empty() {
            continue;
        }
        let source_project = &identities[sha].0;
        let immediate: Vec<_> = parents
            .iter()
            .filter(|parent| identities[*parent].1 == *source_project)
            .collect();
        ensure(
            immediate.len() == 1,
            "Copy ancestry has no unique source-project edge",
        )?;
        let parent = immediate[0];
        let expected: Vec<_> = parents.iter().filter(|p| *p != parent).cloned().collect();
        ensure(
            ancestors[parent] == expected,
            "Copy ancestry does not extend the complete source lineage",
        )?;
        let broken: i64 = source.query_row(
            "SELECT count(*) FROM archive_copy_proof_operations p LEFT JOIN archive_copy_proof_operations c ON c.proof_sha256=?1 AND c.source_operation_id=p.destination_operation_id WHERE p.proof_sha256=?2 AND (c.source_operation_id IS NULL OR c.source_request_sha256!=p.destination_request_sha256 OR c.source_result_sha256!=p.destination_result_sha256)",
            params![sha,parent], |r|r.get(0))?;
        ensure(
            broken == 0,
            "Copy ancestry mapping lost or altered a prior copied operation",
        )?;
    }
    if let Some((operation, sha)) = live {
        ensure(
            ids.contains(&sha),
            "Active copy proof is absent from project ancestry",
        )?;
        bundle.metadata(operation.len() + sha.len() + project_id.len())?;
        bundle.record(
            b"copy-admission",
            &[operation.as_bytes(), project_id.as_bytes(), sha.as_bytes()],
        );
        if let Some((dest, dest_project, true, origin)) = destination {
            if source_is_archive {
                super::source_attachment::reject_reserved_operation(&operation)?;
            }
            tagged_sql(
                origin,
                dest.execute(
                    "INSERT INTO archive_copies VALUES(?1,?2,?3)",
                    params![operation, dest_project, sha],
                ),
            )?;
        }
    }
    if source_is_archive {
        ensure(
            archive::count(source, "archive_copy_proofs")? == ids.len() as u64
                && archive::count(source, "archive_project_lineage")? == ids.len() as u64
                && archive::count(source, "archive_copy_proof_operations")? == total_maps
                && archive::count(source, "archive_copies")? == u64::from(!ids.is_empty()),
            "Archive contains unrelated lineage rows",
        )?;
    }
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    Ok(ids)
}

pub(super) fn check_lineage_references(connection: &Connection, project_id: &str) -> Result<()> {
    if !has_lineage(connection)? {
        return Ok(());
    }
    let mut statement=connection.prepare("SELECT CASE WHEN octet_length(proof_sha256)=64 THEN proof_sha256 END FROM archive_project_lineage WHERE project_id=?1")?;
    let mut rows = statement.query([project_id])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        ensure(count <= MAX_LINEAGE_PROOFS, "Too many lineage proofs")?;
        let sha: String = row.get(0)?;
        crate::check_references(connection, &read_proof(connection, &sha)?.0.document()?)?;
    }
    Ok(())
}

fn validate_owned_closure(connection: &Connection, root: &Path, bundle: &Bundle) -> Result<()> {
    for id in &bundle.assets {
        crate::read_asset(connection, id)?;
    }
    for (id, _) in archive::take_ids(bundle)? {
        verify_owned_take(
            root,
            &crate::takes::load_take(connection, &omniusong_core::contracts::TakeId(id))?,
        )?;
    }
    archive::validate_all_reference_snapshots(connection, bundle.project.id())
}
fn replay(
    connection: &Connection,
    root: &Path,
    intent: &RestoreCopyIntent,
) -> Result<Option<RestoreCopyReceipt>> {
    let entry:Option<(String,String)>=connection.query_row("SELECT CASE WHEN octet_length(project_id)<=80 THEN project_id END,CASE WHEN octet_length(proof_sha256)=64 THEN proof_sha256 END FROM archive_copies WHERE operation_id=?1",[&intent.operation_id.0],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((project, sha)) = entry {
        let (proof, _) = read_proof(connection, &sha)?;
        ensure(
            proof.operation_id == intent.operation_id
                && proof.source_archive_sha256 == intent.expected_archive_sha256
                && proof.destination_project_id == project,
            "Copy operation ID was already used for another archive or receipt",
        )?;
        let mut bundle = archive::copy_metadata(connection, None, &project)?;
        read_lineage(connection, None, &project, &mut bundle, false)?;
        validate_owned_closure(connection, root, &bundle)?;
        return Ok(Some(proof.receipt(true)?));
    }
    let collision: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
        [&intent.operation_id.0],
        |r| r.get(0),
    )?;
    ensure(!collision, "Copy operation ID is already an edit operation")?;
    Ok(None)
}
fn replay_bounded(
    connection: &Connection,
    root: &Path,
    intent: &RestoreCopyIntent,
    budget: &InspectionBudget,
    context: &mut ArchiveContext,
) -> Result<Option<RestoreCopyReceipt>> {
    copy_step(Some(budget))?;
    let entry: Option<(String, String)> = tagged_sql(ArchiveSqlOrigin::LiveCatalog,
        connection.query_row(
            "SELECT CASE WHEN octet_length(project_id)<=80 THEN project_id END,CASE WHEN octet_length(proof_sha256)=64 THEN proof_sha256 END FROM archive_copies WHERE operation_id=?1",
            [&intent.operation_id.0], |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional())?;
    if let Some((project, sha)) = entry {
        let (proof, _) = boundary::live(read_proof_bounded(connection, &sha, budget))?;
        ensure(
            proof.operation_id == intent.operation_id
                && proof.source_archive_sha256 == intent.expected_archive_sha256
                && proof.destination_project_id == project,
            "Copy operation ID was already used for another archive or receipt",
        )?;
        // This historical commitment is proven even if current owned sources
        // are unusable or the remaining governor cannot finish replay.
        context.phase = ArchivePhase::RecordedReplay;
        // Prepare the historical receipt, but never mistake its encoding for
        // source authority. Missing/malformed owned originals refuse replay;
        // this branch never falls through into fresh acquisition or new IDs.
        let receipt = proof.metadata_receipt(true, budget)?;
        crate::archive_governed::validate_owned_closure(connection, root, &project, budget)?;
        budget.checkpoint()?;
        return Ok(Some(receipt));
    }
    copy_step(Some(budget))?;
    let collision: bool = tagged_sql(
        ArchiveSqlOrigin::LiveCatalog,
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
            [&intent.operation_id.0],
            |row| row.get(0),
        ),
    )?;
    ensure(!collision, "Copy operation ID is already an edit operation")?;
    budget.checkpoint()?;
    Ok(None)
}
fn fresh_project(connection: &Connection, source: &str) -> Result<String> {
    for _ in 0..8 {
        let id = uuid::Uuid::new_v4().to_string();
        let exists: bool = tagged_sql(
            ArchiveSqlOrigin::LiveCatalog,
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                [&id],
                |r| r.get(0),
            ),
        )?;
        if !exists && id != source {
            return Ok(id);
        }
    }
    Err(invalid("Could not allocate a fresh project identity"))
}
fn fresh_operation(connection: &Connection, source: &Connection, reserved: &str) -> Result<String> {
    for _ in 0..8 {
        let id = uuid::Uuid::new_v4().to_string();
        let exists:bool=tagged_sql(ArchiveSqlOrigin::LiveCatalog,connection.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1 UNION ALL SELECT 1 FROM archive_copies WHERE operation_id=?1)",[&id],|r|r.get(0)))?;
        let source_exists: bool = tagged_sql(
            ArchiveSqlOrigin::Input,
            source.query_row(
                "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
                [&id],
                |r| r.get(0),
            ),
        )?;
        if !exists && !source_exists && id != reserved {
            return Ok(id);
        }
    }
    Err(invalid(
        "Could not allocate a fresh global operation identity",
    ))
}
fn remap_document(json: &str, project_id: &str) -> Result<ProjectDocument> {
    let mut value = ProjectDocument::from_json(json.as_bytes())?.value().clone();
    value["id"] = project_id.into();
    Ok(ProjectDocument::from_value(value)?)
}
/// Allocate/remap only typed identities. The caller owns pending_copy_map and
/// still must authenticate the entire archive and final expected postimages.
#[cfg(test)]
pub(super) fn mapped_copy_request(
    connection: &Connection,
    source: &Connection,
    request: &str,
    project: &str,
    reserved: &str,
) -> Result<(String, String)> {
    mapped_copy_request_in(
        CopyCatalogs {
            live: connection,
            input: source,
            pending: connection,
            pending_origin: ArchiveSqlOrigin::LiveCatalog,
        },
        request,
        project,
        reserved,
    )
}
struct CopyCatalogs<'a> {
    live: &'a Connection,
    input: &'a Connection,
    pending: &'a Connection,
    pending_origin: ArchiveSqlOrigin,
}
fn mapped_copy_request_in(
    catalogs: CopyCatalogs<'_>,
    request: &str,
    project: &str,
    reserved: &str,
) -> Result<(String, String)> {
    let parsed = crate::archive_operation::ArchiveOperation::parse(request)?;
    let ordinary = if matches!(
        parsed,
        crate::archive_operation::ArchiveOperation::Projection(_)
    ) {
        String::new()
    } else {
        fresh_operation(catalogs.live, catalogs.input, reserved)?
    };
    let (operation, request) = parsed.remap(project, &ordinary)?;
    let exists: bool = tagged_sql(ArchiveSqlOrigin::LiveCatalog,catalogs.live.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1 UNION ALL SELECT 1 FROM archive_copies WHERE operation_id=?1)",
        [&operation], |row| row.get(0),
    ))?;
    let pending: bool = tagged_sql(catalogs.pending_origin,catalogs.pending.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1 UNION ALL SELECT 1 FROM archive_copies WHERE operation_id=?1 UNION ALL SELECT 1 FROM pending_copy_map WHERE destination_operation_id=?1)",
        [&operation], |row| row.get(0),
    ))?;
    let source_sql = if boundary::input(has_lineage(catalogs.input))? {
        "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1 UNION ALL SELECT 1 FROM archive_copies WHERE operation_id=?1)"
    } else {
        // Strict version-1 archives have no lineage/copy tables.
        "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)"
    };
    let source_exists: bool = tagged_sql(
        ArchiveSqlOrigin::Input,
        catalogs
            .input
            .query_row(source_sql, [&operation], |row| row.get(0)),
    )?;
    ensure(
        !exists && !pending && !source_exists && operation != reserved,
        "Copied operation identity collides with retained or pending authority",
    )?;
    Ok((operation, request))
}

fn document_json(document: &ProjectDocument) -> Result<String> {
    String::from_utf8(document.to_json()?).map_err(|e| invalid(e.to_string()))
}

/// Structural OSAR hash columns, not source or projection authentication.
pub(super) fn validate_snapshot_hashes_bounded(
    connection: &Connection,
    project_id: &str,
    budget: &InspectionBudget,
) -> Result<()> {
    copy_step(Some(budget))?;
    let (projects, edits): (i64, i64) = tagged_sql(
        ArchiveSqlOrigin::Input,
        connection.query_row(
            "SELECT (SELECT count(*) FROM projects WHERE id=?1),(SELECT count(*) FROM edits WHERE project_id=?1)",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ),
    )?;
    ensure(
        projects == 1 && (0..=10_000).contains(&edits),
        "Archive snapshot hash row count exceeds its explicit bound",
    )?;
    let mut count = 0;
    for (length_sql, payload_sql, expected_count) in [
        (
            "SELECT 0,CASE WHEN typeof(payload)='text' THEN octet_length(payload) ELSE -1 END FROM projects WHERE id=?1",
            "SELECT CASE WHEN typeof(payload)='text' AND octet_length(payload)<=?3 THEN payload END,CASE WHEN typeof(payload_sha256)='text' AND octet_length(payload_sha256)=64 THEN payload_sha256 END FROM projects WHERE id=?1 AND ?2=0",
            projects as u64,
        ),
        (
            "SELECT CASE WHEN typeof(sequence)='integer' THEN sequence ELSE -1 END,CASE WHEN typeof(before_json)='text' THEN octet_length(before_json) ELSE -1 END FROM edits WHERE project_id=?1 ORDER BY sequence",
            "SELECT CASE WHEN typeof(before_json)='text' AND octet_length(before_json)<=?3 THEN before_json END,CASE WHEN typeof(before_sha256)='text' AND octet_length(before_sha256)=64 THEN before_sha256 END FROM edits WHERE project_id=?1 AND sequence=?2",
            edits as u64,
        ),
        (
            "SELECT CASE WHEN typeof(sequence)='integer' THEN sequence ELSE -1 END,CASE WHEN typeof(after_json)='text' THEN octet_length(after_json) ELSE -1 END FROM edits WHERE project_id=?1 ORDER BY sequence",
            "SELECT CASE WHEN typeof(after_json)='text' AND octet_length(after_json)<=?3 THEN after_json END,CASE WHEN typeof(after_sha256)='text' AND octet_length(after_sha256)=64 THEN after_sha256 END FROM edits WHERE project_id=?1 AND sequence=?2",
            edits as u64,
        ),
    ] {
        copy_step(Some(budget))?;
        let mut statement = tagged_sql(ArchiveSqlOrigin::Input, connection.prepare(length_sql))?;
        let mut rows = tagged_sql(ArchiveSqlOrigin::Input, statement.query([project_id]))?;
        let mut seen = 0;
        while let Some(row) = {
            copy_step(Some(budget))?;
            tagged_sql(ArchiveSqlOrigin::Input, rows.next())?
        } {
            archive::add(&mut count, 1, 20_001, "archive snapshot hash rows")?;
            archive::add(&mut seen, 1, expected_count, "archive snapshot hash rows")?;
            let sequence: i64 = tagged_sql(ArchiveSqlOrigin::Input, row.get(0))?;
            ensure(
                (0..=10_000).contains(&sequence),
                "Archive snapshot hash identity is out of bounds",
            )?;
            let length = metadata_length(
                tagged_sql(ArchiveSqlOrigin::Input, row.get(1))?,
                MAX_NATIVE_PROJECT_BYTES,
            )?;
            // Advancing the cursor above reads only fixed identities/types and
            // SQLite's stored lengths. Prepare/request the variable JSON only
            // after this pass has paid for its complete materialization/hash.
            prepay_metadata(budget, length)?;
            copy_step(Some(budget))?;
            let (payload, expected): (Option<String>, Option<String>) = tagged_sql(
                ArchiveSqlOrigin::Input,
                connection.query_row(
                    payload_sql,
                    params![project_id, sequence, MAX_NATIVE_PROJECT_BYTES as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ),
            )?;
            let payload = payload.ok_or_else(|| invalid("Archive snapshot checksum mismatch"))?;
            let expected = expected.ok_or_else(|| invalid("Archive snapshot checksum mismatch"))?;
            ensure(
                payload.len() == length
                    && archive::digest_valid(&expected)
                    && checksum(payload.as_bytes()) == expected,
                "Archive snapshot checksum mismatch",
            )?;
        }
        ensure(
            seen == expected_count,
            "Archive snapshot hash row count changed during traversal",
        )?;
    }
    budget.checkpoint()?;
    Ok(())
}

struct BoundedCopyPlan {
    project_id: String,
    receipt: RestoreCopyReceipt,
    object_metadata: u64,
}
struct BoundedCopyInput<'a> {
    bundle: &'a Bundle,
    source_project: &'a str,
    ancestors: BTreeSet<String>,
    intent: &'a RestoreCopyIntent,
    byte_length: u64,
    sha256: &'a str,
}

/// Derive every intended row from immutable input and newly allocated typed
/// identities in a trigger-free private namespace. No live write or final-path
/// object placement occurs here, and these rows carry no source authority.
fn plan_copy_bounded(
    live: &Connection,
    input: &Connection,
    plan: &Connection,
    request: BoundedCopyInput<'_>,
    budget: &InspectionBudget,
) -> Result<BoundedCopyPlan> {
    let BoundedCopyInput {
        bundle,
        source_project,
        ancestors,
        intent,
        byte_length,
        sha256,
    } = request;
    copy_step(Some(budget))?;
    let project_id = fresh_project(live, source_project)?;
    let current = crate::archive_metadata::document_json(&bundle.project, budget)?;
    prepay_metadata(budget, current.len())?;
    let document = remap_document(&current, &project_id)?;
    let current = crate::archive_metadata::document_json(&document, budget)?;
    let cursor: i64 = tagged_sql(
        ArchiveSqlOrigin::Input,
        input.query_row(
            "SELECT cursor FROM projects WHERE id=?1",
            [source_project],
            |row| row.get(0),
        ),
    )?;
    tagged_sql(
        ArchiveSqlOrigin::PrivateOutput,
        plan.execute(
            "INSERT INTO projects(id,payload,revision,cursor) VALUES(?1,?2,?3,?4)",
            params![project_id, current, document.revision() as i64, cursor],
        ),
    )?;

    copy_step(Some(budget))?;
    let history_count: i64 = tagged_sql(
        ArchiveSqlOrigin::Input,
        input.query_row(
            "SELECT count(*) FROM edits WHERE project_id=?1",
            [source_project],
            |row| row.get(0),
        ),
    )?;
    ensure(
        (0..=10_000).contains(&history_count) && history_count as u64 == bundle.totals.edits,
        "Planned history row count differs from acquired metadata",
    )?;
    copy_step(Some(budget))?;
    let mut history = tagged_sql(ArchiveSqlOrigin::Input, input.prepare(
        "SELECT CASE WHEN typeof(sequence)='integer' THEN sequence ELSE -1 END,CASE WHEN typeof(before_json)='text' THEN octet_length(before_json) ELSE -1 END,CASE WHEN typeof(after_json)='text' THEN octet_length(after_json) ELSE -1 END FROM edits WHERE project_id=?1 ORDER BY sequence",
    ))?;
    let mut rows = tagged_sql(ArchiveSqlOrigin::Input, history.query([source_project]))?;
    let mut count = 0;
    while let Some(row) = {
        copy_step(Some(budget))?;
        tagged_sql(ArchiveSqlOrigin::Input, rows.next())?
    } {
        archive::add(&mut count, 1, 10_000, "planned history rows")?;
        let before_length = metadata_length(
            tagged_sql(ArchiveSqlOrigin::Input, row.get(1))?,
            MAX_NATIVE_PROJECT_BYTES,
        )?;
        let after_length = metadata_length(
            tagged_sql(ArchiveSqlOrigin::Input, row.get(2))?,
            MAX_NATIVE_PROJECT_BYTES,
        )?;
        prepay_metadata(budget, before_length)?;
        prepay_metadata(budget, after_length)?;
        let sequence: i64 = tagged_sql(ArchiveSqlOrigin::Input, row.get(0))?;
        ensure(
            u64::try_from(sequence).ok() == Some(count),
            "Planned history sequences are not contiguous",
        )?;
        copy_step(Some(budget))?;
        let (before, after): (Option<String>, Option<String>) = tagged_sql(
            ArchiveSqlOrigin::Input,
            input.query_row(
                "SELECT CASE WHEN typeof(before_json)='text' AND octet_length(before_json)<=?3 THEN before_json END,CASE WHEN typeof(after_json)='text' AND octet_length(after_json)<=?3 THEN after_json END FROM edits WHERE project_id=?1 AND sequence=?2",
                params![source_project, sequence, MAX_NATIVE_PROJECT_BYTES as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ),
        )?;
        let before =
            before.ok_or_else(|| invalid("Planned history exceeds its bounded row ceiling"))?;
        let after =
            after.ok_or_else(|| invalid("Planned history exceeds its bounded row ceiling"))?;
        ensure(
            before.len() == before_length && after.len() == after_length,
            "Planned history metadata changed during traversal",
        )?;
        let before =
            crate::archive_metadata::document_json(&remap_document(&before, &project_id)?, budget)?;
        let after =
            crate::archive_metadata::document_json(&remap_document(&after, &project_id)?, budget)?;
        tagged_sql(
            ArchiveSqlOrigin::PrivateOutput,
            plan.execute(
                "INSERT INTO edits VALUES(?1,?2,?3,?4)",
                params![project_id, sequence, before, after],
            ),
        )?;
    }
    ensure(
        count == history_count as u64,
        "Planned history row count changed during traversal",
    )?;
    drop(rows);
    drop(history);

    copy_step(Some(budget))?;
    let length: Option<i64> = tagged_sql(
        ArchiveSqlOrigin::Input,
        input
            .query_row(
                "SELECT octet_length(original_json) FROM imports WHERE project_id=?1",
                [source_project],
                |row| row.get(0),
            )
            .optional(),
    )?;
    if let Some(length) = length {
        prepay_metadata(
            budget,
            metadata_length(length, omniusong_core::MAX_PROJECT_BYTES)?,
        )?;
        let (sha, bytes): (String, Vec<u8>) = tagged_sql(ArchiveSqlOrigin::Input, input.query_row(
            "SELECT CASE WHEN octet_length(source_sha256)=64 THEN source_sha256 END,CASE WHEN octet_length(original_json)<=?2 THEN original_json END FROM imports WHERE project_id=?1",
            params![source_project, omniusong_core::MAX_PROJECT_BYTES as i64], |row| Ok((row.get(0)?,row.get(1)?)),
        ))?;
        tagged_sql(
            ArchiveSqlOrigin::PrivateOutput,
            plan.execute(
                "INSERT INTO imports VALUES(?1,?2,?3)",
                params![project_id, sha, bytes],
            ),
        )?;
    }
    copy_step(Some(budget))?;
    tagged_sql(ArchiveSqlOrigin::PrivateOutput, plan.execute_batch(
        "CREATE TEMP TABLE pending_copy_map(source_operation_id TEXT PRIMARY KEY,destination_operation_id TEXT UNIQUE,source_request_sha256 TEXT,source_result_sha256 TEXT,destination_request_sha256 TEXT,destination_result_sha256 TEXT);",
    ))?;
    copy_step(Some(budget))?;
    let operation_count: i64 = tagged_sql(
        ArchiveSqlOrigin::Input,
        input.query_row(
            "SELECT count(*) FROM operations WHERE project_id=?1",
            [source_project],
            |row| row.get(0),
        ),
    )?;
    ensure(
        (0..=MAX_PROOF_MAPPINGS as i64).contains(&operation_count)
            && operation_count as u64 == bundle.totals.operations,
        "Planned operation row count differs from acquired metadata",
    )?;
    copy_step(Some(budget))?;
    let mut statement = tagged_sql(ArchiveSqlOrigin::Input, input.prepare(
        "SELECT CASE WHEN typeof(operation_id)='text' AND octet_length(operation_id)<=96 THEN operation_id END,CASE WHEN typeof(request_json)='text' THEN octet_length(request_json) ELSE -1 END,CASE WHEN typeof(result_json)='text' THEN octet_length(result_json) ELSE -1 END FROM operations WHERE project_id=?1 ORDER BY operation_id",
    ))?;
    let mut rows = tagged_sql(ArchiveSqlOrigin::Input, statement.query([source_project]))?;
    let mut mapping_count = 0;
    let mut mapping_digest = mapping_hash();
    while let Some(row) = {
        copy_step(Some(budget))?;
        tagged_sql(ArchiveSqlOrigin::Input, rows.next())?
    } {
        archive::add(
            &mut mapping_count,
            1,
            MAX_PROOF_MAPPINGS,
            "planned operation mappings",
        )?;
        let request_length = metadata_length(
            tagged_sql(ArchiveSqlOrigin::Input, row.get(1))?,
            WORKER_MAX_BYTES,
        )?;
        let result_length = metadata_length(
            tagged_sql(ArchiveSqlOrigin::Input, row.get(2))?,
            MAX_NATIVE_PROJECT_BYTES,
        )?;
        prepay_metadata(budget, request_length)?;
        prepay_metadata(budget, result_length)?;
        let source_id: String = tagged_sql(ArchiveSqlOrigin::Input, row.get(0))?;
        copy_step(Some(budget))?;
        let (source_request_json, source_request, previous, revision, result, source_result): (Option<String>, Option<String>, i64, i64, Option<String>, Option<String>) = tagged_sql(
            ArchiveSqlOrigin::Input,
            input.query_row(
                "SELECT CASE WHEN typeof(request_json)='text' AND octet_length(request_json)<=?3 THEN request_json END,CASE WHEN typeof(request_sha256)='text' AND octet_length(request_sha256)=64 THEN request_sha256 END,previous_revision,committed_revision,CASE WHEN typeof(result_json)='text' AND octet_length(result_json)<=?4 THEN result_json END,CASE WHEN typeof(result_sha256)='text' AND octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE operation_id=?1 AND project_id=?2",
                params![source_id, source_project, WORKER_MAX_BYTES as i64, MAX_NATIVE_PROJECT_BYTES as i64],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            ),
        )?;
        let source_request_json = source_request_json
            .ok_or_else(|| invalid("Planned operation request exceeds its bounded row ceiling"))?;
        let source_request = source_request
            .ok_or_else(|| invalid("Planned operation request digest is malformed"))?;
        let result = result
            .ok_or_else(|| invalid("Planned operation result exceeds its bounded row ceiling"))?;
        let source_result =
            source_result.ok_or_else(|| invalid("Planned operation result digest is malformed"))?;
        ensure(
            source_request_json.len() == request_length
                && result.len() == result_length
                && checksum(source_request_json.as_bytes()) == source_request
                && checksum(result.as_bytes()) == source_result,
            "Planned operation raw metadata/hash differs",
        )?;
        let (operation, request) = mapped_copy_request_in(
            CopyCatalogs {
                live,
                input,
                pending: plan,
                pending_origin: ArchiveSqlOrigin::PrivateOutput,
            },
            &source_request_json,
            &project_id,
            &intent.operation_id.0,
        )?;
        let result_doc = remap_document(&result, &project_id)?;
        let result = crate::archive_metadata::document_json(&result_doc, budget)?;
        crate::archive_metadata::validate_request(
            &request,
            &operation,
            &project_id,
            archive::unsigned(previous, "copy operation revision")?,
            &result_doc,
            budget,
        )?;
        budget.charge_work((request.len() + result.len()) as u64)?;
        let mapping = Mapping {
            source: source_id,
            destination: operation,
            source_request,
            source_result,
            destination_request: checksum(request.as_bytes()),
            destination_result: checksum(result.as_bytes()),
        };
        mapping.validate()?;
        hash_mapping(&mut mapping_digest, &mapping);
        tagged_sql(
            ArchiveSqlOrigin::PrivateOutput,
            plan.execute(
                "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    mapping.destination,
                    project_id,
                    request,
                    mapping.destination_request,
                    previous,
                    revision,
                    result,
                    mapping.destination_result
                ],
            ),
        )?;
        tagged_sql(
            ArchiveSqlOrigin::PrivateOutput,
            plan.execute(
                "INSERT INTO pending_copy_map VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    mapping.source,
                    mapping.destination,
                    mapping.source_request,
                    mapping.source_result,
                    mapping.destination_request,
                    mapping.destination_result
                ],
            ),
        )?;
    }
    ensure(
        mapping_count == operation_count as u64,
        "Planned operation row count changed during traversal",
    )?;
    drop(rows);
    drop(statement);

    let mut inherited = crate::archive_metadata::copy_metadata(
        input,
        None,
        source_project,
        ArchiveSqlOrigin::Input,
        budget,
    )?;
    boundary::input(read_lineage_bounded(
        input,
        Some((plan, &project_id, false, ArchiveSqlOrigin::PrivateOutput)),
        source_project,
        &mut inherited,
        true,
        budget,
    ))?;
    let result_length = encoded_length(document.value(), MAX_NATIVE_PROJECT_BYTES, budget)?;
    prepay_metadata(budget, result_length)?;
    copy_step(Some(budget))?;
    let proof = Proof {
        version: 1,
        operation_id: intent.operation_id.clone(),
        source_archive_sha256: sha256.into(),
        source_archive_byte_length: byte_length,
        source_project_id: source_project.into(),
        destination_project_id: project_id.clone(),
        mapping_count,
        mapping_sha256: format!("{:x}", mapping_digest.finalize()),
        ancestors: ancestors.into_iter().collect(),
        result: document.value().clone(),
        result_sha256: checksum(current.as_bytes()),
    };
    let json = proof.metadata_json(budget)?;
    budget.charge_work(json.len() as u64)?;
    let proof_sha = checksum(json.as_bytes());
    tagged_sql(
        ArchiveSqlOrigin::PrivateOutput,
        plan.execute(
            "INSERT INTO archive_copy_proofs VALUES(?1,?2)",
            params![proof_sha, json],
        ),
    )?;
    tagged_sql(ArchiveSqlOrigin::PrivateOutput, plan.execute(
        "INSERT INTO archive_copy_proof_operations SELECT ?1,source_operation_id,destination_operation_id,source_request_sha256,source_result_sha256,destination_request_sha256,destination_result_sha256 FROM pending_copy_map", [&proof_sha],
    ))?;
    tagged_sql(
        ArchiveSqlOrigin::PrivateOutput,
        plan.execute_batch("DROP TABLE pending_copy_map"),
    )?;
    tagged_sql(
        ArchiveSqlOrigin::PrivateOutput,
        plan.execute(
            "INSERT INTO archive_project_lineage VALUES(?1,?2)",
            params![project_id, proof_sha],
        ),
    )?;
    tagged_sql(
        ArchiveSqlOrigin::PrivateOutput,
        plan.execute(
            "INSERT INTO archive_copies VALUES(?1,?2,?3)",
            params![intent.operation_id.0, project_id, proof_sha],
        ),
    )?;
    crate::archive_governed::plan_objects(input, bundle, plan, budget)?;
    let mut planned_bundle = crate::archive_metadata::copy_metadata(
        plan,
        None,
        &project_id,
        ArchiveSqlOrigin::PrivateOutput,
        budget,
    )?;
    boundary::output(read_lineage_bounded(
        plan,
        None,
        &project_id,
        &mut planned_bundle,
        false,
        budget,
    ))?;
    let object_metadata = bundle
        .totals
        .metadata
        .checked_sub(inherited.totals.metadata)
        .ok_or_else(|| invalid("Archive metadata accounting underflow"))?;
    archive::add(
        &mut planned_bundle.totals.metadata,
        object_metadata,
        archive::ARCHIVE_METADATA_BYTES,
        "planned copy metadata including objects",
    )?;
    let receipt = proof.metadata_receipt(false, budget)?;
    budget.checkpoint()?;
    Ok(BoundedCopyPlan {
        project_id,
        receipt,
        object_metadata,
    })
}

/// Reserve an owned immutable input copy so validation and publication consume
/// exactly the fingerprinted bytes even if the acquisition path is replaced.
pub(super) fn acquire(
    path: &Path,
    stage: &Path,
    expected: Option<&str>,
) -> Result<(PathBuf, u64, String)> {
    let mut source = File::open(path)?;
    let metadata = source.metadata()?;
    ensure(
        metadata.is_file()
            && metadata.len() > 0
            && metadata.len() <= archive::MAX_ARCHIVE_FILE_BYTES,
        "Archive acquisition requires a bounded regular file",
    )?;
    let staged = stage.join("source-archive.sqlite3");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged)?;
    let mut buffer = [0; 64 * 1024];
    let mut length = 0;
    let mut hash = Sha256::new();
    loop {
        let size = source.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        archive::add(
            &mut length,
            size as u64,
            archive::MAX_ARCHIVE_FILE_BYTES,
            "acquired archive bytes",
        )?;
        hash.update(&buffer[..size]);
        file.write_all(&buffer[..size])?;
    }
    let sha = format!("{:x}", hash.finalize());
    ensure(
        length == metadata.len() && expected.is_none_or(|expected| expected == sha),
        "Archive changed since inspection or while being acquired",
    )?;
    file.sync_all()?;
    archive::sync_directory(stage)?;
    Ok((staged, length, sha))
}

impl NativeStore {
    /// A fingerprint and bounded header summary. This is not complete restore
    /// validation. It uses a private disk snapshot, then removes it on success.
    /// This compatibility wrapper discards post-completion private cleanup
    /// warnings; use `inspect_archive_outcome` when those warnings are needed.
    pub fn inspect_archive(&self, path: &Path) -> Result<ArchiveInspection> {
        self.inspect_archive_outcome(path)
            .map(|outcome| outcome.value)
            .map_err(|failure| StoreError::Archive(Box::new(failure)))
    }

    pub fn inspect_archive_outcome(
        &self,
        path: &Path,
    ) -> std::result::Result<ArchiveOutcome<ArchiveInspection>, ArchiveFailure> {
        let mut context = ArchiveContext::new(ArchiveAction::Inspect, path);
        context.source = Some(path.to_owned());
        let result = (|| -> Result<_> {
            let _writer = crate::retained_file_coordination::writer_with_file(&self.root, path)?;
            let stage = archive::private_directory(&self.root, "archive-inspect")?;
            context.staging = Some(stage.clone());
            let (input, byte_length, sha256) = acquire(path, &stage, None)?;
            context.archive_sha256 = Some(sha256.clone());
            let archive = boundary::input(archive::readonly(&input))?;
            boundary::input(archive::validate_schema(&archive))?;
            let meta = boundary::input(archive::load_meta(&archive))?;
            let project = boundary::input(load_document(&archive, &meta.project_id))?;
            boundary::input(archive::validate_snapshot_hashes(
                &archive,
                &meta.project_id,
            ))?;
            let result = ArchiveInspection {
                sha256,
                byte_length,
                project_id: project.id().into(),
                name: project.name().into(),
                track_count: project.tracks().len(),
                history_rows: meta.totals.edits,
                operation_rows: meta.totals.operations,
                asset_count: meta.totals.asset_count,
                take_count: meta.totals.take_count,
            };
            drop(archive);
            context.phase = ArchivePhase::ReadOnlyCompleted;
            let mut outcome = ArchiveOutcome::new(result, ArchiveCompletion::ReadOnlyCompleted);
            outcome.cleanup_tree(&stage);
            Ok(outcome)
        })();
        result.map_err(|error| ArchiveFailure::new(context, error))
    }

    /// Compatibility wrapper which discards post-completion private cleanup
    /// warnings. A returned receipt still denotes completion, never a retry.
    pub fn restore_archive_as_copy(
        &mut self,
        path: &Path,
        intent: &RestoreCopyIntent,
    ) -> Result<RestoreCopyReceipt> {
        self.restore_archive_as_copy_outcome(path, intent)
            .map(|outcome| outcome.value)
            .map_err(|failure| StoreError::Archive(Box::new(failure)))
    }

    /// Acquire, plan and authenticate a fresh typed copy with one cumulative
    /// governor. Retained projection metadata is accepted only after the held
    /// originals prove every input, planned and final retained snapshot.
    pub fn restore_archive_as_copy_bounded_outcome(
        &mut self,
        path: &Path,
        intent: &RestoreCopyIntent,
        budget: &InspectionBudget,
    ) -> std::result::Result<ArchiveOutcome<RestoreCopyReceipt>, ArchiveFailure> {
        // The failure envelope retains the store's trusted canonical root.
        // Untrusted source/intent copies are populated only after their credit.
        let mut context = ArchiveContext::new(ArchiveAction::Copy, &self.root);
        let result = (|| -> Result<_> {
            budget.checkpoint()?;
            intent.validate()?;
            ensure(
                path.as_os_str().as_encoded_bytes().len() <= 4096
                    && self.root.as_os_str().as_encoded_bytes().len() <= 4096,
                "Bounded archive copy paths exceed their byte ceiling",
            )?;
            budget.charge_work(
                ((path.as_os_str().as_encoded_bytes().len()
                    + self.root.as_os_str().as_encoded_bytes().len()
                    + intent.operation_id.0.len()
                    + intent.expected_archive_sha256.len()) as u64)
                    .saturating_mul(16)
                    .saturating_add(32 * 1024),
            )?;
            let root = self.root.clone();
            context.destination = root.clone();
            context.source = Some(path.to_owned());
            context.operation_id = Some(intent.operation_id.0.clone());
            context.expected_archive_sha256 = Some(intent.expected_archive_sha256.clone());
            let _owned_files = crate::retained_file_coordination::writer(&root)?;
            let tx = boundary::live(CatalogTransaction::deferred(&self.connection, budget))?;
            let existing = boundary::live(tx.run(|tx| {
                let existing = replay_bounded(tx, &root, intent, budget, &mut context)?;
                tx.rollback()?;
                Ok(existing)
            }))?;
            budget.checkpoint()?;
            if let Some(receipt) = existing {
                context.phase = ArchivePhase::RecordedReplay;
                return Ok(ArchiveOutcome::new(
                    receipt,
                    ArchiveCompletion::RecordedReplay,
                ));
            }
            super::source_attachment::reject_reserved_operation(&intent.operation_id.0)?;
            crate::projection_publication_types::reject_reserved_operation(&intent.operation_id.0)?;
            let _writer = crate::retained_file_coordination::writer_with_file(&root, path)?;
            copy_step(Some(budget))?;
            let stage = archive::private_directory(&root, "archive-copy")?;
            context.staging = Some(stage.clone());
            let diagnostic = serde_json::json!({"mode":"copy","operationId":intent.operation_id.0,
                "expectedArchiveSha256":intent.expected_archive_sha256,"sourcePath":path,"destinationRoot":root});
            let diagnostic = bounded_encoding(&diagnostic, 16 * 1024, budget)?;
            let mut report = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(stage.join("restore.json"))?;
            report.write_all(diagnostic.as_bytes())?;
            report.sync_all()?;
            drop(report);
            let (input, byte_length, sha256) = crate::archive_governed::acquire(
                path,
                &stage,
                Some(&intent.expected_archive_sha256),
                budget,
            )?;
            context.archive_sha256 = Some(sha256.clone());
            let input = crate::archive_governed::open_input(&input, budget)?;
            let mut input_sql =
                crate::archive_governed::PrivateSql::new(&input, budget, ArchiveSqlOrigin::Input)?;
            // Fixed known schema has at most 3*14 CASE-bounded rows; reserve
            // their metadata before the compatibility structural checker reads.
            budget.visit_rows(64)?;
            budget.charge_work(64 * (8192 + 1024) * 4)?;
            boundary::input(archive::validate_schema(&input))?;
            copy_step(Some(budget))?;
            let version: u32 = tagged_sql(
                ArchiveSqlOrigin::Input,
                input.pragma_query_value(None, "user_version", |row| row.get(0)),
            )?;
            budget.visit_rows(16)?;
            budget.charge_work(64 * 1024)?;
            let meta = boundary::input(archive::load_meta(&input))?;
            boundary::input(validate_snapshot_hashes_bounded(
                &input,
                &meta.project_id,
                budget,
            ))?;
            let mut bundle = crate::archive_metadata::copy_metadata(
                &input,
                None,
                &meta.project_id,
                ArchiveSqlOrigin::Input,
                budget,
            )?;
            let ancestors = boundary::input(read_lineage_bounded(
                &input,
                None,
                &meta.project_id,
                &mut bundle,
                true,
                budget,
            ))?;
            ensure(
                ancestors.len() < MAX_LINEAGE_PROOFS,
                "Copy ancestry would exceed its proof ceiling",
            )?;
            crate::archive_governed::reconstruct(&input, &stage, &mut bundle, budget)?;
            ensure(
                bundle.totals == meta.totals && bundle.index_digest() == meta.index,
                "Archive metadata, payload totals or canonical index mismatch",
            )?;
            let (input_walk, source_set, _input_assets) = crate::archive_projection::collect(
                &input,
                crate::archive_projection::RetainedLayout::Osar { version },
                &meta.project_id,
                ArchiveSqlOrigin::Input,
                budget,
            )?
            .into_parts();

            let tx = boundary::live(CatalogTransaction::immediate(&mut self.connection, budget))?;
            let (receipt, completion) = boundary::live(tx.run(|tx| {
                if let Some(receipt) = replay_bounded(tx, &root, intent, budget, &mut context)? {
                    input_sql.detach()?;
                    tx.rollback()?;
                    budget.checkpoint()?;
                    context.phase = ArchivePhase::RecordedReplay;
                    return Ok((receipt, ArchiveCompletion::RecordedReplay));
                }
                let ledger = PostimageLedger::new(&stage, Layout::Catalog, Some(budget))?;
                ledger.capture_baseline(tx, ArchiveSqlOrigin::LiveCatalog)?;
                let plan = crate::archive_governed::prepare_plan(&stage, budget)?;
                let mut plan_sql = crate::archive_governed::PrivateSql::new(
                    &plan,
                    budget,
                    ArchiveSqlOrigin::PrivateOutput,
                )?;
                let planned = plan_copy_bounded(
                    tx,
                    &input,
                    &plan,
                    BoundedCopyInput {
                        bundle: &bundle,
                        source_project: &meta.project_id,
                        ancestors,
                        intent,
                        byte_length,
                        sha256: &sha256,
                    },
                    budget,
                )?;
                let (plan_walk, _plan_sources, _plan_assets) = crate::archive_projection::collect(
                    &plan,
                    crate::archive_projection::RetainedLayout::Catalog,
                    &planned.project_id,
                    ArchiveSqlOrigin::PrivateOutput,
                    budget,
                )?
                .into_parts();
                ledger.expect_from_plan(
                    &plan,
                    ArchiveSqlOrigin::PrivateOutput,
                    crate::archive_plan::REUSABLE_TABLES,
                )?;
                let expected = ledger.freeze()?;
                tagged_sql(
                    ArchiveSqlOrigin::PrivateOutput,
                    plan.execute_batch("PRAGMA query_only=ON"),
                )?;
                #[cfg(test)]
                boundary::checkpoint(boundary::TestEvent::CopyPlanFrozen, &context)?;

                context.phase = ArchivePhase::ObjectsPlacementAttempted;
                crate::archive_governed::install(
                    tx,
                    &input,
                    &root,
                    &stage,
                    &bundle,
                    &mut context.published_objects,
                    budget,
                )?;
                context.phase = ArchivePhase::ObjectsPlaced;
                // The owner is acquired only after tentative ordinary object
                // placement/registration, and remains held through COMMIT.
                let sources =
                    crate::projection_sources::RegisteredProjectionSources::open_collected_from(
                        &root,
                        tx,
                        source_set,
                        ArchiveSqlOrigin::LiveCatalog,
                    )?;
                let proofs = sources.authenticate(budget)?;
                let mut validator =
                    crate::archive_projection::RetainedValidator::new(&proofs, budget)?;
                validator.validate(&input, ArchiveSqlOrigin::Input, &input_walk)?;
                validator.validate(&plan, ArchiveSqlOrigin::PrivateOutput, &plan_walk)?;
                crate::archive_metadata::copy_metadata(
                    &plan,
                    Some(archive::Destination::Store(tx)),
                    &planned.project_id,
                    ArchiveSqlOrigin::PrivateOutput,
                    budget,
                )?;
                let mut applied = crate::archive_metadata::copy_metadata(
                    &plan,
                    None,
                    &planned.project_id,
                    ArchiveSqlOrigin::PrivateOutput,
                    budget,
                )?;
                boundary::output(read_lineage_bounded(
                    &plan,
                    Some((tx, &planned.project_id, true, ArchiveSqlOrigin::LiveCatalog)),
                    &planned.project_id,
                    &mut applied,
                    false,
                    budget,
                ))?;
                let mut final_bundle = crate::archive_metadata::copy_metadata(
                    tx,
                    None,
                    &planned.project_id,
                    ArchiveSqlOrigin::LiveCatalog,
                    budget,
                )?;
                boundary::live(read_lineage_bounded(
                    tx,
                    None,
                    &planned.project_id,
                    &mut final_bundle,
                    false,
                    budget,
                ))?;
                archive::add(
                    &mut final_bundle.totals.metadata,
                    planned.object_metadata,
                    archive::ARCHIVE_METADATA_BYTES,
                    "copy metadata including objects",
                )?;
                let (final_walk, _final_sources, final_assets) =
                    crate::archive_projection::collect(
                        tx,
                        crate::archive_projection::RetainedLayout::Catalog,
                        &planned.project_id,
                        ArchiveSqlOrigin::LiveCatalog,
                        budget,
                    )?
                    .into_parts();
                validator.validate(tx, ArchiveSqlOrigin::LiveCatalog, &final_walk)?;
                expected.verify(tx, ArchiveSqlOrigin::LiveCatalog)?;
                crate::archive_governed::verify_assets(
                    tx,
                    &final_assets,
                    ArchiveSqlOrigin::LiveCatalog,
                    budget,
                )?;
                validator.verify_final(tx, ArchiveSqlOrigin::LiveCatalog)?;
                // No substantive private SQL or fallible result construction
                // follows the final live fence, and COMMIT owns late stops.
                plan_sql.detach()?;
                input_sql.detach()?;
                budget.checkpoint()?;
                context.phase = ArchivePhase::CatalogCommitAttempted;
                #[cfg(test)]
                boundary::checkpoint(boundary::TestEvent::CopyCommit, &context)?;
                // Re-prove held originals after the last observable boundary;
                // this is metadata/identity only, never a second source proof.
                validator.verify_final(tx, ArchiveSqlOrigin::LiveCatalog)?;
                tx.commit()?;
                context.phase = ArchivePhase::CatalogCommitted;
                Ok((planned.receipt, ArchiveCompletion::CatalogCommitted))
            }))?;
            drop(input_sql);
            drop(input);
            let mut outcome = ArchiveOutcome::new(receipt, completion);
            outcome.cleanup_tree(&stage);
            Ok(outcome)
        })();
        result.map_err(|error| crate::archive_governed::finish_failure(context, error, budget))
    }

    pub fn restore_archive_as_copy_outcome(
        &mut self,
        path: &Path,
        intent: &RestoreCopyIntent,
    ) -> std::result::Result<ArchiveOutcome<RestoreCopyReceipt>, ArchiveFailure> {
        let root = self.root.clone();
        let mut context = ArchiveContext::new(ArchiveAction::Copy, &root);
        context.source = Some(path.to_owned());
        let result = (|| -> Result<_> {
            intent.validate()?;
            context.operation_id = Some(intent.operation_id.0.clone());
            context.expected_archive_sha256 = Some(intent.expected_archive_sha256.clone());
            let _owned_files = crate::retained_file_coordination::writer(&root)?;
            // Exact replay precedes input acquisition and identity allocation.
            // Its source-authenticated receipt is built before explicit rollback.
            let tx = boundary::live(CatalogTransaction::legacy_deferred(&self.connection))?;
            let existing = boundary::live(tx.run(|tx| {
                let existing = boundary::live(replay(tx, &root, intent))?;
                tx.rollback()?;
                Ok(existing)
            }))?;
            if let Some(receipt) = existing {
                context.phase = ArchivePhase::RecordedReplay;
                return Ok(ArchiveOutcome::new(
                    receipt,
                    ArchiveCompletion::RecordedReplay,
                ));
            }
            super::source_attachment::reject_reserved_operation(&intent.operation_id.0)?;
            crate::projection_publication_types::reject_reserved_operation(&intent.operation_id.0)?;
            let _writer = crate::retained_file_coordination::writer_with_file(&root, path)?;
            let stage = archive::private_directory(&root, "archive-copy")?;
            context.staging = Some(stage.clone());
            let diagnostic = serde_json::json!({"mode":"copy","operationId":intent.operation_id.0,"expectedArchiveSha256":intent.expected_archive_sha256,"sourcePath":path,"destinationRoot":root});
            let mut report = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(stage.join("restore.json"))?;
            report.write_all(
                &serde_json::to_vec_pretty(&diagnostic).map_err(|e| invalid(e.to_string()))?,
            )?;
            report.sync_all()?;
            drop(report);
            let (input, byte_length, sha256) =
                acquire(path, &stage, Some(&intent.expected_archive_sha256))?;
            context.archive_sha256 = Some(sha256.clone());
            let archive = boundary::input(archive::readonly(&input))?;
            boundary::input(archive::validate_schema(&archive))?;
            let meta = boundary::input(archive::load_meta(&archive))?;
            boundary::input(archive::validate_snapshot_hashes(
                &archive,
                &meta.project_id,
            ))?;
            let mut bundle =
                boundary::input(archive::copy_metadata(&archive, None, &meta.project_id))?;
            let ancestors = boundary::input(read_lineage(
                &archive,
                None,
                &meta.project_id,
                &mut bundle,
                true,
            ))?;
            ensure(
                ancestors.len() < MAX_LINEAGE_PROOFS,
                "Copy would exceed its ancestry proof ceiling",
            )?;
            boundary::input(archive::restore_object_files(&archive, &stage, &mut bundle))?;
            ensure(
                bundle.totals == meta.totals && bundle.index_digest() == meta.index,
                "Archive closure differs from its receipt",
            )?;
            boundary::input(archive::validate_all_reference_snapshots(
                &archive,
                &meta.project_id,
            ))?;
            let tx = boundary::live(CatalogTransaction::legacy_immediate(&mut self.connection))?;
            let (receipt,completion)=boundary::live(tx.run(|tx| {
            // Another connection may have completed this same request meanwhile.
            if let Some(receipt) = boundary::live(replay(tx, &root, intent))? {
                tx.rollback()?;
                context.phase=ArchivePhase::RecordedReplay;
                return Ok((receipt,ArchiveCompletion::RecordedReplay));
            }
            // The complete target rows are planned independently while the
            // live transaction is still read-only. No destination trigger or
            // mutable live pending table can author the expected postimage.
            let ledger=PostimageLedger::new(&stage,Layout::Catalog,None)?;
            ledger.capture_baseline(tx,ArchiveSqlOrigin::LiveCatalog)?;
            let plan=crate::archive_plan::open_catalog(&stage)?;
            let project_id = fresh_project(tx, &meta.project_id)?;
            let current_json = document_json(&bundle.project)?;
            let document = remap_document(&current_json, &project_id)?;
            let cursor: i64 = tagged_sql(ArchiveSqlOrigin::Input,archive.query_row(
                "SELECT cursor FROM projects WHERE id=?1",
                [&meta.project_id],
                |r| r.get(0),
            ))?;
            tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute(
                "INSERT INTO projects(id,payload,revision,cursor) VALUES(?1,?2,?3,?4)",
                params![
                    project_id,
                    document_json(&document)?,
                    document.revision() as i64,
                    cursor
                ],
            ))?;
            let mut history=tagged_sql(ArchiveSqlOrigin::Input,archive.prepare("SELECT sequence,CASE WHEN octet_length(before_json)<=?2 THEN before_json END,CASE WHEN octet_length(after_json)<=?2 THEN after_json END FROM edits WHERE project_id=?1 ORDER BY sequence"))?;
            let mut rows =
                tagged_sql(ArchiveSqlOrigin::Input,history.query(params![meta.project_id, MAX_NATIVE_PROJECT_BYTES as i64]))?;
            while let Some(row) = tagged_sql(ArchiveSqlOrigin::Input,rows.next())? {
                let sequence: i64 = tagged_sql(ArchiveSqlOrigin::Input,row.get(0))?;
                let before: String = tagged_sql(ArchiveSqlOrigin::Input,row.get(1))?;
                let after: String = tagged_sql(ArchiveSqlOrigin::Input,row.get(2))?;
                tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute("INSERT INTO edits(project_id,sequence,before_json,after_json) VALUES(?1,?2,?3,?4)",params![project_id,sequence,document_json(&remap_document(&before,&project_id)?)?,document_json(&remap_document(&after,&project_id)?)?]))?;
            }
            drop(rows);
            drop(history);
            let original:Option<(String,Vec<u8>)>=tagged_sql(ArchiveSqlOrigin::Input,archive.query_row("SELECT source_sha256,CASE WHEN octet_length(original_json)<=?2 THEN original_json END FROM imports WHERE project_id=?1",params![meta.project_id,omniusong_core::MAX_PROJECT_BYTES as i64],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
            if let Some((sha, bytes)) = original {
                tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute(
                    "INSERT INTO imports VALUES(?1,?2,?3)",
                    params![project_id, sha, bytes],
                ))?;
            }
            // Stage bounded mapping rows in a private SQLite table. This keeps a
            // 100k-operation copy from collecting every receipt in memory.
            tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute_batch("CREATE TEMP TABLE pending_copy_map(source_operation_id TEXT PRIMARY KEY,destination_operation_id TEXT UNIQUE,source_request_sha256 TEXT,source_result_sha256 TEXT,destination_request_sha256 TEXT,destination_result_sha256 TEXT);"))?;
            let mut statement=tagged_sql(ArchiveSqlOrigin::Input,archive.prepare("SELECT operation_id,CASE WHEN octet_length(request_json)<=?2 THEN request_json END,request_sha256,previous_revision,committed_revision,CASE WHEN octet_length(result_json)<=?3 THEN result_json END,result_sha256 FROM operations WHERE project_id=?1 ORDER BY operation_id"))?;
            let mut rows = tagged_sql(ArchiveSqlOrigin::Input,statement.query(params![
                meta.project_id,
                WORKER_MAX_BYTES as i64,
                MAX_NATIVE_PROJECT_BYTES as i64
            ]))?;
            let mut mapping_count = 0;
            let mut mapping_digest = mapping_hash();
            while let Some(row) = tagged_sql(ArchiveSqlOrigin::Input,rows.next())? {
                let source_id: String = tagged_sql(ArchiveSqlOrigin::Input,row.get(0))?;
                let request: String = tagged_sql(ArchiveSqlOrigin::Input,row.get(1))?;
                let source_request: String = tagged_sql(ArchiveSqlOrigin::Input,row.get(2))?;
                let previous: i64 = tagged_sql(ArchiveSqlOrigin::Input,row.get(3))?;
                let revision: i64 = tagged_sql(ArchiveSqlOrigin::Input,row.get(4))?;
                let result: String = tagged_sql(ArchiveSqlOrigin::Input,row.get(5))?;
                let source_result: String = tagged_sql(ArchiveSqlOrigin::Input,row.get(6))?;
                let (operation, request) = mapped_copy_request_in(
                    CopyCatalogs{live:tx,input:&archive,pending:&plan,pending_origin:ArchiveSqlOrigin::PrivateOutput},
                    &request,
                    &project_id,
                    &intent.operation_id.0,
                )?;
                let result_doc = remap_document(&result, &project_id)?;
                let result = document_json(&result_doc)?;
                archive::validate_request(
                    &request,
                    &operation,
                    &project_id,
                    archive::unsigned(previous, "copy operation revision")?,
                    &result_doc,
                )?;
                let mapping = Mapping {
                    source: source_id,
                    destination: operation,
                    source_request,
                    source_result,
                    destination_request: checksum(request.as_bytes()),
                    destination_result: checksum(result.as_bytes()),
                };
                mapping.validate()?;
                hash_mapping(&mut mapping_digest, &mapping);
                mapping_count += 1;
                tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute(
                    "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        mapping.destination,
                        project_id,
                        request,
                        mapping.destination_request,
                        previous,
                        revision,
                        result,
                        mapping.destination_result
                    ],
                ))?;
                tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute(
                    "INSERT INTO pending_copy_map VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        mapping.source,
                        mapping.destination,
                        mapping.source_request,
                        mapping.source_result,
                        mapping.destination_request,
                        mapping.destination_result
                    ],
                ))?;
            }
            drop(rows);
            drop(statement);
            let mut inherited = boundary::input(archive::copy_metadata(&archive, None, &meta.project_id))?;
            boundary::input(read_lineage(
                &archive,
                Some((&plan, &project_id, false,ArchiveSqlOrigin::PrivateOutput)),
                &meta.project_id,
                &mut inherited,
                true,
            ))?;
            let proof = Proof {
                version: 1,
                operation_id: intent.operation_id.clone(),
                source_archive_sha256: sha256,
                source_archive_byte_length: byte_length,
                source_project_id: meta.project_id.clone(),
                destination_project_id: project_id.clone(),
                mapping_count,
                mapping_sha256: format!("{:x}", mapping_digest.finalize()),
                ancestors: ancestors.into_iter().collect(),
                result: document.value().clone(),
                result_sha256: checksum(&document.to_json()?),
            };
            let json = proof.json()?;
            let proof_sha = checksum(json.as_bytes());
            tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute(
                "INSERT INTO archive_copy_proofs VALUES(?1,?2)",
                params![proof_sha, json],
            ))?;
            tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute("INSERT INTO archive_copy_proof_operations SELECT ?1,source_operation_id,destination_operation_id,source_request_sha256,source_result_sha256,destination_request_sha256,destination_result_sha256 FROM pending_copy_map",[&proof_sha]))?;
            tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute_batch("DROP TABLE pending_copy_map"))?;
            tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute(
                "INSERT INTO archive_project_lineage VALUES(?1,?2)",
                params![project_id, proof_sha],
            ))?;
            tagged_sql(ArchiveSqlOrigin::PrivateOutput,plan.execute(
                "INSERT INTO archive_copies VALUES(?1,?2,?3)",
                params![intent.operation_id.0, project_id, proof_sha],
            ))?;
            crate::archive_plan::plan_objects(&archive,&bundle,&plan)?;
            let mut planned_bundle=boundary::output(archive::copy_metadata(&plan,None,&project_id))?;
            boundary::output(read_lineage(&plan,None,&project_id,&mut planned_bundle,false))?;
            boundary::output(archive::validate_all_reference_snapshots(&plan,&project_id))?;
            let object_metadata = bundle.totals.metadata.checked_sub(inherited.totals.metadata)
                .ok_or_else(||invalid("Archive metadata accounting underflow"))?;
            archive::add(&mut planned_bundle.totals.metadata,object_metadata,archive::ARCHIVE_METADATA_BYTES,
                "planned copy metadata including objects")?;
            // Freeze both the exact planned rows and the full receipt before
            // the first live write or final-path object placement.
            let receipt=proof.receipt(false)?;
            ledger.expect_from_plan(&plan,ArchiveSqlOrigin::PrivateOutput,crate::archive_plan::REUSABLE_TABLES)?;
            let expected=ledger.freeze()?;
            boundary::output((|| -> Result<()> {plan.execute_batch("PRAGMA query_only=ON")?;Ok(())})())?;
            #[cfg(test)]
            boundary::checkpoint(boundary::TestEvent::CopyPlanFrozen,&context)?;
            context.phase=ArchivePhase::ObjectsPlacementAttempted;
            archive::install_objects(tx,&archive,&root,&stage,&bundle,&mut context.published_objects)?;
            context.phase=ArchivePhase::ObjectsPlaced;
            boundary::output(archive::copy_metadata(&plan,Some(archive::Destination::Store(tx)),&project_id))?;
            let mut applied=boundary::output(archive::copy_metadata(&plan,None,&project_id))?;
            boundary::output(read_lineage(&plan,Some((tx,&project_id,true,ArchiveSqlOrigin::LiveCatalog)),
                &project_id,&mut applied,false))?;
            // Validate the final copy's own budgets, lineage and references before
            // the sole publication commit. Its new proof may push it over a cap.
            let mut final_bundle = archive::copy_metadata(tx, None, &project_id)?;
            read_lineage(tx, None, &project_id, &mut final_bundle, false)?;
            archive::add(
                &mut final_bundle.totals.metadata,
                object_metadata,
                archive::ARCHIVE_METADATA_BYTES,
                "copy metadata including objects",
            )?;
            validate_owned_closure(tx, &root, &final_bundle)?;
            expected.verify(tx,ArchiveSqlOrigin::LiveCatalog)?;
            context.phase=ArchivePhase::CatalogCommitAttempted;
            #[cfg(test)]
            boundary::checkpoint(boundary::TestEvent::CopyCommit,&context)?;
            tx.commit()?;
            context.phase=ArchivePhase::CatalogCommitted;
            Ok((receipt,ArchiveCompletion::CatalogCommitted))
            }))?;
            drop(archive);
            let mut outcome = ArchiveOutcome::new(receipt, completion);
            outcome.cleanup_tree(&stage);
            Ok(outcome)
        })();
        result.map_err(|error| ArchiveFailure::new(context, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::contracts::{EditOperation, EditScope, Extensions, ProjectEditIntent};
    use serde_json::json;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/archive-copy-owned")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn store(&self, name: &str) -> NativeStore {
            NativeStore::open(self.0.join(name)).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!("Copy fixture retained: {}", self.0.display());
            } else {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn edit(doc: &ProjectDocument, id: &str) -> ProjectEditIntent {
        ProjectEditIntent {
            operation_id: OperationId(id.into()),
            project_id: ProjectId(doc.id().into()),
            expected_revision: doc.revision(),
            scope: EditScope::Project,
            edit: EditOperation::UpdateSession {
                name: "Edited session".into(),
                tempo: 117.5,
            },
            extensions: Extensions::new(),
        }
    }
    fn intent(sha: &str, id: &str) -> RestoreCopyIntent {
        RestoreCopyIntent {
            operation_id: OperationId(id.into()),
            expected_archive_sha256: sha.into(),
        }
    }
    fn fixture_archive(f: &Fixture) -> (NativeStore, ProjectDocument, PathBuf, String) {
        let mut store = f.store("source");
        let mut value = ProjectDocument::new("Source").unwrap().value().clone();
        value["opaque"] = json!({"project_id":value["id"],"operation_id":"edit-source","takeId":"do-not-remap","nested":{"id":value["id"]}});
        value["precision"]=serde_json::from_str("{\"huge\":1234567890123456789012345678901234567890,\"decimal\":0.123456789012345678901234567890}").unwrap();
        let doc = store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap();
        let mut request = edit(&doc, "edit-source");
        request
            .extensions
            .insert("kind".into(), json!("take_publication"));
        request.extensions.insert("version".into(), json!(1));
        request.extensions.insert(
            "intent".into(),
            json!({"project_id":doc.id(),"operation_id":"edit-source"}),
        );
        request
            .extensions
            .insert("projection_id".into(), json!("opaque-projection"));
        request.extensions.insert(
            "request".into(),
            json!({"source":{"take_id":"opaque-original"}}),
        );
        request.extensions.insert(
            "lanes".into(),
            json!([{"trackId":"opaque-lane","noteId":"opaque-note"}]),
        );
        let doc = store.apply_edit(&request).unwrap().document;
        let path = f.0.join("source.omniusong");
        let receipt = store.export_archive(doc.id(), &path).unwrap();
        (store, doc, path, receipt.sha256)
    }
    #[test]
    fn opaque_projection_named_edit_fields_survive_public_export_identity_restore_and_copy() {
        let f = Fixture::new();
        let (source, project, path, sha) = fixture_archive(&f);
        let original: String = source
            .connection
            .query_row(
                "SELECT request_json FROM operations WHERE operation_id='edit-source'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let original_intent: ProjectEditIntent = serde_json::from_str(&original).unwrap();
        assert_eq!(
            original_intent.extensions["projection_id"],
            "opaque-projection"
        );
        let mut identity = f.store("identity");
        assert_eq!(identity.restore_archive(&path).unwrap(), project);
        let restored: String = identity
            .connection
            .query_row(
                "SELECT request_json FROM operations WHERE operation_id='edit-source'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(restored, original);
        let mut copied = f.store("copy");
        let receipt = copied
            .restore_archive_as_copy(&path, &intent(&sha, "copy-opaque"))
            .unwrap();
        let raw: String = copied
            .connection
            .query_row(
                "SELECT request_json FROM operations WHERE project_id=?1",
                [receipt.document.id()],
                |r| r.get(0),
            )
            .unwrap();
        let mapped: ProjectEditIntent = serde_json::from_str(&raw).unwrap();
        assert_eq!(mapped.extensions, original_intent.extensions);
        assert_ne!(mapped.operation_id, original_intent.operation_id);
        assert_eq!(mapped.project_id.0, receipt.document.id());
        assert!(matches!(
            crate::archive_operation::ArchiveOperation::parse(&raw).unwrap(),
            crate::archive_operation::ArchiveOperation::Edit(_)
        ));
        assert!(copied.apply_edit(&mapped).unwrap().replayed);
        let second = f.0.join("second.osar");
        copied
            .export_archive(receipt.document.id(), &second)
            .unwrap();
        let mut reopened = f.store("reopened");
        assert_eq!(reopened.restore_archive(&second).unwrap(), receipt.document);
        assert_eq!(source.load(project.id()).unwrap(), project);
        let untouched: String = source
            .connection
            .query_row(
                "SELECT request_json FROM operations WHERE operation_id='edit-source'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(untouched, original);
    }

    #[test]
    fn copy_remaps_owned_ids_only_and_replays_without_source_after_later_edit() {
        let f = Fixture::new();
        let (mut store, source, path, sha) = fixture_archive(&f);
        let inspection = store.inspect_archive(&path).unwrap();
        assert_eq!(inspection.sha256, sha);
        assert_eq!(inspection.project_id, source.id());
        let request = intent(&sha, "copy-one");
        let receipt = store.restore_archive_as_copy(&path, &request).unwrap();
        assert!(!receipt.replayed);
        assert_ne!(receipt.document.id(), source.id());
        let mut expected = source.value().clone();
        expected["id"] = receipt.document.id().into();
        assert_eq!(receipt.document.value(), &expected);
        assert_eq!(store.load(source.id()).unwrap(), source);
        let mapped: String = store
            .connection
            .query_row(
                "SELECT request_json FROM operations WHERE project_id=?1",
                [receipt.document.id()],
                |r| r.get(0),
            )
            .unwrap();
        let mapped: ProjectEditIntent = serde_json::from_str(&mapped).unwrap();
        assert_ne!(mapped.operation_id.0, "edit-source");
        assert_eq!(mapped.project_id.0, receipt.document.id());
        assert_eq!(mapped.extensions["intent"]["project_id"], source.id());
        let replay = store.apply_edit(&mapped).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.document, receipt.document);
        let mut later = edit(&receipt.document, "later");
        later.edit = EditOperation::Rename {
            name: "After copy".into(),
        };
        let later = store.apply_edit(&later).unwrap().document;
        fs::remove_file(&path).unwrap();
        let repeated = store.restore_archive_as_copy(&path, &request).unwrap();
        assert!(repeated.replayed);
        assert_eq!(repeated.document, receipt.document);
        assert_eq!(store.load(later.id()).unwrap(), later);
        assert!(
            store
                .restore_archive_as_copy(&path, &intent(&"f".repeat(64), "copy-one"))
                .is_err()
        );
        assert_eq!(store.list().unwrap().len(), 2);
    }
    #[test]
    fn copy_of_copy_and_identity_restore_preserve_complete_proof_bytes() {
        let f = Fixture::new();
        let (mut source, _, path, sha) = fixture_archive(&f);
        let first = source
            .restore_archive_as_copy(&path, &intent(&sha, "copy-first"))
            .unwrap();
        let first_archive = f.0.join("first.omniusong");
        source
            .export_archive(first.document.id(), &first_archive)
            .unwrap();
        let (first_proof, first_json): (String, String) = source
            .connection
            .query_row(
                "SELECT proof_sha256,receipt_json FROM archive_copy_proofs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut target = f.store("target");
        let restored = target.restore_archive(&first_archive).unwrap();
        assert_eq!(restored, first.document);
        let retry = target
            .restore_archive_as_copy(Path::new("missing"), &intent(&sha, "copy-first"))
            .unwrap();
        assert!(retry.replayed);
        let inspection = target.inspect_archive(&first_archive).unwrap();
        let second = target
            .restore_archive_as_copy(&first_archive, &intent(&inspection.sha256, "copy-second"))
            .unwrap();
        assert_ne!(second.document.id(), first.document.id());
        let inherited: String = target
            .connection
            .query_row(
                "SELECT receipt_json FROM archive_copy_proofs WHERE proof_sha256=?1",
                [&first_proof],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(inherited, first_json);
        let count: i64 = target
            .connection
            .query_row(
                "SELECT count(*) FROM archive_project_lineage WHERE project_id=?1",
                [second.document.id()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        let second_archive = f.0.join("second.omniusong");
        target
            .export_archive(second.document.id(), &second_archive)
            .unwrap();
        let mut third = f.store("third");
        let document = third.restore_archive(&second_archive).unwrap();
        assert_eq!(document, second.document);
        let live: i64 = third
            .connection
            .query_row("SELECT count(*) FROM archive_copies", [], |r| r.get(0))
            .unwrap();
        assert_eq!(live, 1);
        let proofs: i64 = third
            .connection
            .query_row("SELECT count(*) FROM archive_copy_proofs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(proofs, 2);
        assert!(
            third
                .restore_archive_as_copy(
                    Path::new("missing"),
                    &intent(&inspection.sha256, "copy-second")
                )
                .unwrap()
                .replayed
        );
    }
    #[test]
    fn strict_v1_read_compatibility_and_explicit_v2_schema() {
        let f = Fixture::new();
        let (_, source, path, _) = fixture_archive(&f);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DROP TABLE archive_copies; DROP TABLE archive_project_lineage; DROP TABLE archive_copy_proof_operations; DROP TABLE archive_copy_proofs; PRAGMA user_version=1;").unwrap();
        drop(db);
        let mut target = f.store("target");
        assert_eq!(target.restore_archive(&path).unwrap(), source);
        let inspection = target.inspect_archive(&path).unwrap();
        assert!(
            target
                .restore_archive_as_copy(&path, &intent(&inspection.sha256, "legacy-copy"))
                .is_ok()
        );
        let db = Connection::open(&path).unwrap();
        db.execute_batch(LINEAGE_TABLES[0].1).unwrap();
        drop(db);
        assert!(target.inspect_archive(&path).is_err());
    }
    #[test]
    fn changed_fingerprint_global_conflicts_and_final_sql_failure_publish_nothing() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("target");
        assert!(
            target
                .restore_archive_as_copy(&path, &intent(&"f".repeat(64), "changed"))
                .is_err()
        );
        assert!(target.list().unwrap().is_empty());
        target.connection.execute_batch("CREATE TRIGGER copy_fail BEFORE INSERT ON archive_copies BEGIN SELECT RAISE(ABORT,'copy fixture final failure'); END;").unwrap();
        let error = target
            .restore_archive_as_copy_outcome(&path, &intent(&sha, "retry-copy"))
            .unwrap_err();
        assert_eq!(error.context().phase, ArchivePhase::ObjectsPlaced);
        assert!(!error.publication_uncertain());
        assert!(matches!(error.operation_error(),StoreError::ArchiveSql(sql)
            if sql.origin()==ArchiveSqlOrigin::LiveCatalog));
        assert_eq!(
            error.catalog_sqlite_error_code().map(|code| code & 0xff),
            Some(rusqlite::ffi::SQLITE_CONSTRAINT)
        );
        assert!(error.context().staging.as_ref().unwrap().is_dir());
        for table in [
            "projects",
            "edits",
            "operations",
            "archive_copy_proofs",
            "archive_copy_proof_operations",
            "archive_project_lineage",
            "archive_copies",
        ] {
            assert_eq!(
                archive::count(&target.connection, table).unwrap(),
                0,
                "{table}"
            );
        }
        target
            .connection
            .execute_batch("DROP TRIGGER copy_fail")
            .unwrap();
        let receipt = target
            .restore_archive_as_copy(&path, &intent(&sha, "retry-copy"))
            .unwrap();
        assert!(!receipt.replayed);
        let current = target
            .apply_edit(&edit(&receipt.document, "live-edit-id"))
            .unwrap();
        let error = target
            .restore_archive_as_copy(Path::new("missing"), &intent(&sha, "live-edit-id"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("already an edit"));
        assert_eq!(
            target.load(current.document.id()).unwrap(),
            current.document
        );
        assert_eq!(
            archive::hash_file(&path, archive::MAX_ARCHIVE_FILE_BYTES)
                .unwrap()
                .1,
            sha
        );
    }
    #[test]
    fn concurrent_same_operation_publishes_one_copy_and_one_receipt() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        drop(f.store("target"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut joins = Vec::new();
        for _ in 0..2 {
            let root = f.0.join("target");
            let path = path.clone();
            let request = intent(&sha, "concurrent-copy");
            let barrier = barrier.clone();
            joins.push(std::thread::spawn(move || {
                let mut store = NativeStore::open(root).unwrap();
                barrier.wait();
                store.restore_archive_as_copy(&path, &request).unwrap()
            }));
        }
        let a = joins.remove(0).join().unwrap();
        let b = joins.remove(0).join().unwrap();
        assert_eq!(a.document, b.document);
        assert_ne!(a.replayed, b.replayed);
        let target = f.store("target");
        assert_eq!(target.list().unwrap().len(), 1);
        assert_eq!(
            archive::count(&target.connection, "archive_copies").unwrap(),
            1
        );
    }

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum SqlCell {
        Null,
        Integer(i64),
        Real(u64),
        Text(Vec<u8>),
        Blob(Vec<u8>),
    }
    fn sql_snapshot(connection: &Connection) -> BTreeMap<String, Vec<Vec<SqlCell>>> {
        use rusqlite::types::ValueRef;
        let names: Vec<String> = connection
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut snapshot = BTreeMap::new();
        for name in names
            .into_iter()
            .chain(std::iter::once("sqlite_schema".into()))
        {
            let quoted = format!("\"{}\"", name.replace('"', "\"\""));
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {quoted}"))
                .unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            let mut values = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                values.push(
                    (0..columns)
                        .map(|i| match row.get_ref(i).unwrap() {
                            ValueRef::Null => SqlCell::Null,
                            ValueRef::Integer(v) => SqlCell::Integer(v),
                            ValueRef::Real(v) => SqlCell::Real(v.to_bits()),
                            ValueRef::Text(v) => SqlCell::Text(v.to_vec()),
                            ValueRef::Blob(v) => SqlCell::Blob(v.to_vec()),
                        })
                        .collect::<Vec<_>>(),
                );
            }
            values.sort();
            snapshot.insert(name, values);
        }
        snapshot
    }

    fn assert_exact_postimage_refusal(
        target: &mut NativeStore,
        path: &Path,
        sha: &str,
        operation: &str,
    ) {
        let before = sql_snapshot(&target.connection);
        let input = fs::read(path).unwrap();
        let failure = target
            .restore_archive_as_copy_outcome(path, &intent(sha, operation))
            .unwrap_err();
        assert!(
            matches!(failure.operation_error(),StoreError::Invalid(message)
            if message=="Final SQL rows differ from the frozen expected postimage"),
            "{failure:?}"
        );
        assert_eq!(failure.context().phase, ArchivePhase::ObjectsPlaced);
        assert!(target.connection.is_autocommit());
        assert!(!failure.catalog_connection_unusable());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(path).unwrap(), input);
    }

    #[test]
    fn exact_copy_postimage_refuses_a_valid_changed_current_project() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("changed-current");
        target.connection.execute_batch("CREATE TRIGGER alter_copied_project AFTER INSERT ON projects BEGIN
            UPDATE projects SET payload=json_set(payload,'$.lyrics','valid but unplanned') WHERE id=NEW.id; END;").unwrap();
        assert_exact_postimage_refusal(&mut target, &path, &sha, "copy-current-postimage");
        target
            .connection
            .execute_batch("DROP TRIGGER alter_copied_project")
            .unwrap();
        assert_eq!(
            target
                .restore_archive_as_copy_outcome(&path, &intent(&sha, "copy-current-postimage"))
                .unwrap()
                .completion,
            ArchiveCompletion::CatalogCommitted
        );
    }

    #[test]
    fn exact_copy_postimage_refuses_same_count_unrelated_row_replacement() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("unrelated-swap");
        let original = target
            .create(&ProjectDocument::new("Unrelated retained original").unwrap())
            .unwrap();
        let replacement = ProjectDocument::new("Valid unrelated replacement").unwrap();
        let literal = document_json(&replacement).unwrap().replace('\'', "''");
        target
            .connection
            .execute_batch(&format!(
                "CREATE TRIGGER replace_unrelated AFTER INSERT ON projects WHEN NEW.id!='{}' BEGIN
            DELETE FROM projects WHERE id='{}'; INSERT INTO projects(id,payload,revision,cursor)
            VALUES('{}','{}',0,0); END;",
                replacement.id(),
                original.id(),
                replacement.id(),
                literal
            ))
            .unwrap();
        assert_exact_postimage_refusal(&mut target, &path, &sha, "copy-unrelated-postimage");
        assert_eq!(target.load(original.id()).unwrap(), original);
        assert!(target.load(replacement.id()).is_err());
    }

    #[test]
    fn exact_copy_postimage_refuses_unplanned_local_recording_intent() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("local-intent");
        let digest = checksum(b"{}");
        target
            .connection
            .execute_batch(&format!(
                "CREATE TRIGGER inject_local_intent AFTER INSERT ON projects BEGIN
            INSERT INTO recording_intents(id,revision,payload,payload_sha256)
            VALUES('11111111111111111111111111111111',1,X'7b7d','{digest}'); END;"
            ))
            .unwrap();
        assert_exact_postimage_refusal(&mut target, &path, &sha, "copy-local-intent-postimage");
        assert!(target.list_performance_intents().unwrap().is_empty());
    }

    #[test]
    fn frozen_copy_postimage_refuses_coherently_rewritten_private_plan_mapping() {
        use std::{cell::Cell, rc::Rc};
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("private-plan-rewrite");
        let before = sql_snapshot(&target.connection);
        let input = fs::read(&path).unwrap();
        let attacked = Rc::new(Cell::new(false));
        let observed = Rc::clone(&attacked);
        let failure=boundary::with_test_hook(Box::new(move |event,context| {
            if event!=boundary::TestEvent::CopyPlanFrozen {return Ok(());}
            assert_eq!(context.phase,ArchivePhase::NotStarted);assert_eq!(context.published_objects,0);
            let plan=Connection::open(context.staging.as_ref().unwrap().join("planned-catalog.sqlite3")).unwrap();
            let (old_operation,raw):(String,String)=plan.query_row("SELECT operation_id,request_json FROM operations",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
            let mut request:ProjectEditIntent=serde_json::from_str(&raw).unwrap();
            let operation="coherently-rewritten-private-operation";
            request.operation_id=OperationId(operation.into());
            let raw=serde_json::to_string(&request).unwrap();let request_sha=checksum(raw.as_bytes());
            plan.execute("UPDATE operations SET operation_id=?1,request_json=?2,request_sha256=?3 WHERE operation_id=?4",params![operation,raw,request_sha,old_operation]).unwrap();
            let (old_proof,raw):(String,String)=plan.query_row("SELECT proof_sha256,receipt_json FROM archive_copy_proofs",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
            plan.execute("UPDATE archive_copy_proof_operations SET destination_operation_id=?1,destination_request_sha256=?2 WHERE proof_sha256=?3",params![operation,request_sha,old_proof]).unwrap();
            let mut proof:Proof=serde_json::from_str(&raw).unwrap();
            let mut digest=mapping_hash();let mut statement=plan.prepare(MAP_SELECT).unwrap();
            let mut rows=statement.query([&old_proof]).unwrap();
            while let Some(row)=rows.next().unwrap(){let mapping=Mapping::from_row(row).unwrap();mapping.validate().unwrap();hash_mapping(&mut digest,&mapping);}
            drop(rows);drop(statement);
            proof.mapping_sha256=format!("{:x}",digest.finalize());let raw=proof.json().unwrap();let sha=checksum(raw.as_bytes());
            plan.execute("INSERT INTO archive_copy_proofs VALUES(?1,?2)",params![sha,raw]).unwrap();
            for table in ["archive_copy_proof_operations","archive_project_lineage","archive_copies"] {
                plan.execute(&format!("UPDATE {table} SET proof_sha256=?1 WHERE proof_sha256=?2"),params![sha,old_proof]).unwrap();
            }
            plan.execute("DELETE FROM archive_copy_proofs WHERE proof_sha256=?1",[old_proof]).unwrap();
            // The private replacement is internally valid, including its new
            // framed map digest, canonical proof and exact live-map hashes.
            let project:String=plan.query_row("SELECT id FROM projects",[],|row|row.get(0)).unwrap();
            let mut bundle=archive::copy_metadata(&plan,None,&project).unwrap();
            read_lineage(&plan,None,&project,&mut bundle,false).unwrap();
            observed.set(true);Ok(())
        }),||target.restore_archive_as_copy_outcome(&path,&intent(&sha,"copy-private-plan-postimage"))).unwrap_err();
        assert!(attacked.get());
        assert!(
            matches!(failure.operation_error(),StoreError::Invalid(message)
            if message=="Final SQL rows differ from the frozen expected postimage"),
            "{failure:?}"
        );
        assert_eq!(failure.context().phase, ArchivePhase::ObjectsPlaced);
        assert!(target.connection.is_autocommit());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(&path).unwrap(), input);
    }

    #[test]
    fn inspection_completion_survives_a_private_cleanup_warning() {
        let f = Fixture::new();
        let (_, source, path, _) = fixture_archive(&f);
        let target = f.store("inspect-target");
        let before = sql_snapshot(&target.connection);
        let input = fs::read(&path).unwrap();
        let outcome = boundary::with_test_hook(
            Box::new(|event, _| {
                if event == boundary::TestEvent::PrivateCleanup {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected private inspection cleanup",
                    ))
                } else {
                    Ok(())
                }
            }),
            || target.inspect_archive_outcome(&path),
        )
        .unwrap();
        assert_eq!(outcome.completion, ArchiveCompletion::ReadOnlyCompleted);
        assert_eq!(outcome.value.project_id, source.id());
        assert_eq!(outcome.cleanup.len(), 1);
        assert_eq!(
            outcome.cleanup[0].error.kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(outcome.cleanup[0].path.is_dir());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(&path).unwrap(), input);
    }

    #[test]
    fn committed_copy_and_missing_input_replay_keep_completion_separate_from_cleanup() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("cleanup-target");
        let request = intent(&sha, "copy-with-cleanup-warning");
        let outcome = boundary::with_test_hook(
            Box::new(|event, _| {
                if event == boundary::TestEvent::PrivateCleanup {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected private copy cleanup",
                    ))
                } else {
                    Ok(())
                }
            }),
            || target.restore_archive_as_copy_outcome(&path, &request),
        )
        .unwrap();
        assert_eq!(outcome.completion, ArchiveCompletion::CatalogCommitted);
        assert!(!outcome.value.replayed);
        assert_eq!(outcome.cleanup.len(), 1);
        assert_eq!(
            outcome.cleanup[0].error.kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(outcome.cleanup[0].path.is_dir());
        assert_eq!(
            target.load(outcome.value.document.id()).unwrap(),
            outcome.value.document
        );
        fs::remove_file(&path).unwrap();
        let before = sql_snapshot(&target.connection);
        let replay = target
            .restore_archive_as_copy_outcome(&path, &request)
            .unwrap();
        assert_eq!(replay.completion, ArchiveCompletion::RecordedReplay);
        assert!(replay.value.replayed && replay.cleanup.is_empty());
        assert_eq!(replay.value.operation_id, request.operation_id);
        assert_eq!(replay.value.document, outcome.value.document);
        assert_eq!(sql_snapshot(&target.connection), before);
        assert!(!path.exists());
    }

    #[test]
    fn injected_before_copy_commit_failure_rolls_back_every_catalog_cell_then_reuses_connection() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("precommit-target");
        let request = intent(&sha, "copy-precommit-refused");
        let before = sql_snapshot(&target.connection);
        let input = fs::read(&path).unwrap();
        let failure = boundary::with_test_hook(
            Box::new(|event, _| {
                if event == boundary::TestEvent::CopyCommit {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected before copy commit",
                    ))
                } else {
                    Ok(())
                }
            }),
            || target.restore_archive_as_copy_outcome(&path, &request),
        )
        .unwrap_err();
        assert_eq!(failure.context().action, ArchiveAction::Copy);
        assert_eq!(
            failure.context().phase,
            ArchivePhase::CatalogCommitAttempted
        );
        assert!(failure.publication_uncertain());
        assert!(
            matches!(failure.operation_error(),StoreError::Io(error) if error.kind()==std::io::ErrorKind::PermissionDenied)
        );
        assert!(target.connection.is_autocommit());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(&path).unwrap(), input);
        let completed = target
            .restore_archive_as_copy_outcome(&path, &request)
            .unwrap();
        assert_eq!(completed.completion, ArchiveCompletion::CatalogCommitted);
        assert!(!completed.value.replayed);
    }

    #[test]
    fn real_private_notadb_stays_input_local_on_inspection_and_copy() {
        let f = Fixture::new();
        let path = f.0.join("not-an-archive.omniusong");
        let bytes = vec![0xa5; 4096];
        fs::write(&path, &bytes).unwrap();
        let mut target = f.store("notadb-target");
        let before = sql_snapshot(&target.connection);
        let inspection = target.inspect_archive_outcome(&path).unwrap_err();
        let copying = target
            .restore_archive_as_copy_outcome(&path, &intent(&checksum(&bytes), "notadb-copy"))
            .unwrap_err();
        for failure in [inspection, copying] {
            let StoreError::ArchiveSql(sql) = failure.operation_error() else {
                panic!("Expected actual private SQLite error: {failure:?}")
            };
            assert_eq!(sql.origin(), ArchiveSqlOrigin::Input);
            assert!(matches!(sql.operation_error(),StoreError::Database(error)
                if error.sqlite_error().is_some_and(|code|code.extended_code&0xff==rusqlite::ffi::SQLITE_NOTADB)));
            assert_eq!(failure.catalog_sqlite_error_code(), None);
            assert!(!failure.catalog_connection_unusable());
        }
        assert_eq!(
            target
                .connection
                .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn actual_main_commit_hook_refusal_keeps_live_sql_origin_and_rolls_back() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("commit-target");
        let before = sql_snapshot(&target.connection);
        let input = fs::read(&path).unwrap();
        target.connection.commit_hook(Some(|| true)).unwrap();
        let failure = target
            .restore_archive_as_copy_outcome(&path, &intent(&sha, "commit-hook-refused"))
            .unwrap_err();
        target.connection.commit_hook(None::<fn() -> bool>).unwrap();
        assert_eq!(
            failure.context().phase,
            ArchivePhase::CatalogCommitAttempted
        );
        assert!(
            matches!(failure.operation_error(),StoreError::ArchiveSql(sql)
            if sql.origin()==ArchiveSqlOrigin::LiveCatalog)
        );
        assert_eq!(
            failure.catalog_sqlite_error_code(),
            Some(rusqlite::ffi::SQLITE_CONSTRAINT_COMMITHOOK)
        );
        assert!(target.connection.is_autocommit());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(&path).unwrap(), input);
        let completed = target
            .restore_archive_as_copy_outcome(&path, &intent(&sha, "commit-hook-refused"))
            .unwrap();
        assert_eq!(completed.completion, ArchiveCompletion::CatalogCommitted);
    }

    fn bounded_budget() -> InspectionBudget {
        InspectionBudget::new(
            Default::default(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap()
    }

    #[test]
    fn bounded_copy_commits_exact_plan_and_replays_without_input() {
        let f = Fixture::new();
        let (_, original, path, sha) = fixture_archive(&f);
        let bytes = fs::read(&path).unwrap();
        let mut target = f.store("bounded-target");
        let request = intent(&sha, "bounded-copy");
        let budget = bounded_budget();
        let completed = target
            .restore_archive_as_copy_bounded_outcome(&path, &request, &budget)
            .unwrap();
        assert_eq!(completed.completion, ArchiveCompletion::CatalogCommitted);
        assert!(completed.cleanup.is_empty());
        assert!(!completed.value.replayed);
        assert_ne!(completed.value.document.id(), original.id());
        let mut expected = original.value().clone();
        expected["id"] = completed.value.document.id().into();
        assert_eq!(completed.value.document.value(), &expected);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(budget.usage().vm_steps > 0);
        assert!(budget.usage().work_bytes > bytes.len() as u64);
        let before = sql_snapshot(&target.connection);
        fs::remove_file(&path).unwrap();
        let replay_budget = bounded_budget();
        let replayed = target
            .restore_archive_as_copy_bounded_outcome(&path, &request, &replay_budget)
            .unwrap();
        assert_eq!(replayed.completion, ArchiveCompletion::RecordedReplay);
        assert!(replayed.value.replayed);
        assert_eq!(
            replayed.value.document.value(),
            completed.value.document.value()
        );
        assert_eq!(sql_snapshot(&target.connection), before);
        assert!(replay_budget.usage().vm_steps > 0);
        assert_eq!(replay_budget.usage().file_bytes, 0);
        assert!(target.connection.is_autocommit());
    }

    #[test]
    fn bounded_copy_zero_work_refuses_before_acquisition_and_does_not_reset_budget() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("bounded-limit-target");
        let before = sql_snapshot(&target.connection);
        let budget = InspectionBudget::new(
            omniusong_recording::InspectionLimits {
                max_work_bytes: 0,
                ..Default::default()
            },
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        for _ in 0..2 {
            let failure = target
                .restore_archive_as_copy_bounded_outcome(
                    &path,
                    &intent(&sha, "bounded-limit"),
                    &budget,
                )
                .unwrap_err();
            assert_eq!(failure.context().phase, ArchivePhase::NotStarted);
            assert!(failure.context().staging.is_none());
            assert!(failure.context().archive_sha256.is_none());
            assert_eq!(sql_snapshot(&target.connection), before);
            assert!(matches!(
                budget.checkpoint(),
                Err(omniusong_recording::RecordingError::InspectionLimit {
                    resource: omniusong_recording::InspectionResource::WorkBytes,
                    limit: 0,
                    ..
                })
            ));
        }
        assert!(target.connection.is_autocommit());
    }

    #[test]
    fn bounded_copy_vm_limit_detaches_actual_catalog_hook_before_reuse() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("bounded-vm-target");
        let before = sql_snapshot(&target.connection);
        let budget = InspectionBudget::new(
            omniusong_recording::InspectionLimits {
                max_vm_steps: 1,
                ..Default::default()
            },
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        let failure = target
            .restore_archive_as_copy_bounded_outcome(&path, &intent(&sha, "bounded-vm"), &budget)
            .unwrap_err();
        assert_eq!(failure.context().phase, ArchivePhase::NotStarted);
        assert!(failure.context().staging.is_none());
        assert!(!failure.catalog_connection_unusable());
        assert!(matches!(
            budget.checkpoint(),
            Err(omniusong_recording::RecordingError::InspectionLimit {
                resource: omniusong_recording::InspectionResource::VmSteps,
                limit: 1,
                ..
            })
        ));
        assert_eq!(budget.usage().vm_steps, 1);
        assert!(target.connection.is_autocommit());
        assert_eq!(
            target
                .connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(sql_snapshot(&target.connection), before);
    }

    #[test]
    fn bounded_copy_checks_stored_snapshot_hash_columns_before_planning() {
        let f = Fixture::new();
        let (_, _, path, _) = fixture_archive(&f);
        let input = Connection::open(&path).unwrap();
        input
            .execute("UPDATE edits SET before_sha256=?1", ["0".repeat(64)])
            .unwrap();
        drop(input);
        let bytes = fs::read(&path).unwrap();
        let sha = checksum(&bytes);
        let mut target = f.store("bounded-snapshot-hash-target");
        let before = sql_snapshot(&target.connection);
        let failure = target
            .restore_archive_as_copy_bounded_outcome(
                &path,
                &intent(&sha, "bounded-snapshot-hash"),
                &bounded_budget(),
            )
            .unwrap_err();
        assert!(
            matches!(failure.operation_error(),StoreError::Invalid(message) if message=="Archive snapshot checksum mismatch")
        );
        assert_eq!(failure.context().phase, ArchivePhase::NotStarted);
        assert_eq!(
            failure.context().archive_sha256.as_deref(),
            Some(sha.as_str())
        );
        assert_eq!(failure.context().published_objects, 0);
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn bounded_copy_actual_private_notadb_never_becomes_live_fatal() {
        let f = Fixture::new();
        let path = f.0.join("bounded-notadb.omniusong");
        let bytes = vec![0xa5; 4096];
        fs::write(&path, &bytes).unwrap();
        let mut target = f.store("bounded-notadb-target");
        let before = sql_snapshot(&target.connection);
        let failure = target
            .restore_archive_as_copy_bounded_outcome(
                &path,
                &intent(&checksum(&bytes), "bounded-notadb"),
                &bounded_budget(),
            )
            .unwrap_err();
        let StoreError::ArchiveSql(sql) = failure.operation_error() else {
            panic!("Expected private SQLite origin, got {failure:?}")
        };
        assert_eq!(sql.origin(), ArchiveSqlOrigin::Input);
        assert!(matches!(sql.operation_error(),StoreError::Database(error)
            if error.sqlite_error().is_some_and(|code|code.extended_code & 0xff == rusqlite::ffi::SQLITE_NOTADB)));
        assert_eq!(failure.catalog_sqlite_error_code(), None);
        assert!(!failure.catalog_connection_unusable());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(target.connection.is_autocommit());
    }

    #[test]
    fn bounded_copy_live_trigger_failure_keeps_actual_origin_and_rolls_back() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("bounded-trigger-target");
        target.connection.execute_batch("CREATE TRIGGER refuse_bounded_copy BEFORE INSERT ON projects BEGIN SELECT RAISE(ABORT,'bounded live refusal'); END;").unwrap();
        let before = sql_snapshot(&target.connection);
        let failure = target
            .restore_archive_as_copy_bounded_outcome(
                &path,
                &intent(&sha, "bounded-trigger"),
                &bounded_budget(),
            )
            .unwrap_err();
        assert_eq!(failure.context().phase, ArchivePhase::ObjectsPlaced);
        assert!(
            matches!(failure.operation_error(),StoreError::ArchiveSql(sql)
            if sql.origin()==ArchiveSqlOrigin::LiveCatalog)
        );
        assert_eq!(
            failure.catalog_sqlite_error_code(),
            Some(rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER)
        );
        assert_eq!(sql_snapshot(&target.connection), before);
        assert!(target.connection.is_autocommit());
    }

    #[test]
    fn bounded_copy_cancel_after_frozen_plan_retains_no_catalog_authority() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("bounded-cancel-target");
        let before = sql_snapshot(&target.connection);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let budget = InspectionBudget::new(Default::default(), cancelled).unwrap();
        let failure = boundary::with_test_hook(
            Box::new(move |event, context| {
                if event == boundary::TestEvent::CopyPlanFrozen {
                    assert_eq!(context.phase, ArchivePhase::NotStarted);
                    assert_eq!(context.published_objects, 0);
                    flag.store(true, Ordering::Release);
                }
                Ok(())
            }),
            || {
                target.restore_archive_as_copy_bounded_outcome(
                    &path,
                    &intent(&sha, "bounded-cancel"),
                    &budget,
                )
            },
        )
        .unwrap_err();
        assert!(matches!(
            budget.checkpoint(),
            Err(omniusong_recording::RecordingError::InspectionCancelled)
        ));
        assert_eq!(
            failure.context().phase,
            ArchivePhase::ObjectsPlacementAttempted
        );
        assert_eq!(failure.context().published_objects, 0);
        assert!(failure.context().staging.as_ref().unwrap().is_dir());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert!(target.connection.is_autocommit());
    }

    #[test]
    fn bounded_copy_cancellation_inside_successful_commit_cannot_relabel_completion() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("bounded-late-cancel-target");
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let budget = InspectionBudget::new(Default::default(), cancelled).unwrap();
        target
            .connection
            .commit_hook(Some(move || {
                flag.store(true, Ordering::Release);
                false
            }))
            .unwrap();
        let outcome = target
            .restore_archive_as_copy_bounded_outcome(
                &path,
                &intent(&sha, "bounded-late-cancel"),
                &budget,
            )
            .unwrap();
        target.connection.commit_hook(None::<fn() -> bool>).unwrap();
        assert_eq!(outcome.completion, ArchiveCompletion::CatalogCommitted);
        assert!(!outcome.value.replayed);
        assert!(matches!(
            budget.checkpoint(),
            Err(omniusong_recording::RecordingError::InspectionCancelled)
        ));
        assert!(target.connection.is_autocommit());
        assert_eq!(
            target.load(outcome.value.document.id()).unwrap().value(),
            outcome.value.document.value()
        );
    }

    #[test]
    fn bounded_copy_invalid_recorded_proof_never_falls_through_to_missing_input() {
        let f = Fixture::new();
        let (_, _, path, sha) = fixture_archive(&f);
        let mut target = f.store("bounded-replay-invalid-target");
        let request = intent(&sha, "bounded-replay-invalid");
        target
            .restore_archive_as_copy_bounded_outcome(&path, &request, &bounded_budget())
            .unwrap();
        // Deliberately bypass the normal write guard to model retained damage;
        // the replay validator, rather than this setup trigger, must refuse.
        target
            .connection
            .execute_batch("DROP TRIGGER immutable_archive_copy_proofs_update")
            .unwrap();
        target
            .connection
            .execute(
                "UPDATE archive_copy_proofs SET receipt_json=receipt_json||' '",
                [],
            )
            .unwrap();
        let before = sql_snapshot(&target.connection);
        fs::remove_file(&path).unwrap();
        let failure = target
            .restore_archive_as_copy_bounded_outcome(&path, &request, &bounded_budget())
            .unwrap_err();
        assert!(
            matches!(failure.operation_error(),StoreError::Invalid(message) if message=="Copy proof checksum mismatch")
        );
        assert_eq!(failure.context().phase, ArchivePhase::NotStarted);
        assert!(failure.context().staging.is_none());
        assert!(failure.context().archive_sha256.is_none());
        assert_eq!(sql_snapshot(&target.connection), before);
        assert!(target.connection.is_autocommit());
    }
}

#[cfg(test)]
mod bounded_materialization_tests {
    use super::*;
    use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn catalog() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,payload TEXT,revision INTEGER,cursor INTEGER,payload_sha256 TEXT);
            CREATE TABLE edits(project_id TEXT,sequence INTEGER,before_json TEXT,after_json TEXT,before_sha256 TEXT,after_sha256 TEXT,PRIMARY KEY(project_id,sequence));
            CREATE TABLE operations(operation_id TEXT PRIMARY KEY,project_id TEXT,request_json TEXT,request_sha256 TEXT,previous_revision INTEGER,committed_revision INTEGER,result_json TEXT,result_sha256 TEXT);
            CREATE TABLE imports(project_id TEXT PRIMARY KEY,source_sha256 TEXT,original_json BLOB);").unwrap();
        connection
    }

    fn budget(max_work_bytes: u64) -> InspectionBudget {
        InspectionBudget::new(
            InspectionLimits {
                max_work_bytes,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
    }

    fn assert_work_stop<T>(result: Result<T>) {
        assert!(matches!(
            result,
            Err(StoreError::Recording(RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                ..
            }))
        ));
    }

    fn deny_digest_reads(connection: &Connection, column: &'static str) -> Arc<AtomicBool> {
        let observed = Arc::new(AtomicBool::new(false));
        let callback_observed = Arc::clone(&observed);
        connection.authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::Read { column_name, .. } if column_name == column) {
                callback_observed.store(true, Ordering::Release);
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        })).unwrap();
        observed
    }

    #[test]
    fn snapshot_hash_pass_prepays_before_preparing_any_large_payload_query() {
        for column in ["payload_sha256", "before_sha256", "after_sha256"] {
            let source = catalog();
            let small = " \n{\"opaque\":true}\n ";
            let large = format!(" \n{{\"opaque\":\"{}\"}}\n ", "x".repeat(128 * 1024));
            let current = if column == "payload_sha256" {
                large.as_str()
            } else {
                small
            };
            let before = if column == "before_sha256" {
                large.as_str()
            } else {
                small
            };
            let after = if column == "after_sha256" {
                large.as_str()
            } else {
                small
            };
            source
                .execute(
                    "INSERT INTO projects VALUES('source-project',?1,1,1,?2)",
                    params![current, checksum(current.as_bytes())],
                )
                .unwrap();
            source
                .execute(
                    "INSERT INTO edits VALUES('source-project',1,?1,?2,?3,?4)",
                    params![
                        before,
                        after,
                        checksum(before.as_bytes()),
                        checksum(after.as_bytes())
                    ],
                )
                .unwrap();

            // A combined length/payload SELECT asks for this digest as soon as
            // it is prepared. Only the prepaid point SELECT may request it.
            let point_query = deny_digest_reads(&source, column);
            let governor = budget(100_000);
            assert_work_stop(validate_snapshot_hashes_bounded(
                &source,
                "source-project",
                &governor,
            ));
            assert!(
                !point_query.load(Ordering::Acquire),
                "Payload SQL was prepared before this pass paid for its bytes"
            );
            source
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
                .unwrap();

            // Raw whitespace is part of each recorded digest. The full-credit
            // structural pass must hash those exact bytes without reencoding.
            validate_snapshot_hashes_bounded(
                &source,
                "source-project",
                &budget(InspectionLimits::default().max_work_bytes),
            )
            .unwrap();
        }
    }

    #[test]
    fn copy_plan_prepays_operation_payload_before_preparing_its_point_query() {
        let input = catalog();
        let live = catalog();
        let plan = catalog();
        let document = ProjectDocument::new("Prepaid copy plan").unwrap();
        let current = document.to_json().unwrap();
        input
            .execute(
                "INSERT INTO projects VALUES(?1,?2,0,0,?3)",
                params![
                    document.id(),
                    String::from_utf8(current.clone()).unwrap(),
                    checksum(&current)
                ],
            )
            .unwrap();
        let request = "{damaged request";
        let result = "{damaged result".repeat(16_384);
        input
            .execute(
                "INSERT INTO operations VALUES('source-operation',?1,?2,?3,0,1,?4,?5)",
                params![
                    document.id(),
                    request,
                    checksum(request.as_bytes()),
                    result,
                    checksum(result.as_bytes())
                ],
            )
            .unwrap();
        let bundle = Bundle {
            project: document.clone(),
            assets: BTreeSet::new(),
            takes: BTreeSet::new(),
            totals: archive::Totals {
                operations: 1,
                ..Default::default()
            },
            index: Sha256::new(),
        };
        let intent = RestoreCopyIntent {
            operation_id: OperationId("copy-operation".into()),
            expected_archive_sha256: "a".repeat(64),
        };
        let point_query = deny_digest_reads(&input, "request_sha256");
        let governor = budget(1_000_000);
        assert_work_stop(plan_copy_bounded(
            &live,
            &input,
            &plan,
            BoundedCopyInput {
                bundle: &bundle,
                source_project: document.id(),
                ancestors: BTreeSet::new(),
                intent: &intent,
                byte_length: 1,
                sha256: &intent.expected_archive_sha256,
            },
            &governor,
        ));
        assert!(
            !point_query.load(Ordering::Acquire),
            "Operation payload SQL was prepared before this pass paid for its bytes"
        );
        assert_eq!(
            plan.query_row("SELECT count(*) FROM operations", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        input
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
    }
}

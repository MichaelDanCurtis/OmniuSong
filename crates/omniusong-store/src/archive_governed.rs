//! Coherent bounded OSAR routes. Metadata inventories are untrusted until the
//! complete retained closure is checked against one held final-source owner.
//! Legacy ordinary entry points remain separate, with their original limits.
use crate::{
    NativeStore, Result, StoreError,
    archive::{self, ArchiveReceipt, Bundle, Destination},
    archive_boundary::{
        self as boundary, ArchiveAction, ArchiveCompletion, ArchiveContext, ArchiveFailure,
        ArchiveOutcome, ArchivePhase, ArchiveSqlOrigin,
    },
    archive_metadata as metadata,
    archive_postimage::{Layout, PostimageLedger},
    archive_projection::{self, RetainedLayout, RetainedValidator},
    catalog_transaction::CatalogTransaction,
    checksum,
    projection_sources::{ProjectionProofs, RegisteredProjectionSources},
    takes::{self, object_path},
};
use omniusong_core::ProjectDocument;
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, OpenFlags, params, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn sql<T>(origin: ArchiveSqlOrigin, result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|error| boundary::sql_error(origin, error))
}
pub(crate) fn credit(budget: &InspectionBudget, bytes: u64) -> Result<()> {
    budget.checkpoint()?;
    budget.charge_work(bytes)?;
    Ok(())
}
fn row(budget: &InspectionBudget) -> Result<()> {
    budget.visit_rows(1)?;
    credit(budget, 4096)
}
pub(crate) fn finish_failure(
    context: ArchiveContext,
    error: StoreError,
    budget: &InspectionBudget,
) -> ArchiveFailure {
    ArchiveFailure::new(context, error).with_governor_stop(budget.checkpoint().err())
}

/// Only disposable private handles. Disable before unregistering or running
/// fixed transaction controls; the original governor is never reset.
pub(crate) struct PrivateSql<'a> {
    connection: &'a Connection,
    origin: ArchiveSqlOrigin,
    enabled: Arc<AtomicBool>,
    detached: bool,
}
impl<'a> PrivateSql<'a> {
    pub(crate) fn new(
        connection: &'a Connection,
        budget: &InspectionBudget,
        origin: ArchiveSqlOrigin,
    ) -> Result<Self> {
        archive::ensure(
            origin != ArchiveSqlOrigin::LiveCatalog,
            "Private SQL scope cannot own a live catalog hook",
        )?;
        budget.checkpoint()?;
        let mut scope = Self {
            connection,
            origin,
            enabled: Arc::new(AtomicBool::new(true)),
            detached: false,
        };
        let enabled = Arc::clone(&scope.enabled);
        sql(
            origin,
            connection.rollback_hook(Some(move || enabled.store(false, Ordering::Release))),
        )?;
        let enabled = Arc::clone(&scope.enabled);
        let mut governor = budget.sqlite_vm_governor();
        if let Err(error) = sql(
            origin,
            connection.progress_handler(
                1,
                Some(move || enabled.load(Ordering::Acquire) && governor()),
            ),
        ) {
            let _ = scope.detach();
            return Err(error);
        }
        Ok(scope)
    }
    pub(crate) fn detach(&mut self) -> Result<()> {
        if self.detached {
            return Ok(());
        }
        self.enabled.store(false, Ordering::Release);
        let progress = sql(
            self.origin,
            self.connection.progress_handler(0, None::<fn() -> bool>),
        );
        let rollback = sql(self.origin, self.connection.rollback_hook(None::<fn()>));
        self.detached = true;
        progress.and(rollback)
    }
}
impl Drop for PrivateSql<'_> {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}

/// Acquired-byte pin, not a claim about a concurrently modified source's
/// historic consistency. Subsequent validation uses only the private copy.
pub(crate) fn acquire(
    path: &Path,
    stage: &Path,
    expected: Option<&str>,
    budget: &InspectionBudget,
) -> Result<(PathBuf, u64, String)> {
    credit(budget, 128 * 1024)?;
    let mut input = File::open(path)?;
    let opening = input.metadata()?;
    archive::ensure(
        opening.is_file() && opening.len() > 0 && opening.len() <= archive::MAX_ARCHIVE_FILE_BYTES,
        "Archive acquisition requires a bounded regular file",
    )?;
    budget.observe_file_bytes(opening.len())?;
    let output = stage.join("source-archive.sqlite3");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)?;
    let mut buffer = [0; 65_536];
    let mut length = 0u64;
    let mut hash = Sha256::new();
    loop {
        let capacity = opening
            .len()
            .saturating_sub(length)
            .saturating_add(1)
            .min(buffer.len() as u64) as usize;
        credit(budget, capacity as u64 * 3)?;
        let size = input.read(&mut buffer[..capacity])?;
        if size == 0 {
            break;
        }
        length = length
            .checked_add(size as u64)
            .ok_or_else(|| archive::invalid("Archive acquisition length overflow"))?;
        budget.observe_file_bytes(length)?;
        archive::ensure(length <= opening.len(), "Archive grew while being acquired")?;
        hash.update(&buffer[..size]);
        file.write_all(&buffer[..size])?;
    }
    let sha256 = format!("{:x}", hash.finalize());
    archive::ensure(
        length == opening.len()
            && input.metadata()?.len() == opening.len()
            && expected.is_none_or(|pin| pin == sha256),
        "Archive changed since inspection or while being acquired",
    )?;
    budget.checkpoint()?;
    file.sync_all()?;
    archive::sync_directory(stage)?;
    Ok((output, length, sha256))
}
pub(crate) fn open_input(path: &Path, budget: &InspectionBudget) -> Result<Connection> {
    credit(budget, path.as_os_str().len() as u64 * 8 + 4096)?;
    boundary::input(archive::readonly(path))
}
pub(crate) fn prepare_plan(stage: &Path, budget: &InspectionBudget) -> Result<Connection> {
    credit(budget, 128 * 1024)?;
    crate::archive_plan::open_catalog_bounded(stage, budget)
}
pub(crate) fn plan_objects(
    input: &Connection,
    bundle: &Bundle,
    plan: &Connection,
    budget: &InspectionBudget,
) -> Result<()> {
    for id in &bundle.assets {
        let bytes = metadata::read_asset(input, id, ArchiveSqlOrigin::Input, budget)?;
        row(budget)?;
        sql(
            ArchiveSqlOrigin::PrivateOutput,
            plan.execute("INSERT INTO assets VALUES(?1,?2)", params![id, bytes]),
        )?;
    }
    for (id, _) in archive::take_ids(bundle)? {
        let (object, json) =
            metadata::read_take_metadata(input, &id, ArchiveSqlOrigin::Input, budget)?;
        row(budget)?;
        boundary::output(archive::register_take(plan, &object, &json))?;
    }
    Ok(())
}
pub(crate) fn verify_assets(
    connection: &Connection,
    ids: &BTreeSet<String>,
    origin: ArchiveSqlOrigin,
    budget: &InspectionBudget,
) -> Result<()> {
    for id in ids {
        metadata::read_asset(connection, id, origin, budget)?;
    }
    budget.checkpoint()?;
    Ok(())
}
pub(crate) fn validate_owned_closure(
    connection: &Connection,
    root: &Path,
    project: &str,
    budget: &InspectionBudget,
) -> Result<()> {
    let (walk, set, assets) = archive_projection::collect(
        connection,
        RetainedLayout::Catalog,
        project,
        ArchiveSqlOrigin::LiveCatalog,
        budget,
    )?
    .into_parts();
    let sources = RegisteredProjectionSources::open_collected_from(
        root,
        connection,
        set,
        ArchiveSqlOrigin::LiveCatalog,
    )?;
    let proofs = sources.authenticate(budget)?;
    let mut validator = RetainedValidator::new(&proofs, budget)?;
    validator.validate(connection, ArchiveSqlOrigin::LiveCatalog, &walk)?;
    verify_assets(connection, &assets, ArchiveSqlOrigin::LiveCatalog, budget)?;
    validator.verify_final(connection, ArchiveSqlOrigin::LiveCatalog)
}

fn hash_file(path: &Path, budget: &InspectionBudget) -> Result<(u64, String)> {
    let opening = takes::ordinary_object(path)?;
    budget.observe_file_bytes(opening.len())?;
    credit(budget, 65_536)?;
    let mut file = File::open(path)?;
    let mut buffer = [0; 65_536];
    let mut length = 0u64;
    let mut hash = Sha256::new();
    loop {
        let capacity = opening
            .len()
            .saturating_sub(length)
            .saturating_add(1)
            .min(buffer.len() as u64) as usize;
        credit(budget, capacity as u64 * 2)?;
        let size = file.read(&mut buffer[..capacity])?;
        if size == 0 {
            break;
        }
        length = length
            .checked_add(size as u64)
            .ok_or_else(|| archive::invalid("Archive hash length overflow"))?;
        budget.observe_file_bytes(length)?;
        archive::ensure(
            length <= opening.len(),
            "Archive object grew during hashing",
        )?;
        hash.update(&buffer[..size]);
    }
    archive::ensure(
        length == opening.len(),
        "Archive object changed length during hashing",
    )?;
    takes::same_file(&opening, &file.metadata()?)?;
    takes::same_file(&opening, &takes::ordinary_object(path)?)?;
    Ok((length, format!("{:x}", hash.finalize())))
}

/// Structural reconstruction only. The sole original/performance
/// authentication occurs on final owned paths inside the live transaction.
pub(crate) fn reconstruct(
    input: &Connection,
    stage: &Path,
    bundle: &mut Bundle,
    budget: &InspectionBudget,
) -> Result<()> {
    fs::create_dir(stage.join("takes"))?;
    for id in bundle.assets.clone() {
        let payload = metadata::read_asset(input, &id, ArchiveSqlOrigin::Input, budget)?;
        archive::account_asset(bundle, &id, &payload)?;
    }
    let mut objects = BTreeSet::new();
    for (id, manifest) in archive::take_ids(bundle)? {
        let (object, json) =
            metadata::read_take_metadata(input, &id, ArchiveSqlOrigin::Input, budget)?;
        archive::ensure(
            object.manifest_sha256 == manifest && objects.insert(object.object_sha256.clone()),
            "Archived original identity differs or is duplicated",
        )?;
        budget.observe_file_bytes(object.byte_length)?;
        archive::account_take(bundle, &object, &json)?;
        let path = object_path(stage, &object.object_sha256)?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut statement=sql(ArchiveSqlOrigin::Input,input.prepare("SELECT sequence,byte_length,CASE WHEN octet_length(sha256)=64 THEN sha256 END,octet_length(payload) FROM take_chunks WHERE object_sha256=?1 ORDER BY sequence"))?;
        let mut rows = sql(
            ArchiveSqlOrigin::Input,
            statement.query([&object.object_sha256]),
        )?;
        let mut sequence = 0u64;
        let mut length = 0u64;
        let mut hash = Sha256::new();
        loop {
            row(budget)?;
            let Some(chunk) = sql(ArchiveSqlOrigin::Input, rows.next())? else {
                break;
            };
            let actual = archive::unsigned(
                sql(ArchiveSqlOrigin::Input, chunk.get(0))?,
                "chunk sequence",
            )?;
            let announced =
                archive::unsigned(sql(ArchiveSqlOrigin::Input, chunk.get(1))?, "chunk length")?;
            let sha: String = sql(ArchiveSqlOrigin::Input, chunk.get(2))?;
            let physical = archive::unsigned(
                sql(ArchiveSqlOrigin::Input, chunk.get(3))?,
                "chunk payload length",
            )?;
            let expected = object
                .byte_length
                .checked_sub(length)
                .ok_or_else(|| archive::invalid("Archive object length underflow"))?
                .min(archive::ARCHIVE_CHUNK_BYTES as u64);
            archive::ensure(
                actual == sequence && expected > 0 && announced == expected && physical == expected,
                "Archive chunk sequence/size differs",
            )?;
            budget.visit_chunks(1)?;
            credit(budget, expected * 4 + 4096)?;
            let payload:Vec<u8>=sql(ArchiveSqlOrigin::Input,input.query_row("SELECT CASE WHEN octet_length(payload)<=?3 THEN payload END FROM take_chunks WHERE object_sha256=?1 AND sequence=?2",params![object.object_sha256,actual as i64,archive::ARCHIVE_CHUNK_BYTES as i64],|r|r.get(0)))?;
            archive::ensure(
                payload.len() as u64 == expected && checksum(&payload) == sha,
                "Archive chunk payload checksum differs",
            )?;
            file.write_all(&payload)?;
            hash.update(&payload);
            length += expected;
            sequence += 1;
            archive::add(
                &mut bundle.totals.chunks,
                1,
                archive::ARCHIVE_PAYLOAD_BYTES / archive::ARCHIVE_CHUNK_BYTES as u64 + 4096,
                "chunk count",
            )?;
            bundle.metadata(144)?;
        }
        archive::ensure(
            length == object.byte_length
                && format!("{:x}", hash.finalize()) == object.object_sha256,
            "Archived original is missing, truncated or damaged",
        )?;
        budget.checkpoint()?;
        file.sync_all()?;
    }
    archive::sync_directory(&stage.join("takes"))?;
    Ok(())
}
pub(crate) fn install(
    destination: &Connection,
    input: &Connection,
    root: &Path,
    stage: &Path,
    bundle: &Bundle,
    published: &mut usize,
    budget: &InspectionBudget,
) -> Result<()> {
    let directory = root.join("takes");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) => archive::ensure(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Owned takes directory is redirected",
        )?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&directory)?,
        Err(error) => return Err(error.into()),
    }
    for id in &bundle.assets {
        let payload = metadata::read_asset(input, id, ArchiveSqlOrigin::Input, budget)?;
        row(budget)?;
        let exists: bool = sql(
            ArchiveSqlOrigin::LiveCatalog,
            destination.query_row(
                "SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1)",
                [id],
                |r| r.get(0),
            ),
        )?;
        if exists {
            archive::ensure(
                metadata::read_asset(destination, id, ArchiveSqlOrigin::LiveCatalog, budget)?
                    == payload,
                "Shared legacy asset differs from archive",
            )?;
        } else {
            sql(
                ArchiveSqlOrigin::LiveCatalog,
                destination.execute("INSERT INTO assets VALUES(?1,?2)", params![id, payload]),
            )?;
        }
    }
    for (id, _) in archive::take_ids(bundle)? {
        let (object, json) =
            metadata::read_take_metadata(input, &id, ArchiveSqlOrigin::Input, budget)?;
        row(budget)?;
        let exists: bool = sql(
            ArchiveSqlOrigin::LiveCatalog,
            destination.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_takes WHERE take_id=?1)",
                [&id],
                |r| r.get(0),
            ),
        )?;
        if exists {
            let (old, raw) = metadata::read_take_metadata(
                destination,
                &id,
                ArchiveSqlOrigin::LiveCatalog,
                budget,
            )?;
            archive::ensure(
                old == object && raw == json,
                "Shared original has different exact registration",
            )?;
        }
        let path = object_path(root, &object.object_sha256)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                archive::ensure(!exists, "Registered owned original is missing")?;
                let staged = object_path(stage, &object.object_sha256)?;
                fs::hard_link(&staged, &path)?;
                *published = published
                    .checked_add(1)
                    .ok_or_else(|| archive::invalid("Published object count overflow"))?;
                fs::remove_file(&staged)?;
                File::open(&path)?.sync_all()?;
            }
            Err(error) => return Err(error.into()),
        }
        let (length, sha) = hash_file(&path, budget)?;
        archive::ensure(
            length == object.byte_length && sha == object.object_sha256,
            "Owned original bytes differ from the acquired archive",
        )?;
        if !exists {
            boundary::live(archive::register_take(destination, &object, &json))?;
        }
    }
    budget.checkpoint()?;
    archive::sync_directory(&directory)?;
    archive::sync_directory(root)?;
    Ok(())
}

fn export_objects(
    source: &Connection,
    output: &Connection,
    root: &Path,
    bundle: &mut Bundle,
    expected: &PostimageLedger<'_>,
    proofs: &ProjectionProofs<'_>,
    budget: &InspectionBudget,
) -> Result<()> {
    for id in bundle.assets.clone() {
        let payload = metadata::read_asset(source, &id, ArchiveSqlOrigin::LiveCatalog, budget)?;
        archive::account_asset(bundle, &id, &payload)?;
        expected.expect_insert(
            "assets",
            &[ValueRef::Text(id.as_bytes()), ValueRef::Blob(&payload)],
            false,
        )?;
        row(budget)?;
        sql(
            ArchiveSqlOrigin::PrivateOutput,
            output.execute("INSERT INTO assets VALUES(?1,?2)", params![id, payload]),
        )?;
    }
    let mut objects = BTreeSet::new();
    for (id, manifest) in archive::take_ids(bundle)? {
        let (object, json) =
            metadata::read_take_metadata(source, &id, ArchiveSqlOrigin::LiveCatalog, budget)?;
        archive::ensure(
            object.manifest_sha256 == manifest && objects.insert(object.object_sha256.clone()),
            "Duplicate or mismatched exported original",
        )?;
        let reference = bundle
            .takes
            .iter()
            .find(|r| r.take_id.0 == id)
            .ok_or_else(|| archive::invalid("Export original has no exact reference"))?;
        let (held, path) = proofs.original(reference)?;
        archive::ensure(
            held == &object && path == object_path(root, &object.object_sha256)?,
            "Export original differs from its held source",
        )?;
        budget.observe_file_bytes(object.byte_length)?;
        archive::account_take(bundle, &object, &json)?;
        credit(budget, archive::ARCHIVE_CHUNK_BYTES as u64)?;
        let mut file = File::open(path)?;
        let mut buffer = vec![0; archive::ARCHIVE_CHUNK_BYTES];
        let mut length = 0u64;
        let mut sequence = 0i64;
        let mut hash = Sha256::new();
        while length < object.byte_length {
            let size = (object.byte_length - length).min(buffer.len() as u64) as usize;
            credit(budget, size as u64 * 4 + 4096)?;
            file.read_exact(&mut buffer[..size])?;
            budget.visit_chunks(1)?;
            hash.update(&buffer[..size]);
            let sha = checksum(&buffer[..size]);
            expected.expect_insert(
                "take_chunks",
                &[
                    ValueRef::Text(object.object_sha256.as_bytes()),
                    ValueRef::Integer(sequence),
                    ValueRef::Integer(size as i64),
                    ValueRef::Text(sha.as_bytes()),
                    ValueRef::Blob(&buffer[..size]),
                ],
                false,
            )?;
            row(budget)?;
            sql(
                ArchiveSqlOrigin::PrivateOutput,
                output.execute(
                    "INSERT INTO take_chunks VALUES(?1,?2,?3,?4,?5)",
                    params![
                        object.object_sha256,
                        sequence,
                        size as i64,
                        sha,
                        &buffer[..size]
                    ],
                ),
            )?;
            bundle.metadata(144)?;
            archive::add(
                &mut bundle.totals.chunks,
                1,
                archive::ARCHIVE_PAYLOAD_BYTES / archive::ARCHIVE_CHUNK_BYTES as u64 + 4096,
                "chunk count",
            )?;
            length += size as u64;
            sequence += 1;
        }
        credit(budget, 1)?;
        archive::ensure(
            file.read(&mut buffer[..1])? == 0
                && format!("{:x}", hash.finalize()) == object.object_sha256,
            "Held original changed while exporting",
        )?;
        proofs.verify_unchanged()?;
        expected.expect_insert(
            "native_takes",
            &[
                ValueRef::Text(id.as_bytes()),
                ValueRef::Text(manifest.as_bytes()),
                ValueRef::Text(object.object_sha256.as_bytes()),
                ValueRef::Integer(object.byte_length as i64),
                ValueRef::Text(json.as_bytes()),
            ],
            false,
        )?;
        boundary::output(archive::register_take(output, &object, &json))?;
    }
    Ok(())
}

fn prepay_schema(budget: &InspectionBudget) -> Result<()> {
    budget.visit_rows(64)?;
    credit(budget, 64 * (8192 + 1024) * 4)
}
fn version(connection: &Connection, origin: ArchiveSqlOrigin) -> Result<u32> {
    sql(
        origin,
        connection.pragma_query_value(None, "user_version", |r| r.get(0)),
    )
}

// This is a separate source pass from the lineage copier: its large proof
// payloads must be prepaid again before SQLite materializes them.
fn expect_lineage(
    source: &Connection,
    project: &str,
    expected: &PostimageLedger<'_>,
    budget: &InspectionBudget,
) -> Result<()> {
    let origin = ArchiveSqlOrigin::LiveCatalog;
    let mut statement = sql(origin, source.prepare(
        "SELECT CASE WHEN octet_length(p.proof_sha256)=64 THEN p.proof_sha256 END,octet_length(p.receipt_json) FROM archive_copy_proofs p JOIN archive_project_lineage l ON l.proof_sha256=p.proof_sha256 WHERE l.project_id=?1 ORDER BY p.proof_sha256",
    ))?;
    let mut rows = sql(origin, statement.query([project]))?;
    loop {
        row(budget)?;
        let Some(entry) = sql(origin, rows.next())? else {
            break;
        };
        let sha: String = sql(origin, entry.get(0))?;
        let bytes: i64 = sql(origin, entry.get(1))?;
        archive::ensure(
            (0..=8_421_376).contains(&bytes),
            "Archive lineage proof exceeds its field bound",
        )?;
        credit(budget, bytes as u64 * 64 + 4096)?;
        let proof: String = sql(origin, source.query_row(
            "SELECT CASE WHEN octet_length(receipt_json)=?2 THEN receipt_json END FROM archive_copy_proofs WHERE proof_sha256=?1",
            params![sha,bytes], |entry| entry.get(0),
        ))?;
        expected.expect_insert(
            "archive_copy_proofs",
            &[
                ValueRef::Text(sha.as_bytes()),
                ValueRef::Text(proof.as_bytes()),
            ],
            false,
        )?;
    }
    for (table, query, columns) in [
        (
            "archive_copy_proof_operations",
            "SELECT p.proof_sha256,p.source_operation_id,p.destination_operation_id,p.source_request_sha256,p.source_result_sha256,p.destination_request_sha256,p.destination_result_sha256 FROM archive_copy_proof_operations p JOIN archive_project_lineage l ON l.proof_sha256=p.proof_sha256 WHERE l.project_id=?1 ORDER BY p.proof_sha256,p.source_operation_id",
            7,
        ),
        (
            "archive_project_lineage",
            "SELECT project_id,proof_sha256 FROM archive_project_lineage WHERE project_id=?1 ORDER BY proof_sha256",
            2,
        ),
        (
            "archive_copies",
            "SELECT operation_id,project_id,proof_sha256 FROM archive_copies WHERE project_id=?1 ORDER BY operation_id",
            3,
        ),
    ] {
        let mut statement = sql(origin, source.prepare(query))?;
        let mut rows = sql(origin, statement.query([project]))?;
        loop {
            // The preceding bounded lineage validation holds this same SQL
            // snapshot; these fixed identity/hash columns fit this credit.
            row(budget)?;
            let Some(entry) = sql(origin, rows.next())? else {
                break;
            };
            let cells = sql(
                origin,
                (0..columns)
                    .map(|index| entry.get_ref(index))
                    .collect::<rusqlite::Result<Vec<_>>>(),
            )?;
            expected.expect_insert(table, &cells, false)?;
        }
    }
    Ok(())
}

impl NativeStore {
    /// Complete bounded performance-aware export. No inference is performed.
    pub fn export_archive_bounded_outcome(
        &self,
        project: &str,
        destination: &Path,
        budget: &InspectionBudget,
    ) -> std::result::Result<ArchiveOutcome<ArchiveReceipt>, ArchiveFailure> {
        let mut context = ArchiveContext::new(ArchiveAction::Export, &self.root);
        let result = (|| -> Result<_> {
            archive::ensure(
                destination.as_os_str().len() <= 4096
                    && self.root.as_os_str().len() <= 4096
                    && project.len() <= 96,
                "Bounded archive path or project identity exceeds its field bound",
            )?;
            credit(
                budget,
                (destination.as_os_str().len() + self.root.as_os_str().len() + project.len())
                    as u64
                    * 16
                    + 128 * 1024,
            )?;
            context.source = Some(self.root.join("studio.sqlite3"));
            let parent = fs::canonicalize(
                destination
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new(".")),
            )?;
            let destination = parent.join(
                destination
                    .file_name()
                    .ok_or_else(|| archive::invalid("Archive requires a file name"))?,
            );
            context.destination = destination.clone();
            let _owned =
                crate::retained_file_coordination::writer_with_file(&self.root, &destination)?;
            archive::ensure(
                !destination.try_exists()? && fs::symlink_metadata(&destination).is_err(),
                "Archive destination already exists",
            )?;
            let stage = archive::private_directory(&parent, "archive-export")?;
            context.staging = Some(stage.clone());
            let path = stage.join("archive.sqlite3");
            let reservation = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)?;
            let output = sql(
                ArchiveSqlOrigin::PrivateOutput,
                Connection::open_with_flags(
                    &path,
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                ),
            )?;
            sql(ArchiveSqlOrigin::PrivateOutput,output.execute_batch("PRAGMA journal_mode=DELETE;PRAGMA synchronous=FULL;PRAGMA cache_size=-2048;PRAGMA mmap_size=0;PRAGMA trusted_schema=OFF;BEGIN IMMEDIATE;"))?;
            let mut output_sql = PrivateSql::new(&output, budget, ArchiveSqlOrigin::PrivateOutput)?;
            sql(
                ArchiveSqlOrigin::PrivateOutput,
                output.pragma_update(None, "application_id", archive::ARCHIVE_APPLICATION_ID),
            )?;
            sql(
                ArchiveSqlOrigin::PrivateOutput,
                output.pragma_update(None, "user_version", archive::ARCHIVE_VERSION),
            )?;
            for (_, ddl) in archive::TABLES
                .iter()
                .chain(crate::archive_copy::LINEAGE_TABLES.iter())
            {
                row(budget)?;
                credit(budget, ddl.len() as u64 * 16)?;
                sql(ArchiveSqlOrigin::PrivateOutput, output.execute_batch(ddl))?;
            }
            output_sql.detach()?;
            sql(
                ArchiveSqlOrigin::PrivateOutput,
                output.execute_batch("COMMIT"),
            )?;
            #[cfg(test)]
            boundary::checkpoint(boundary::TestEvent::ExportSchema, &context)?;
            let expected = PostimageLedger::new(
                &stage,
                Layout::archive_version(archive::ARCHIVE_VERSION)?,
                Some(budget),
            )?;
            expected.capture_schema(&output, ArchiveSqlOrigin::PrivateOutput)?;
            let expected_path = expected.path().to_owned();
            sql(
                ArchiveSqlOrigin::PrivateOutput,
                output.execute_batch("BEGIN IMMEDIATE"),
            )?;
            let mut output_rows =
                PrivateSql::new(&output, budget, ArchiveSqlOrigin::PrivateOutput)?;
            let source = boundary::live((|| -> Result<_> {
                let connection = Connection::open_with_flags(
                    self.root.join("studio.sqlite3"),
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                connection.busy_timeout(Duration::from_secs(2))?;
                connection.execute_batch("PRAGMA query_only=ON;PRAGMA trusted_schema=OFF;PRAGMA cache_size=-2048;PRAGMA mmap_size=0;")?;
                Ok(connection)
            })())?;
            let mut tx = boundary::live(CatalogTransaction::deferred(&source, budget))?;
            let (walk, set, assets) = boundary::live(tx.prepare(|scope| {
                archive_projection::collect(
                    scope,
                    RetainedLayout::Catalog,
                    project,
                    ArchiveSqlOrigin::LiveCatalog,
                    budget,
                )
                .map(|c| c.into_parts())
            }))?;
            let sources = boundary::live(tx.prepare(|scope| {
                RegisteredProjectionSources::open_collected_from(
                    &self.root,
                    scope,
                    set,
                    ArchiveSqlOrigin::LiveCatalog,
                )
            }))?;
            let proofs = tx.prepare(|_| sources.authenticate(budget))?;
            let mut validator = tx.prepare(|_| RetainedValidator::new(&proofs, budget))?;
            let receipt = boundary::live(tx.run(|scope| {
                validator.validate(scope, ArchiveSqlOrigin::LiveCatalog, &walk)?;
                verify_assets(scope, &assets, ArchiveSqlOrigin::LiveCatalog, budget)?;
                let mut bundle = metadata::copy_metadata(
                    scope,
                    Some(Destination::ArchiveExpected(&output, &expected)),
                    project,
                    ArchiveSqlOrigin::LiveCatalog,
                    budget,
                )?;
                boundary::live(crate::archive_copy::read_lineage_bounded(
                    scope,
                    Some((&output, project, true, ArchiveSqlOrigin::PrivateOutput)),
                    project,
                    &mut bundle,
                    false,
                    budget,
                ))?;
                expect_lineage(scope, project, &expected, budget)?;
                export_objects(
                    scope,
                    &output,
                    &self.root,
                    &mut bundle,
                    &expected,
                    &proofs,
                    budget,
                )?;
                boundary::output(archive::insert_meta(&output, &bundle, &expected))?;
                let (written, _, written_assets) = archive_projection::collect(
                    &output,
                    RetainedLayout::Osar {
                        version: archive::ARCHIVE_VERSION,
                    },
                    project,
                    ArchiveSqlOrigin::PrivateOutput,
                    budget,
                )?
                .into_parts();
                validator.validate(&output, ArchiveSqlOrigin::PrivateOutput, &written)?;
                verify_assets(
                    &output,
                    &written_assets,
                    ArchiveSqlOrigin::PrivateOutput,
                    budget,
                )?;
                expected
                    .freeze()?
                    .verify(&output, ArchiveSqlOrigin::PrivateOutput)?;
                prepay_schema(budget)?;
                boundary::output(archive::validate_schema(&output))?;
                validator.verify_final(scope, ArchiveSqlOrigin::LiveCatalog)?;
                output_rows.detach()?;
                sql(
                    ArchiveSqlOrigin::PrivateOutput,
                    output.execute_batch("COMMIT"),
                )?;
                scope.rollback()?;
                Ok((bundle.totals, bundle.project.id().to_owned()))
            }))?;
            drop(output_rows);
            drop(output_sql);
            output
                .close()
                .map_err(|(_, e)| boundary::sql_error(ArchiveSqlOrigin::PrivateOutput, e))?;
            source
                .close()
                .map_err(|(_, e)| boundary::sql_error(ArchiveSqlOrigin::LiveCatalog, e))?;
            budget.checkpoint()?;
            reservation.sync_all()?;
            drop(reservation);
            let (byte_length, sha256) = hash_file(&path, budget)?;
            context.archive_sha256 = Some(sha256.clone());
            let (totals, project_id) = receipt;
            let receipt = ArchiveReceipt {
                path: destination.clone(),
                project_id,
                sha256,
                byte_length,
                history_rows: totals.edits,
                operation_rows: totals.operations,
                asset_count: totals.asset_count,
                take_count: totals.take_count,
            };
            context.phase = ArchivePhase::OutputLinkAttempted;
            #[cfg(test)]
            boundary::checkpoint(boundary::TestEvent::OutputLink, &context)?;
            proofs.verify_unchanged()?;
            budget.checkpoint()?;
            fs::hard_link(&path, &destination)?;
            context.phase = ArchivePhase::OutputLinked;
            #[cfg(test)]
            boundary::checkpoint(boundary::TestEvent::ExportSync, &context)?;
            archive::sync_directory(&parent)?;
            context.phase = ArchivePhase::ExportDurable;
            let mut outcome = ArchiveOutcome::new(receipt, ArchiveCompletion::ExportDurable);
            outcome.cleanup_file(&path);
            outcome.cleanup_file(&expected_path);
            outcome.cleanup_empty_dir(&stage);
            Ok(outcome)
        })();
        result.map_err(|error| finish_failure(context, error, budget))
    }

    pub fn restore_archive_bounded_outcome(
        &mut self,
        path: &Path,
        budget: &InspectionBudget,
    ) -> std::result::Result<ArchiveOutcome<ProjectDocument>, ArchiveFailure> {
        let mut context = ArchiveContext::new(ArchiveAction::Restore, &self.root);
        let result = (|| -> Result<_> {
            archive::ensure(
                path.as_os_str().len() <= 4096 && self.root.as_os_str().len() <= 4096,
                "Bounded archive path exceeds its field bound",
            )?;
            credit(
                budget,
                (path.as_os_str().len() + self.root.as_os_str().len()) as u64 * 16 + 64 * 1024,
            )?;
            context.source = Some(path.to_owned());
            let root = self.root.clone();
            let _owned = crate::retained_file_coordination::writer_with_file(&root, path)?;
            let stage = archive::private_directory(&root, "archive-restore")?;
            context.staging = Some(stage.clone());
            let (input, _, sha) = acquire(path, &stage, None, budget)?;
            context.archive_sha256 = Some(sha);
            #[cfg(test)]
            boundary::checkpoint(boundary::TestEvent::RestoreAcquired, &context)?;
            let input = open_input(&input, budget)?;
            let mut input_sql = PrivateSql::new(&input, budget, ArchiveSqlOrigin::Input)?;
            prepay_schema(budget)?;
            boundary::input(archive::validate_schema(&input))?;
            let version = version(&input, ArchiveSqlOrigin::Input)?;
            credit(budget, 64 * 1024)?;
            let meta = boundary::input(archive::load_meta(&input))?;
            boundary::input(crate::archive_copy::validate_snapshot_hashes_bounded(
                &input,
                &meta.project_id,
                budget,
            ))?;
            let mut bundle = metadata::copy_metadata(
                &input,
                None,
                &meta.project_id,
                ArchiveSqlOrigin::Input,
                budget,
            )?;
            boundary::input(crate::archive_copy::read_lineage_bounded(
                &input,
                None,
                &meta.project_id,
                &mut bundle,
                true,
                budget,
            ))?;
            reconstruct(&input, &stage, &mut bundle, budget)?;
            archive::ensure(
                bundle.totals == meta.totals && bundle.index_digest() == meta.index,
                "Archive closure differs from its receipt",
            )?;
            let (input_walk, set, _) = archive_projection::collect(
                &input,
                RetainedLayout::Osar { version },
                &meta.project_id,
                ArchiveSqlOrigin::Input,
                budget,
            )?
            .into_parts();
            credit(budget, meta.project_id.len() as u64 * 2 + 4096)?;
            fs::write(
                stage.join("restore.json"),
                serde_json::to_vec(
                    &serde_json::json!({"mode":"identity-bounded","projectId":meta.project_id,"acquiredArchiveSha256":context.archive_sha256}),
                ).map_err(|error|archive::invalid(error.to_string()))?,
            )?;
            let tx = boundary::live(CatalogTransaction::immediate(&mut self.connection, budget))?;
            let project = boundary::live(tx.run(|tx| {
                archive::collision_checks(tx, &input, &meta.project_id)?;
                let ledger = PostimageLedger::new(&stage, Layout::Catalog, Some(budget))?;
                ledger.capture_baseline(tx, ArchiveSqlOrigin::LiveCatalog)?;
                let plan = prepare_plan(&stage, budget)?;
                let mut plan_sql = PrivateSql::new(&plan, budget, ArchiveSqlOrigin::PrivateOutput)?;
                let mut planned = metadata::copy_metadata(
                    &input,
                    Some(Destination::PlanCatalog(&plan)),
                    &meta.project_id,
                    ArchiveSqlOrigin::Input,
                    budget,
                )?;
                plan_objects(&input, &bundle, &plan, budget)?;
                boundary::input(crate::archive_copy::read_lineage_bounded(
                    &input,
                    Some((
                        &plan,
                        &meta.project_id,
                        true,
                        ArchiveSqlOrigin::PrivateOutput,
                    )),
                    &meta.project_id,
                    &mut planned,
                    true,
                    budget,
                ))?;
                let (plan_walk, _, _) = archive_projection::collect(
                    &plan,
                    RetainedLayout::Catalog,
                    &meta.project_id,
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
                sql(
                    ArchiveSqlOrigin::PrivateOutput,
                    plan.execute_batch("PRAGMA query_only=ON"),
                )?;
                context.phase = ArchivePhase::ObjectsPlacementAttempted;
                install(
                    tx,
                    &input,
                    &root,
                    &stage,
                    &bundle,
                    &mut context.published_objects,
                    budget,
                )?;
                context.phase = ArchivePhase::ObjectsPlaced;
                let sources = RegisteredProjectionSources::open_collected_from(
                    &root,
                    tx,
                    set,
                    ArchiveSqlOrigin::LiveCatalog,
                )?;
                let proofs = sources.authenticate(budget)?;
                let mut validator = RetainedValidator::new(&proofs, budget)?;
                validator.validate(&input, ArchiveSqlOrigin::Input, &input_walk)?;
                validator.validate(&plan, ArchiveSqlOrigin::PrivateOutput, &plan_walk)?;
                let mut applied = metadata::copy_metadata(
                    &plan,
                    Some(Destination::Store(tx)),
                    &meta.project_id,
                    ArchiveSqlOrigin::PrivateOutput,
                    budget,
                )?;
                boundary::output(crate::archive_copy::read_lineage_bounded(
                    &plan,
                    Some((tx, &meta.project_id, true, ArchiveSqlOrigin::LiveCatalog)),
                    &meta.project_id,
                    &mut applied,
                    false,
                    budget,
                ))?;
                let (final_walk, _, final_assets) = archive_projection::collect(
                    tx,
                    RetainedLayout::Catalog,
                    &meta.project_id,
                    ArchiveSqlOrigin::LiveCatalog,
                    budget,
                )?
                .into_parts();
                validator.validate(tx, ArchiveSqlOrigin::LiveCatalog, &final_walk)?;
                expected.verify(tx, ArchiveSqlOrigin::LiveCatalog)?;
                verify_assets(tx, &final_assets, ArchiveSqlOrigin::LiveCatalog, budget)?;
                validator.verify_final(tx, ArchiveSqlOrigin::LiveCatalog)?;
                // Prepare the exact result and finish all private SQL before
                // the sole live commit. Nothing fallible follows success.
                let project = planned.project;
                plan_sql.detach()?;
                input_sql.detach()?;
                budget.checkpoint()?;
                context.phase = ArchivePhase::CatalogCommitAttempted;
                #[cfg(test)]
                boundary::checkpoint(boundary::TestEvent::RestoreCommit, &context)?;
                validator.verify_final(tx, ArchiveSqlOrigin::LiveCatalog)?;
                tx.commit()?;
                context.phase = ArchivePhase::CatalogCommitted;
                Ok(project)
            }))?;
            drop(input_sql);
            drop(input);
            let mut outcome = ArchiveOutcome::new(project, ArchiveCompletion::CatalogCommitted);
            outcome.cleanup_tree(&stage);
            Ok(outcome)
        })();
        result.map_err(|error| finish_failure(context, error, budget))
    }
}

#[cfg(test)]
mod tests;

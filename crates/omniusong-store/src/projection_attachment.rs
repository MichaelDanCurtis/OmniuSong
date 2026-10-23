//! Source-aware recording publication. The current registry is immutable;
//! incoming audio joins the same held proof set before any project mutation.
use crate::{
    EditReceipt, NativeStore, Result, SourceAttachmentCommit, StoreError,
    StoredPerformanceSourceAction, TakePublication,
    archive_boundary::{self as boundary, ArchiveSqlOrigin},
    archive_postimage::{Layout, PostimageLedger},
    catalog_transaction::CatalogTransaction,
    checksum,
    operations::insert_operation_metadata,
    performance_attachment_check::checked_publication_receipt_with_budget,
    performance_source_actions::{append_in_transaction, load_history},
    projection_publication::{check_assets, default_budget},
    projection_sources::RegisteredProjectionSources,
    read_document_metadata_with_budget, write_save_metadata,
};
use omniusong_core::ProjectDocument;
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, params, types::ValueRef as V};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}
struct Stage {
    path: PathBuf,
    directory: fs::Metadata,
    files: Vec<(PathBuf, fs::Metadata)>,
}
impl Stage {
    fn new(root: &Path) -> Result<Self> {
        let path = root.join(format!(".attachment-plan-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path)?;
        let directory = fs::symlink_metadata(&path)?;
        Ok(Self {
            path,
            directory,
            files: Vec::new(),
        })
    }
    fn remember(&mut self, path: PathBuf) -> Result<()> {
        if path.parent() != Some(self.path.as_path()) {
            return Err(invalid("Unexpected attachment scratch path"));
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(invalid("Invalid attachment scratch object"));
        }
        self.files.push((path, metadata));
        Ok(())
    }
    fn cleanup_completed(&self) {
        // No recursive deletion, no new directory members followed, no fallible
        // acknowledgement after commit. Failed scratch remains diagnostic.
        if !same_identity(&self.directory, &self.path, true) {
            return;
        }
        for (path, metadata) in &self.files {
            if same_identity(metadata, path, false) {
                let _ = fs::remove_file(path);
            }
        }
        let _ = fs::remove_dir(&self.path);
    }
}
fn same_identity(expected: &fs::Metadata, path: &Path, directory: bool) -> bool {
    let Ok(current) = fs::symlink_metadata(path) else {
        return false;
    };
    if current.file_type().is_symlink()
        || current.is_dir() != directory
        || (!directory && !current.is_file())
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if expected.dev() != current.dev()
            || expected.ino() != current.ino()
            || (!directory && current.nlink() != 1)
        {
            return false;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = expected;
        return false; // Retain scratch until native file IDs are qualified.
    }
    #[cfg(unix)]
    {
        true
    }
}

fn prepay_history(connection: &Connection, budget: &InspectionBudget) -> Result<()> {
    for (table, maximum, fields) in [
        (
            "performance_source_reviews",
            32u64,
            "octet_length(id)+octet_length(payload)+octet_length(payload_sha256)",
        ),
        (
            "performance_source_actions",
            512u64,
            "octet_length(id)+octet_length(review_blob_sha256)+octet_length(payload)+octet_length(payload_sha256)+8",
        ),
    ] {
        let mut statement =
            connection.prepare(&format!("SELECT {fields} FROM {table} NOT INDEXED"))?;
        let mut rows = statement.query([])?;
        let mut count = 0u64;
        loop {
            budget.visit_rows(1)?;
            let Some(row) = rows.next()? else { break };
            count += 1;
            if count > maximum {
                return Err(invalid("Source history exceeds its row limit"));
            }
            let length: i64 = row.get(0)?;
            let length =
                u64::try_from(length).map_err(|_| invalid("Invalid source history byte length"))?;
            if length > 66_000 {
                return Err(invalid("Source history exceeds its field limit"));
            }
            budget.charge_work(
                length
                    .checked_mul(64)
                    .and_then(|x| x.checked_add(4096))
                    .ok_or_else(|| invalid("Source history work overflow"))?,
            )?;
        }
    }
    Ok(())
}

/// A fixed trigger-free planner executes the same opaque action validation on
/// exact pre-mutation inputs. It never reads any applied destination row.
fn planned_action(
    stage: &Path,
    commit: &SourceAttachmentCommit<'_>,
) -> Result<(StoredPerformanceSourceAction, fs::Metadata)> {
    let budget = commit.budget;
    budget.charge_work(
        (commit.review.len() as u64)
            .saturating_add(commit.committed_payload.len() as u64)
            .saturating_mul(64)
            .saturating_add(4096),
    )?;
    // Five further full-history materializations: private input population,
    // its append pre/post passes, and the live append pre/post passes. Both
    // explicit live load_history calls have separate prepaid scans as well.
    for row in &commit.expected_history.reviews {
        budget.visit_rows(5)?;
        budget.charge_work(
            (row.payload.len() as u64
                + row.id.len() as u64
                + row.payload_sha256.len() as u64
                + 4096)
                .saturating_mul(64 * 5),
        )?;
    }
    for row in &commit.expected_history.revisions {
        budget.visit_rows(5)?;
        budget.charge_work(
            (row.payload.len() as u64
                + row.id.len() as u64
                + row.payload_sha256.len() as u64
                + row.review_blob_sha256.len() as u64
                + 4096)
                .saturating_mul(64 * 5),
        )?;
    }
    budget.charge_work(
        (commit.review.len() as u64 + commit.committed_payload.len() as u64 + 4096)
            .saturating_mul(64 * 4),
    )?;
    let (connection, held_file) = crate::archive_plan::open_catalog_bounded_held(stage, budget)?;
    let mut scope = crate::archive_governed::PrivateSql::new(
        &connection,
        budget,
        ArchiveSqlOrigin::PrivateOutput,
    )?;
    let result = boundary::output((|| -> Result<_> {
        for row in &commit.expected_history.reviews {
            connection.execute(
                "INSERT INTO performance_source_reviews VALUES(?1,?2,?3)",
                params![row.id, row.payload, row.payload_sha256],
            )?;
        }
        for row in &commit.expected_history.revisions {
            connection.execute(
                "INSERT INTO performance_source_actions VALUES(?1,?2,?3,?4,?5)",
                params![
                    row.id,
                    row.revision as i64,
                    row.review_blob_sha256,
                    row.payload,
                    row.payload_sha256
                ],
            )?;
        }
        append_in_transaction(
            &connection,
            commit.id,
            Some(commit.expected_revision),
            commit.review,
            commit.committed_payload,
            Some(commit.expected_history),
        )
    })());
    let result = scope.finish(budget, result);
    let detached = scope.detach();
    drop(scope);
    let row = result?;
    detached?;
    let opening = held_file.metadata()?;
    if !same_identity(&opening, &stage.join("planned-catalog.sqlite3"), false) {
        return Err(invalid("Attachment plan file was replaced"));
    }
    boundary::output(connection.close().map_err(|(_, e)| StoreError::Database(e)))?;
    drop(held_file);
    Ok((row, opening))
}

fn plan_edit(
    ledger: &PostimageLedger<'_>,
    connection: &Connection,
    current: &ProjectDocument,
    candidate: &ProjectDocument,
    publication: &TakePublication,
    request: &str,
    budget: &InspectionBudget,
) -> Result<ProjectDocument> {
    let cursor: i64 = connection.query_row(
        "SELECT cursor FROM projects WHERE id=?1",
        [current.id()],
        |r| r.get(0),
    )?;
    let next_cursor = cursor
        .checked_add(1)
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("Edit cursor exhausted"))?;
    let next = candidate.with_revision(
        current
            .revision()
            .checked_add(1)
            .ok_or_else(|| invalid("Revision exhausted"))?,
    )?;
    let previous =
        i64::try_from(current.revision()).map_err(|_| invalid("Revision exceeds SQL limits"))?;
    let committed =
        i64::try_from(next.revision()).map_err(|_| invalid("Revision exceeds SQL limits"))?;
    let raw_length: i64 = connection.query_row(
        "SELECT octet_length(payload) FROM projects WHERE id=?1",
        [current.id()],
        |r| r.get(0),
    )?;
    if !(0..=omniusong_core::MAX_NATIVE_PROJECT_BYTES as i64).contains(&raw_length) {
        return Err(invalid("Project payload exceeds its metadata bound"));
    }
    budget.charge_work((raw_length as u64).saturating_mul(64).saturating_add(4096))?;
    let raw_before: String = connection.query_row(
        "SELECT payload FROM projects WHERE id=?1",
        [current.id()],
        |r| r.get(0),
    )?;
    if ProjectDocument::from_json(raw_before.as_bytes())? != *current {
        return Err(invalid("Project preimage differs from its metadata view"));
    }
    let before = current.to_json()?;
    let after = next.to_json()?;
    ledger.expect_replace(
        "projects",
        &[
            V::Text(current.id().as_bytes()),
            V::Text(raw_before.as_bytes()),
            V::Integer(previous),
            V::Integer(cursor),
        ],
        &[
            V::Text(current.id().as_bytes()),
            V::Text(&after),
            V::Integer(committed),
            V::Integer(next_cursor),
        ],
    )?;
    // Discover only scalar identities first; full payloads are separately prepaid.
    let mut statement = connection.prepare(
        "SELECT sequence FROM edits WHERE project_id=?1 AND sequence>?2 ORDER BY sequence",
    )?;
    let mut rows = statement.query(params![current.id(), cursor])?;
    loop {
        budget.visit_rows(1)?;
        let Some(row) = rows.next()? else { break };
        let sequence: i64 = row.get(0)?;
        crate::projection_metadata::prepay_history(connection, current.id(), sequence, budget)?;
        let (old_before, old_after): (String, String) = connection.query_row(
            "SELECT before_json,after_json FROM edits WHERE project_id=?1 AND sequence=?2",
            params![current.id(), sequence],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ledger.expect_remove(
            "edits",
            &[
                V::Text(current.id().as_bytes()),
                V::Integer(sequence),
                V::Text(old_before.as_bytes()),
                V::Text(old_after.as_bytes()),
            ],
        )?;
    }
    ledger.expect_insert(
        "edits",
        &[
            V::Text(current.id().as_bytes()),
            V::Integer(next_cursor),
            V::Text(&before),
            V::Text(&after),
        ],
        false,
    )?;
    ledger.expect_insert(
        "operations",
        &[
            V::Text(publication.operation_id.0.as_bytes()),
            V::Text(current.id().as_bytes()),
            V::Text(request.as_bytes()),
            V::Text(checksum(request.as_bytes()).as_bytes()),
            V::Integer(previous),
            V::Integer(committed),
            V::Text(&after),
            V::Text(checksum(&after).as_bytes()),
        ],
        false,
    )?;
    Ok(next)
}

fn plan_action(
    ledger: &PostimageLedger<'_>,
    commit: &SourceAttachmentCommit<'_>,
    row: &StoredPerformanceSourceAction,
) -> Result<()> {
    if !commit
        .expected_history
        .reviews
        .iter()
        .any(|r| r.id == row.id)
    {
        ledger.expect_insert(
            "performance_source_reviews",
            &[
                V::Text(row.id.as_bytes()),
                V::Blob(commit.review),
                V::Text(row.review_blob_sha256.as_bytes()),
            ],
            false,
        )?;
    }
    if !commit
        .expected_history
        .revisions
        .iter()
        .any(|r| r.id == row.id && r.revision == row.revision)
    {
        ledger.expect_insert(
            "performance_source_actions",
            &[
                V::Text(row.id.as_bytes()),
                V::Integer(row.revision as i64),
                V::Text(row.review_blob_sha256.as_bytes()),
                V::Blob(&row.payload),
                V::Text(row.payload_sha256.as_bytes()),
            ],
            false,
        )?;
    }
    Ok(())
}

fn publish(
    tx: &mut CatalogTransaction<'_, '_>,
    root: &Path,
    publication: &TakePublication,
    request: &str,
    action: Option<&SourceAttachmentCommit<'_>>,
    budget: &InspectionBudget,
) -> Result<(EditReceipt, Option<StoredPerformanceSourceAction>)> {
    let _files = crate::retained_file_coordination::writer(root)?;
    budget.checkpoint()?;
    let current = read_document_metadata_with_budget(tx, &publication.project_id.0, budget)?;
    if current.revision() != publication.expected_revision {
        return Err(StoreError::Conflict);
    }
    if let Some(commit) = action {
        prepay_history(tx, budget)?;
        if load_history(tx)? != *commit.expected_history {
            return Err(StoreError::Conflict);
        }
    }
    let mut candidate = if current.schema_version() == 1 {
        current.upgrade_native()?
    } else {
        current.clone()
    };
    candidate.append_take_clip(
        &publication.track_id.0,
        &publication.track_name,
        publication.clip.clone(),
    )?;
    crate::active_projection_edits::unchanged_registry(&current, &candidate)?;
    check_assets(tx, &[&current, &candidate], budget)?;
    let mut collected = crate::projection_sources::RegisteredSourceSet::new(budget);
    collected.add_document(&current)?;
    collected.add_document(&candidate)?;
    let sources = RegisteredProjectionSources::open_collected_from(
        root,
        tx,
        collected,
        ArchiveSqlOrigin::LiveCatalog,
    )?;
    let proofs = sources.authenticate(budget)?;
    proofs.validate_document(&current)?;
    proofs.validate_document_geometry(&candidate)?;
    let mut stage = Stage::new(root)?;
    let planned_row = if let Some(commit) = action {
        let (row, opening) = planned_action(&stage.path, commit)?;
        stage
            .files
            .push((stage.path.join("planned-catalog.sqlite3"), opening));
        Some(row)
    } else {
        None
    };
    let ledger = PostimageLedger::new(&stage.path, Layout::Catalog, Some(budget))?;
    stage.remember(ledger.path().to_owned())?;
    ledger.capture_baseline(tx, ArchiveSqlOrigin::LiveCatalog)?;
    let expected_document = plan_edit(
        &ledger,
        tx,
        &current,
        &candidate,
        publication,
        request,
        budget,
    )?;
    if let (Some(commit), Some(row)) = (action, planned_row.as_ref()) {
        plan_action(&ledger, commit, row)?;
    }
    let expected = ledger.freeze()?;
    let saved = write_save_metadata(tx, &current, &candidate, publication.expected_revision)?;
    if saved.document != expected_document {
        return Err(invalid("Attachment save differs from its independent plan"));
    }
    insert_operation_metadata(
        tx,
        &publication.operation_id.0,
        publication.expected_revision,
        request,
        &saved.document,
    )?;
    let row = if let Some(commit) = action {
        let row = append_in_transaction(
            tx,
            commit.id,
            Some(commit.expected_revision),
            commit.review,
            commit.committed_payload,
            Some(commit.expected_history),
        )?;
        if Some(&row) != planned_row.as_ref() {
            return Err(invalid(
                "Attachment action differs from its independent plan",
            ));
        }
        Some(row)
    } else {
        None
    };
    // The independent frozen plan contains every exact operation column,
    // including request/result bytes and their hashes. Its full comparison
    // below checks the stored receipt without decoding the entire result a
    // second time. `saved.document` already equals the planned document.
    let receipt = EditReceipt {
        operation_id: publication.operation_id.0.clone(),
        previous_revision: publication.expected_revision,
        document: saved.document,
        replayed: false,
    };
    expected.verify(tx, ArchiveSqlOrigin::LiveCatalog)?;
    check_assets(tx, &[&current, &candidate], budget)?;
    proofs.verify_registrations_from(tx, ArchiveSqlOrigin::LiveCatalog)?;
    proofs.verify_unchanged()?;
    budget.checkpoint()?;
    tx.commit()?;
    stage.cleanup_completed();
    Ok((receipt, row))
}

impl NativeStore {
    /// Exact recorded receipt is a historical fact, not current usability. New
    /// publication alone consumes the caller's cumulative source governor.
    pub fn publish_take_with_budget(
        &mut self,
        publication: &TakePublication,
        budget: &InspectionBudget,
    ) -> Result<EditReceipt> {
        crate::source_attachment::reject_reserved_operation(&publication.operation_id.0)?;
        let request = crate::takes::publication_request(publication)?;
        let metadata = default_budget();
        CatalogTransaction::immediate(&mut self.connection, &metadata)?.run(|tx| {
            if let Some(receipt) =
                checked_publication_receipt_with_budget(tx, publication, &metadata)?
            {
                tx.commit()?;
                return Ok(receipt);
            }
            crate::projection_publication_types::reject_reserved_operation(
                &publication.operation_id.0,
            )?;
            tx.switch_governor(budget)?;
            publish(tx, &self.root, publication, &request, None, budget).map(|r| r.0)
        })
    }
    pub(crate) fn publish_source_attachment_governed(
        &mut self,
        commit: SourceAttachmentCommit<'_>,
    ) -> Result<(EditReceipt, StoredPerformanceSourceAction)> {
        let request = crate::takes::publication_request(commit.publication)?;
        CatalogTransaction::immediate(&mut self.connection, commit.budget)?.run(|tx| {
            prepay_history(tx, commit.budget)?;
            if load_history(tx)? != *commit.expected_history {
                return Err(StoreError::Conflict);
            }
            if checked_publication_receipt_with_budget(tx, commit.publication, commit.budget)?
                .is_some()
            {
                return Err(invalid(
                    "An existing publication must be reconciled before fresh source attachment",
                ));
            }
            let (receipt, row) = publish(
                tx,
                &self.root,
                commit.publication,
                &request,
                Some(&commit),
                commit.budget,
            )?;
            Ok((receipt, row.expect("source action was planned")))
        })
    }
}

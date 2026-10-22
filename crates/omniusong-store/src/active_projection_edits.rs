//! Explicit source-aware active paths. Legacy guarded helpers remain available
//! to unmigrated archive, metadata-reader and attachment consumers.
use crate::catalog_transaction::CatalogTransaction;
use crate::{
    EditReceipt, Result, StoreError,
    operations::{insert_operation_metadata, read_operation_metadata},
    projection_publication::{check_assets, has_projection, validate_forward_transition},
    projection_sources::RegisteredProjectionSources,
    read_document_metadata_with_budget, verify_saved_metadata, write_save_metadata,
};
use omniusong_core::{
    ClipPatch, DocumentClipAddress, ProjectDocument,
    contracts::{EditOperation, EditScope, ProjectEditIntent},
    native::MAX_NATIVE_PROJECT_BYTES,
};
use omniusong_recording::InspectionBudget;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
fn invalid(s: &str) -> StoreError {
    StoreError::Invalid(s.into())
}

pub(crate) fn unchanged_registry(before: &ProjectDocument, after: &ProjectDocument) -> Result<()> {
    crate::projection_boundary::reject_opaque_promotion(before, after)?;
    if (has_projection(before)? || has_projection(after)?)
        && (before.schema_version() != after.schema_version()
            || before.value()["performanceProjections"] != after.value()["performanceProjections"])
    {
        return Err(invalid(
            "Generic edits cannot insert, remove or rewrite original projection ownership",
        ));
    }
    Ok(())
}
pub(crate) fn load(
    tx: &mut CatalogTransaction<'_, '_>,
    root: &Path,
    document: ProjectDocument,
    budget: &InspectionBudget,
) -> Result<ProjectDocument> {
    let _files = crate::retained_file_coordination::writer(root)?;
    budget.checkpoint()?;
    check_assets(tx, &[&document], budget)?;
    let sources = RegisteredProjectionSources::open(root, tx, &[&document], &[], budget)?;
    let proofs = sources.authenticate(budget)?;
    proofs.validate_document(&document)?;
    proofs.verify_registrations(tx)?;
    proofs.verify_unchanged()?;
    tx.rollback()?;
    Ok(document)
}

pub(crate) fn replay(
    tx: &mut CatalogTransaction<'_, '_>,
    root: &Path,
    receipt: EditReceipt,
    budget: &InspectionBudget,
) -> Result<EditReceipt> {
    let _files = crate::retained_file_coordination::writer(root)?;
    check_assets(tx, &[&receipt.document], budget)?;
    let sources = RegisteredProjectionSources::open(root, tx, &[&receipt.document], &[], budget)?;
    let proofs = sources.authenticate(budget)?;
    proofs.validate_document(&receipt.document)?;
    proofs.verify_registrations(tx)?;
    proofs.verify_unchanged()?;
    tx.commit()?;
    Ok(receipt)
}

#[derive(Default)]
pub(crate) struct SaveExtras<'a> {
    operation: Option<(&'a str, &'a str)>,
    clip: Option<(&'a DocumentClipAddress, &'a ClipPatch)>,
}
pub(crate) fn save(
    tx: &mut CatalogTransaction<'_, '_>,
    root: &Path,
    current: ProjectDocument,
    candidate: &ProjectDocument,
    expected: u64,
    extras: SaveExtras<'_>,
    budget: &InspectionBudget,
) -> Result<ProjectDocument> {
    let SaveExtras { operation, clip } = extras;
    let _files = crate::retained_file_coordination::writer(root)?;
    budget.checkpoint()?;
    unchanged_registry(&current, candidate)?;
    if current.revision() != expected || candidate.revision() != expected {
        return Err(StoreError::Conflict);
    }
    let sources = RegisteredProjectionSources::open(root, tx, &[&current, candidate], &[], budget)?;
    let proofs = sources.authenticate(budget)?;
    proofs.validate_document(&current)?;
    if let Some((address, patch)) = clip {
        crate::operations::check_requested_audio_window(tx, &current, candidate, address, patch)?;
    }
    // Registry equality was established before opening sources. Only current
    // geometry changed; do not rederive identical retained bindings a second time.
    proofs.validate_document_geometry(candidate)?;
    let saved = write_save_metadata(tx, &current, candidate, expected)?;
    if let Some((id, request)) = operation {
        insert_operation_metadata(tx, id, expected, request, &saved.document)?;
        crate::projection_metadata::prepay_operation(tx, id, budget)?;
        let receipt = read_operation_metadata(tx, id, saved.document.id(), expected, request)?
            .ok_or_else(|| invalid("Edit receipt disappeared inside transaction"))?;
        if receipt.document != saved.document {
            return Err(invalid("Edit receipt postimage changed inside transaction"));
        }
    }
    verify_saved_metadata(tx, &saved, budget)?;
    check_assets(tx, &[&current, candidate], budget)?;
    proofs.verify_registrations(tx)?;
    proofs.verify_unchanged()?;
    budget.checkpoint()?;
    tx.commit()?;
    Ok(saved.document)
}

pub(crate) struct Navigation {
    current: ProjectDocument,
    before: ProjectDocument,
    after: ProjectDocument,
    next: ProjectDocument,
    next_cursor: i64,
}
pub(crate) fn navigation(
    connection: &Connection,
    id: &str,
    expected: u64,
    redo: bool,
    budget: &InspectionBudget,
) -> Result<Navigation> {
    let current = crate::read_document_metadata_with_budget(connection, id, budget)?;
    if current.revision() != expected {
        return Err(StoreError::Conflict);
    }
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
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM edits WHERE project_id=?1 AND sequence=?2)",
        params![id, target],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(invalid(if redo {
            "Nothing to redo"
        } else {
            "Nothing to undo"
        }));
    }
    crate::projection_metadata::prepay_history(connection, id, target, budget)?;
    let row:Option<(Option<String>,Option<String>)>=connection.query_row("SELECT CASE WHEN octet_length(before_json)<=?3 THEN before_json END,CASE WHEN octet_length(after_json)<=?3 THEN after_json END FROM edits WHERE project_id=?1 AND sequence=?2",params![id,target,MAX_NATIVE_PROJECT_BYTES as i64],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (Some(before), Some(after)) = row.ok_or_else(|| {
        invalid(if redo {
            "Nothing to redo"
        } else {
            "Nothing to undo"
        })
    })?
    else {
        return Err(invalid("Stored history exceeds metadata bounds"));
    };
    let before = ProjectDocument::from_json(before.as_bytes())?;
    let after = ProjectDocument::from_json(after.as_bytes())?;
    if before.id() != id
        || after.id() != id
        || before.revision().checked_add(1) != Some(after.revision())
    {
        return Err(invalid("Stored history identity/revision pair differs"));
    }
    let selected = if redo { &before } else { &after };
    if (has_projection(&current)? || has_projection(&before)? || has_projection(&after)?)
        && current.with_revision(selected.revision())? != *selected
    {
        return Err(invalid(
            "Current project does not match the selected history edge",
        ));
    }
    let next = if redo { after.clone() } else { before.clone() }.with_revision(
        expected
            .checked_add(1)
            .ok_or_else(|| invalid("Project revision exhausted"))?,
    )?;
    let next_cursor = if redo {
        Some(target)
    } else {
        cursor.checked_sub(1).filter(|n| *n >= 0)
    }
    .ok_or_else(|| invalid("Stored edit cursor is invalid"))?;
    Ok(Navigation {
        current,
        before,
        after,
        next,
        next_cursor,
    })
}
pub(crate) fn navigate(
    tx: &mut CatalogTransaction<'_, '_>,
    root: &Path,
    plan: Navigation,
    operation: Option<(&str, &str)>,
    budget: &InspectionBudget,
) -> Result<ProjectDocument> {
    let _files = crate::retained_file_coordination::writer(root)?;
    // navigation() proved current/next are exact content copies of these sides
    // (ignoring monotonic revision). Do not charge four duplicate interpretations.
    let docs = [&plan.before, &plan.after];
    let sources = RegisteredProjectionSources::open(root, tx, &docs, &[], budget)?;
    let proofs = sources.authenticate(budget)?;
    proofs.validate_document(&plan.after)?;
    proofs.validate_document_geometry(&plan.before)?;
    validate_forward_transition(tx, &plan.before, &plan.after, budget)?;
    tx.execute(
        "UPDATE projects SET payload=?1,revision=?2,cursor=?3 WHERE id=?4",
        params![
            String::from_utf8(plan.next.to_json()?).expect("JSON UTF8"),
            plan.next.revision() as i64,
            plan.next_cursor,
            plan.next.id()
        ],
    )?;
    if let Some((id, request)) = operation {
        insert_operation_metadata(tx, id, plan.current.revision(), request, &plan.next)?;
        crate::projection_metadata::prepay_operation(tx, id, budget)?;
        let receipt =
            read_operation_metadata(tx, id, plan.next.id(), plan.current.revision(), request)?
                .ok_or_else(|| invalid("History receipt disappeared"))?;
        if receipt.document != plan.next {
            return Err(invalid("History receipt changed inside transaction"));
        }
    }
    let cursor: i64 = tx.query_row(
        "SELECT cursor FROM projects WHERE id=?1",
        [plan.next.id()],
        |r| r.get(0),
    )?;
    if cursor != plan.next_cursor
        || read_document_metadata_with_budget(tx, plan.next.id(), budget)? != plan.next
    {
        return Err(invalid("History postimage changed inside transaction"));
    }
    check_assets(tx, &docs, budget)?;
    proofs.verify_registrations(tx)?;
    proofs.verify_unchanged()?;
    budget.checkpoint()?;
    tx.commit()?;
    Ok(plan.next)
}

pub(crate) fn apply(
    tx: &mut CatalogTransaction<'_, '_>,
    root: &Path,
    intent: &ProjectEditIntent,
    request: &str,
    current: ProjectDocument,
    budget: &InspectionBudget,
) -> Result<EditReceipt> {
    let scope_note = || {
        if let EditScope::DocumentNote { address } = &intent.scope {
            Ok(address)
        } else {
            Err(invalid("Note edit requires document-note scope"))
        }
    };
    let scope_clip = || {
        if let EditScope::DocumentClip { address } = &intent.scope {
            Ok(address)
        } else {
            Err(invalid("Clip edit requires document-clip scope"))
        }
    };
    let mut clip = None;
    let candidate = match &intent.edit {
        EditOperation::Rename { name } => {
            let mut next = current.clone();
            next.rename(name)?;
            next
        }
        EditOperation::SetTempo { bpm } => {
            let mut next = current.clone();
            next.set_tempo(*bpm)?;
            next
        }
        EditOperation::UpdateSession { name, tempo } => {
            let mut next = current.clone();
            next.rename(name)?;
            next.set_tempo(*tempo)?;
            next
        }
        EditOperation::AddNote { values, timeline } => current
            .add_note(scope_note()?, *values, *timeline)
            .map_err(StoreError::EditRejected)?,
        EditOperation::UpdateNote { values, timeline } => current
            .update_note(scope_note()?, *values, *timeline)
            .map_err(StoreError::EditRejected)?,
        EditOperation::DeleteNote => current
            .delete_note(scope_note()?)
            .map_err(StoreError::EditRejected)?,
        EditOperation::RestoreNote { timeline } => current
            .restore_note(scope_note()?, *timeline)
            .map_err(StoreError::EditRejected)?,
        EditOperation::UpdateClip { patch, timeline } => {
            let address = scope_clip()?;
            clip = Some((address, patch));
            current
                .update_clip(address, patch, *timeline)
                .map_err(StoreError::EditRejected)?
        }
        EditOperation::RestoreClip { timeline } => current
            .restore_clip(scope_clip()?, *timeline)
            .map_err(StoreError::EditRejected)?,
        EditOperation::Undo | EditOperation::Redo => {
            let plan = navigation(
                tx,
                current.id(),
                intent.expected_revision,
                matches!(intent.edit, EditOperation::Redo),
                budget,
            )?;
            let document = navigate(
                tx,
                root,
                plan,
                Some((&intent.operation_id.0, request)),
                budget,
            )?;
            return Ok(EditReceipt {
                operation_id: intent.operation_id.0.clone(),
                previous_revision: intent.expected_revision,
                document,
                replayed: false,
            });
        }
        _ => {
            return Err(invalid(
                "Prepared edits and candidates require separate admission",
            ));
        }
    };
    let document = save(
        tx,
        root,
        current,
        &candidate,
        intent.expected_revision,
        SaveExtras {
            operation: Some((&intent.operation_id.0, request)),
            clip,
        },
        budget,
    )?;
    Ok(EditReceipt {
        operation_id: intent.operation_id.0.clone(),
        previous_revision: intent.expected_revision,
        document,
        replayed: false,
    })
}

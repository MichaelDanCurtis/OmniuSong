//! Typed atomic publication; recorded commitment is separate from current
//! source-authenticated usability. This module does not grant archive authority.
use crate::catalog_transaction::CatalogTransaction;
use crate::{
    NativeStore, ProjectionCommitReceipt, ProjectionPublication, Result, StoreError, checksum,
    operations::{insert_operation_metadata, read_operation_metadata},
    projection_sources::RegisteredProjectionSources,
    read_document_metadata_with_budget, verify_saved_metadata, write_save_metadata,
};
use omniusong_core::{
    ProjectDocument, native::TakeReference, performance_projection::PerformanceProjectionBinding,
};
use omniusong_performance::projection::{
    PerformanceProjectionReview, check_retained_binding_encoding_bounded,
};
use omniusong_recording::{InspectionBudget, InspectionLimits};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};

pub(crate) fn default_budget() -> InspectionBudget {
    InspectionBudget::new(
        InspectionLimits::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .expect("compiled inspection ceilings")
}
pub(crate) fn has_projection(document: &ProjectDocument) -> Result<bool> {
    Ok(document
        .performance_projections()?
        .is_some_and(|r| !r.entries.is_empty()))
}
pub(crate) fn check_assets(
    connection: &Connection,
    documents: &[&ProjectDocument],
    budget: &InspectionBudget,
) -> Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    for document in documents {
        budget.checkpoint()?;
        for id in document.referenced_assets()? {
            budget.charge_work(1024)?;
            ids.insert(id);
        }
    }
    for id in ids {
        budget.checkpoint()?;
        let length: i64 = connection.query_row(
            "SELECT octet_length(payload) FROM assets WHERE id=?1",
            [&id],
            |r| r.get(0),
        )?;
        if !(1..=crate::MAX_ASSET_BYTES as i64).contains(&length) {
            return Err(invalid("Referenced audio asset exceeds its bounded size"));
        }
        budget.charge_work((length as u64).saturating_mul(4) + 4096)?;
        crate::read_asset(connection, &id)?;
    }
    budget.checkpoint()?;
    Ok(())
}
fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::Invalid(message.into())
}

fn initial_tracks_match(
    document: &ProjectDocument,
    binding: &PerformanceProjectionBinding,
    intent: &ProjectionPublication,
) -> Result<()> {
    if binding.lanes.len() != intent.lanes.len() {
        return Err(invalid("Recorded publication lane count differs"));
    }
    for (lane, choice) in binding.lanes.iter().zip(&intent.lanes) {
        if lane.source != choice.source
            || lane.instrument != choice.instrument
            || lane.track_id != choice.track_id
        {
            return Err(invalid("Recorded publication original lane choice differs"));
        }
        let notes:Vec<_>=lane.original_notes.iter().map(|note|json!({"id":note.note_id.0,"pitch":note.values.pitch,"startTick":note.values.start_tick,"durationTicks":note.values.duration_ticks,"velocity16":note.values.velocity16,"expression":[]})).collect();
        let expected = json!({"id":choice.track_id,"name":choice.track_name,"kind":"notes","instrument":choice.instrument.as_str(),"gain":0.65,"muted":true,"locked":false,"armed":false,"notes":notes,"provenance":{"kind":"procedural-performance-projection","projectionId":binding.id}});
        if document
            .tracks()
            .iter()
            .find(|track| track["id"] == choice.track_id)
            != Some(&expected)
        {
            return Err(invalid("Recorded publication initial note lane differs"));
        }
    }
    Ok(())
}

/// Bounded catalog consistency only. A coherent catalog may describe false
/// music; this compact result cannot activate or authenticate a project.
pub(crate) fn recorded_commit_with_budget(
    connection: &Connection,
    intent: &ProjectionPublication,
    request: &str,
    budget: &InspectionBudget,
) -> Result<Option<ProjectionCommitReceipt>> {
    budget.checkpoint()?;
    let lengths:Option<(i64,i64,i64)>=connection.query_row("SELECT octet_length(request_json),octet_length(result_json),octet_length(project_id) FROM operations WHERE operation_id=?1",[&intent.operation_id.0],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    if let Some((request_len, result_len, project_len)) = lengths {
        if !(0..=omniusong_core::contracts::WORKER_MAX_BYTES as i64).contains(&request_len)
            || !(0..=omniusong_core::native::MAX_NATIVE_PROJECT_BYTES as i64).contains(&result_len)
            || !(1..=96).contains(&project_len)
        {
            return Err(invalid("Recorded publication exceeds metadata bounds"));
        }
        budget.charge_work(
            (request_len as u64 + result_len as u64 + project_len as u64).saturating_mul(64) + 4096,
        )?;
    }
    let Some(stored) = read_operation_metadata(
        connection,
        &intent.operation_id.0,
        &intent.project_id.0,
        intent.expected_revision,
        request,
    )?
    else {
        return Ok(None);
    };
    let result_sha256:Option<String>=connection.query_row("SELECT CASE WHEN octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE operation_id=?1",[&intent.operation_id.0],|r|r.get(0))?;
    let result_sha256 = result_sha256.ok_or_else(|| {
        invalid("Recorded publication result is not its exact canonical snapshot")
    })?;
    let receipt =
        validate_publication_result(intent, request, &stored.document, &result_sha256, budget)?;
    Ok(Some(receipt))
}

/// Bounded metadata consistency only, never source or archive admission. The
/// caller prepays the raw request/result before decoding and holds its governor
/// through this check. A result without a retained preimage must still preserve
/// the exact initial muted lanes and final immutable append.
pub(crate) fn validate_publication_result(
    intent: &ProjectionPublication,
    request: &str,
    document: &ProjectDocument,
    result_sha256: &str,
    budget: &InspectionBudget,
) -> Result<ProjectionCommitReceipt> {
    budget.checkpoint()?;
    if request.len() > omniusong_core::contracts::WORKER_MAX_BYTES
        || intent.canonical_json()? != request
        || document.id() != intent.project_id.0
        || intent.expected_revision.checked_add(1) != Some(document.revision())
    {
        return Err(invalid(
            "Recorded publication identity/revision or canonical request differs",
        ));
    }
    if checksum(&document.to_json()?) != result_sha256 {
        return Err(invalid(
            "Recorded publication result is not its exact canonical snapshot",
        ));
    }
    let registry = document
        .performance_projections()?
        .ok_or_else(|| invalid("Recorded projection registry is missing"))?;
    let binding = registry
        .entries
        .last()
        .filter(|binding| binding.id == intent.projection_id)
        .ok_or_else(|| invalid("Recorded projection is not its complete registry append"))?;
    // This governor covers only this bounded catalog decoding operation. No
    // original is opened and no source proof is manufactured or resumed here.
    budget
        .charge_work((binding.review.canonical_json.len() as u64).saturating_mul(32) + 1_048_576)?;
    check_retained_binding_encoding_bounded(binding, budget)?;
    let review: PerformanceProjectionReview =
        serde_json::from_str(&binding.review.canonical_json).map_err(|e| invalid(e.to_string()))?;
    if review.request != intent.request
        || review.review_sha256 != intent.expected_review_sha256
        || document.tempo().to_bits() != intent.request.tempo_bits
    {
        return Err(invalid("Recorded projection request/review differs"));
    }
    initial_tracks_match(document, binding, intent)?;
    let note_count = binding
        .lanes
        .iter()
        .map(|lane| lane.original_notes.len() as u32)
        .sum();
    let receipt = ProjectionCommitReceipt {
        version: 1,
        operation_id: intent.operation_id.clone(),
        project_id: intent.project_id.clone(),
        projection_id: intent.projection_id.clone(),
        previous_revision: intent.expected_revision,
        committed_revision: document.revision(),
        request_sha256: checksum(request.as_bytes()),
        result_sha256: result_sha256.into(),
        review_sha256: intent.expected_review_sha256.clone(),
        source: binding.source.clone(),
        lane_count: binding.lanes.len() as u32,
        note_count,
        replayed: true,
    };
    receipt.validate()?;
    budget.checkpoint()?;
    Ok(receipt)
}

impl NativeStore {
    /// Read-only source/publication/history closure for a retained live project.
    /// Counts grant no archive, adoption or current playback authority. This
    /// does not validate an OSAR file's schema, indexes or complete row closure.
    pub fn inspect_retained_projection_closure(
        &self,
        project_id: &str,
        budget: &InspectionBudget,
    ) -> Result<crate::RetainedProjectionClosureInspection> {
        use omniusong_core::contracts::{ProjectId, Validate};
        if project_id.len() > 96 {
            return Err(invalid("Project identity exceeds its limit"));
        }
        ProjectId(project_id.into())
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
        budget.checkpoint()?;
        let _files = crate::retained_file_coordination::writer(&self.root)?;
        CatalogTransaction::deferred(&self.connection, budget)?.run(|tx| {
            let result = crate::archive_projection::inspect(tx, &self.root, project_id, budget)?;
            tx.rollback()?;
            Ok(result)
        })
    }

    /// Exact recorded commitment, even when current originals are unavailable.
    /// This returns no historical or current playable ProjectDocument.
    pub fn projection_publication_receipt(
        &self,
        intent: &ProjectionPublication,
    ) -> Result<Option<ProjectionCommitReceipt>> {
        let request = intent.canonical_json()?;
        let metadata_budget = default_budget();
        CatalogTransaction::deferred(&self.connection, &metadata_budget)?.run(|tx| {
            let receipt = recorded_commit_with_budget(tx, intent, &request, &metadata_budget)?;
            tx.rollback()?;
            Ok(receipt)
        })
    }
    pub fn publish_performance_projection(
        &mut self,
        intent: &ProjectionPublication,
        budget: &InspectionBudget,
    ) -> Result<ProjectionCommitReceipt> {
        let request = intent.canonical_json()?;
        let metadata_budget = default_budget();
        CatalogTransaction::immediate(&mut self.connection, &metadata_budget)?.run(|tx| {
            // Replay precedes source checks and caller cancellation. A prior commit
            // is a recorded fact, never an instruction to install an old snapshot.
            if let Some(receipt) =
                recorded_commit_with_budget(tx, intent, &request, &metadata_budget)?
            {
                tx.commit()?;
                return Ok(receipt);
            }
            tx.switch_governor(budget)?;
            let _files = crate::retained_file_coordination::writer(&self.root)?;
            budget.checkpoint()?;
            let current = read_document_metadata_with_budget(tx, &intent.project_id.0, budget)?;
            if current.revision() != intent.expected_revision {
                return Err(StoreError::Conflict);
            }
            if current.tempo().to_bits() != intent.request.tempo_bits {
                return Err(invalid("Projection tempo differs from the current project"));
            }
            let source = TakeReference {
                take_id: omniusong_core::contracts::TakeId(intent.request.source.take_id.clone()),
                manifest_sha256: intent.request.source.manifest_sha256.clone(),
                stream_id: "audio".into(),
            };
            let sources = RegisteredProjectionSources::open(
                &self.root,
                tx,
                &[&current],
                std::slice::from_ref(&source),
                budget,
            )?;
            let proofs = sources.authenticate(budget)?;
            proofs.validate_document(&current)?;
            let binding = proofs.performance(&source)?.build_binding(
                &intent.projection_id,
                &intent.request,
                &intent.expected_review_sha256,
                &intent.lanes,
            )?;
            let candidate = current
                .append_performance_projection(
                    &binding,
                    &intent.lanes,
                    intent.timeline,
                    intent.allow_projection_schema_upgrade,
                )
                .map_err(StoreError::EditRejected)?;
            let saved = write_save_metadata(tx, &current, &candidate, intent.expected_revision)?;
            insert_operation_metadata(
                tx,
                &intent.operation_id.0,
                intent.expected_revision,
                &request,
                &saved.document,
            )?;
            verify_saved_metadata(tx, &saved, budget)?;
            // Read the exact recorded postimage while rollback is still possible.
            let mut receipt = recorded_commit_with_budget(tx, intent, &request, budget)?
                .ok_or_else(|| invalid("Projection receipt disappeared inside transaction"))?;
            if receipt.result_sha256 != checksum(&saved.document.to_json()?) {
                return Err(invalid("Projection result changed inside transaction"));
            }
            receipt.replayed = false;
            receipt.validate()?;
            check_assets(tx, &[&current], budget)?;
            proofs.verify_registrations(tx)?;
            proofs.verify_unchanged()?;
            budget.checkpoint()?;
            tx.commit()?;
            // No fallible work or cancellation check may erase successful commit.
            Ok(receipt)
        })
    }
}

/// Authenticate the recorded forward transition, including during Undo. Registry
/// removals or rewrites are never fresh publication. Caller holds source proofs.
pub(crate) fn validate_forward_transition(
    connection: &Connection,
    before: &ProjectDocument,
    after: &ProjectDocument,
    budget: &InspectionBudget,
) -> Result<()> {
    validate_forward_transition_in(connection, before, after, budget, false)
}

/// Only for a separately strict-schema-validated OSAR v1 snapshot. This format
/// cannot contain copy admissions; active catalog callers retain their guard.
pub(crate) fn validate_forward_transition_archive_v1(
    connection: &Connection,
    before: &ProjectDocument,
    after: &ProjectDocument,
    budget: &InspectionBudget,
) -> Result<()> {
    validate_forward_transition_in(connection, before, after, budget, true)
}
fn validate_forward_transition_in(
    connection: &Connection,
    before: &ProjectDocument,
    after: &ProjectDocument,
    budget: &InspectionBudget,
    archive_v1: bool,
) -> Result<()> {
    crate::projection_boundary::reject_opaque_promotion(before, after)?;
    let old = before
        .performance_projections()?
        .map(|r| r.entries)
        .unwrap_or_default();
    let new = after
        .performance_projections()?
        .map(|r| r.entries)
        .unwrap_or_default();
    if old == new {
        return Ok(());
    }
    if new.len() != old.len() + 1 || new[..old.len()] != old {
        return Err(invalid(
            "Stored history rewrites original projection ownership",
        ));
    }
    let binding = new.last().expect("one append");
    let op = crate::projection_operation_id(
        &omniusong_core::contracts::ProjectId(before.id().into()),
        &binding.id,
    )?;
    let request_length: Option<i64> = connection
        .query_row(
            "SELECT octet_length(request_json) FROM operations WHERE operation_id=?1",
            [&op.0],
            |r| r.get(0),
        )
        .optional()?;
    let request_length = request_length
        .ok_or_else(|| invalid("Stored projection history has no publication request"))?;
    if !(0..=omniusong_core::contracts::WORKER_MAX_BYTES as i64).contains(&request_length) {
        return Err(invalid("Projection history request exceeds wire bounds"));
    }
    budget.charge_work((request_length as u64).saturating_mul(64) + 4096)?;
    let request:Option<String>=connection.query_row("SELECT CASE WHEN octet_length(request_json)<=?2 THEN request_json END FROM operations WHERE operation_id=?1",params![op.0,omniusong_core::contracts::WORKER_MAX_BYTES as i64],|r|r.get(0))?;
    let intent: ProjectionPublication = serde_json::from_str(
        request
            .as_deref()
            .ok_or_else(|| invalid("Projection history request is oversized"))?,
    )
    .map_err(|e| invalid(e.to_string()))?;
    let canonical = intent.canonical_json()?;
    if request.as_deref() != Some(canonical.as_str())
        || intent.project_id.0 != before.id()
        || intent.projection_id != binding.id
        || intent.expected_revision != before.revision()
    {
        return Err(invalid("Stored history publication request differs"));
    }
    let receipt = if archive_v1 {
        // The retained walk also checks the exact canonical request/result
        // bytes. Bind this history edge to those same operation cells using
        // fixed-sized stored hashes/revisions, without a nonexistent table.
        budget.charge_work(4096)?;
        budget.visit_rows(1)?;
        let (request_sha,result_sha,previous,committed):(Option<String>,Option<String>,i64,i64)=connection.query_row(
            "SELECT CASE WHEN octet_length(request_sha256)=64 THEN request_sha256 END,CASE WHEN octet_length(result_sha256)=64 THEN result_sha256 END,previous_revision,committed_revision FROM operations WHERE operation_id=?1 AND project_id=?2",
            params![op.0,before.id()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
        let expected_sha = checksum(&after.to_json()?);
        if request_sha.as_deref() != Some(checksum(canonical.as_bytes()).as_str())
            || result_sha.as_deref() != Some(expected_sha.as_str())
            || u64::try_from(previous).ok() != Some(before.revision())
            || u64::try_from(committed).ok() != Some(after.revision())
        {
            return Err(invalid("Stored projection history result differs"));
        }
        validate_publication_result(&intent, &canonical, after, &expected_sha, budget)?
    } else {
        recorded_commit_with_budget(connection, &intent, &canonical, budget)?
            .ok_or_else(|| invalid("Stored projection history has no publication receipt"))?
    };
    if receipt.committed_revision != after.revision()
        || receipt.result_sha256 != checksum(&after.to_json()?)
    {
        return Err(invalid("Stored projection history result differs"));
    }
    // The active caller authenticated every binding in `after` against held
    // originals. The exact prefix proves those in `before` unchanged. This
    // metadata check reconstructs the initial append, not another derivation.
    let expected = before
        .append_performance_projection(
            binding,
            &intent.lanes,
            intent.timeline,
            intent.allow_projection_schema_upgrade,
        )?
        .with_revision(after.revision())?;
    if expected != *after {
        return Err(invalid(
            "Stored projection history is not the complete initial candidate",
        ));
    }
    Ok(())
}

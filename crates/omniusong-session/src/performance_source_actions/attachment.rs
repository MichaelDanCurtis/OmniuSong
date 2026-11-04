//! Explicit attachment is separate from copied recovery and local disposition.
use super::decisions::{
    PerformanceSourceAttachmentStatus, PerformanceSourceDecisionEvent,
    PerformanceSourceDecisionRevision, committed_payload,
};
use super::*;
use crate::DocumentView;
use omniusong_engine::performance_copy::authenticate_owned_performance_copy;
use omniusong_recording::SealedJournal;
use omniusong_store::{SourceAttachmentCommit, TakePublication};

#[derive(Debug)]
pub struct PerformanceSourceAttachmentReceipt {
    pub requested: PerformanceSourceActionReference,
    pub state: PerformanceSourceActionState,
    pub publication: TakePublication,
    /// Historical exact receipt, never a claim about current arrangement placement.
    pub committed: bool,
    pub replayed: bool,
    pub current_document: Option<DocumentView>,
    pub current_document_error: Option<ServiceError>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttachmentBoundary {
    Ingested,
    Published,
}
fn source_store(error: omniusong_store::StoreError) -> ServiceError {
    match error {
        error @ (omniusong_store::StoreError::Database(_)
        | omniusong_store::StoreError::Catalog(_)
        | omniusong_store::StoreError::Archive(_)
        | omniusong_store::StoreError::ArchiveSql(_)) => store_failure(error, false),
        omniusong_store::StoreError::Recording(error) => local(error),
        error => unavailable(error.to_string()),
    }
}
fn verified_result(
    state: &PerformanceSourceActionState,
) -> Result<&RecoveredPerformanceSourceResult, ServiceError> {
    match &state.execution {
        Some(PerformanceSourceExecutionRevision {
            phase: PerformanceSourceExecutionPhase::Verified { result, .. },
            ..
        }) => Ok(result),
        _ => Err(ServiceError::rejected(
            "Attachment requires a Verified copied recovery",
        )),
    }
}
fn authenticate_owned(
    sealed: &SealedJournal,
    state: &PerformanceSourceActionState,
    source: &PerformanceSourceReview,
) -> Result<(), ServiceError> {
    authenticate_owned_performance_copy(
        sealed,
        &source.performance,
        &state.prepared.action.id,
        &state.prepared.action.recovery_operation_id.0,
    )
    .map_err(local)?;
    let expected = verified_result(state)?;
    let actual = sealed.inspect_snapshot().map_err(local)?;
    require(
        actual.descriptor.take_id.0 == expected.take_id
            && actual.descriptor_sha256 == expected.descriptor_sha256
            && actual.original_rows_sha256 == expected.original_rows_sha256
            && sealed.manifest().sha256 == expected.manifest_sha256
            && actual.report.unknown_tail,
        "Owned original differs from the immutable Verified recovered result",
    )
}
impl NativeBackend {
    pub(crate) fn attach_source_choice(
        &mut self,
        id: &str,
        revision: u64,
        payload_sha256: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.run_source_attachment(
            Request {
                id,
                revision,
                payload_sha256,
            },
            cancelled,
            true,
            |_, _| Ok(()),
        )
        .map(|r| Reply::PerformanceSourceAttachment(Box::new(r)))
    }
    pub(crate) fn reconcile_source_attachment(
        &mut self,
        id: &str,
        revision: u64,
        payload_sha256: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        self.run_source_attachment(
            Request {
                id,
                revision,
                payload_sha256,
            },
            cancelled,
            false,
            |_, _| Ok(()),
        )
        .map(|r| Reply::PerformanceSourceAttachment(Box::new(r)))
    }
    fn attachment_receipt(
        &mut self,
        requested: &Request<'_>,
        state: PerformanceSourceActionState,
        publication: TakePublication,
        committed: bool,
        replayed: bool,
        budget: &InspectionBudget,
    ) -> PerformanceSourceAttachmentReceipt {
        let current = if committed {
            Some(
                self.store()
                    .and_then(|s| {
                        s.load_with_budget(&publication.project_id.0, budget)
                            .map_err(source_store)
                    })
                    .and_then(DocumentView::prepare),
            )
        } else {
            None
        };
        let (current_document, current_document_error) = match current {
            Some(Ok(view)) => (Some(view), None),
            Some(Err(error)) => (None, Some(error)),
            None => (None, None),
        };
        PerformanceSourceAttachmentReceipt {
            requested: PerformanceSourceActionReference::new(
                requested.id,
                requested.revision,
                requested.payload_sha256,
            ),
            state,
            publication,
            committed,
            replayed,
            current_document,
            current_document_error,
        }
    }
    fn run_source_attachment(
        &mut self,
        request: Request<'_>,
        cancelled: Arc<AtomicBool>,
        execute: bool,
        mut observe: impl FnMut(AttachmentBoundary, &mut Self) -> Result<(), ServiceError>,
    ) -> Result<PerformanceSourceAttachmentReceipt, ServiceError> {
        let loaded = self.load_execution_history()?;
        let state = loaded.authenticate(&request)?;
        let row = loaded
            .history
            .revisions
            .iter()
            .find(|r| r.id == request.id && r.revision == request.revision)
            .ok_or_else(|| ServiceError::rejected("Attachment choice row is missing"))?;
        let revision: PerformanceSourceDecisionRevision = serde_json::from_slice(&row.payload)
            .map_err(|_| {
                ServiceError::rejected("Choose an exact retained AttachmentPrepared row")
            })?;
        let PerformanceSourceDecisionEvent::AttachmentPrepared { choice } = revision.decision
        else {
            return Err(ServiceError::rejected(
                "Choose an exact retained AttachmentPrepared row",
            ));
        };
        let current = state.decisions.attachment.as_ref().filter(|a| {
            a.revision == request.revision
                && a.payload_sha256 == request.payload_sha256
                && a.choice == choice
        });
        // Ledger and receipt are one read snapshot. No path availability may
        // erase or substitute the historical commitment.
        let receipt = self
            .store()?
            .take_publication_receipt_if_source_history_matches(
                &choice.publication,
                &loaded.history,
            )
            .map_err(|e| uncertain(store_failure(e, false)))?;
        let recorded = current.is_some_and(|a| {
            matches!(
                a.status,
                PerformanceSourceAttachmentStatus::Committed { .. }
            )
        });
        if recorded != receipt.is_some() {
            return Err(uncertain(unavailable(
                "Source attachment ledger and exact historical receipt disagree",
            )));
        }
        let budget =
            InspectionBudget::new(InspectionLimits::default(), cancelled).map_err(local)?;
        if receipt.is_some() {
            return Ok(self.attachment_receipt(
                &request,
                state,
                choice.publication,
                true,
                true,
                &budget,
            ));
        }
        if !execute {
            return Ok(self.attachment_receipt(
                &request,
                state,
                choice.publication,
                false,
                true,
                &budget,
            ));
        }
        require(
            current.is_some_and(|a| a.status == PerformanceSourceAttachmentStatus::Pending)
                && !state.decisions.kept_locally,
            "Only the current pending reviewed attachment may execute",
        )?;
        budget.checkpoint().map_err(local)?;
        // Reject unsupported project ownership before a recovered object can
        // be ingested. This is only metadata preflight; commit revalidates the
        // exact revision, complete source history and original identities.
        self.store()?
            .check_project_metadata_admission_with_budget(&choice.publication.project_id.0, &budget)
            .map_err(source_store)?;
        budget.checkpoint().map_err(local)?;
        let source = review(loaded.raw_review(request.id)?)?.0;
        let payload = committed_payload(
            &state,
            choice
                .publication
                .expected_revision
                .checked_add(1)
                .ok_or_else(|| ServiceError::rejected("Project revision exhausted"))?,
        )?;
        let parent = parent_directory(&loaded.root, false)?;
        let owner = CopyActionOwner::open_existing(&parent, request.id).map_err(local)?;
        // A registered original is authoritative. Any damage or identity
        // conflict refuses; never fall back to action working bytes.
        let registered = self
            .store()?
            .has_registered_take(&choice.publication.clip.source.take_id)
            .map_err(|e| store_failure(e, false))?;
        let result=(|| {
            let publish=|this:&mut Self,observe:&mut dyn FnMut(AttachmentBoundary,&mut Self)->Result<(),ServiceError>|->Result<PerformanceSourceActionState,ServiceError> {
                observe(AttachmentBoundary::Ingested,this)?;
                owner.verify().map_err(local)?;budget.checkpoint().map_err(local)?;
                let (edit,row)=this.store()?.publish_source_attachment(SourceAttachmentCommit {
                    publication:&choice.publication,id:request.id,expected_revision:state.revision,
                    review:loaded.raw_review(request.id)?,committed_payload:&payload,expected_history:&loaded.history,budget:&budget,
                }).map_err(|e| match e { error @ (omniusong_store::StoreError::Conflict | omniusong_store::StoreError::Database(_) | omniusong_store::StoreError::Catalog(_))=>store_failure(error,true), error=>source_store(error) })?;
                require(row.id==request.id && row.revision==state.revision+1 && row.payload==payload
                    && row.payload_sha256==digest(b"",&payload) && row.review_blob_sha256==state.prepared.action.review_blob_sha256
                    && edit.document.revision()==choice.publication.expected_revision+1,"Committed source attachment acknowledgement differs from its exact request")?;
                observe(AttachmentBoundary::Published,this)?;
                owner.verify().map_err(local)?;
                let now=this.load_execution_history()?;
                let current=now.authenticate(&request)?;
                require(current.decisions.attachment.as_ref().is_some_and(|a|a.choice==choice
                    && matches!(a.status,PerformanceSourceAttachmentStatus::Committed{project_revision} if project_revision==edit.document.revision())),
                    "Committed source attachment is not retained in current validated history")?;
                Ok(current)
            };
            if registered {
                let sealed=self.store()?.open_take_with_budget(&choice.publication.clip.source,&budget).map_err(source_store)?;
                authenticate_owned(&sealed,&state,&source)?;
                let now=publish(self,&mut observe)?;
                owner.verify().map_err(local)?;
                Ok(now)
            } else {
                let view=RecoveredCopyView::open(&owner,&source.performance.journal_sha256_at_review,source.byte_length,&budget).map_err(local)?;
                let proof=reconcile_performance_copy(&view,&source.performance,request.id,&state.prepared.action.recovery_operation_id.0).map_err(local)?;
                require(observed(&proof,&source)?==*verified_result(&state)?,"Current recovered copy differs from Verified")?;
                self.store()?.ingest_recovered_copy(&view).map_err(source_store)?;
                view.verify_unchanged().map_err(local)?;
                let now=publish(self,&mut observe)?;
                view.verify_unchanged().map_err(local)?;
                Ok(now)
            }
        })().map_err(uncertain)?;
        Ok(self.attachment_receipt(&request, result, choice.publication, true, false, &budget))
    }
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;

//! Atomic source-choice retirement and recovered project publication.
use crate::{
    EditReceipt, NativeStore, PerformanceSourceActionHistory, Result, StoreError,
    StoredPerformanceSourceAction, TakePublication, load_document,
    operations::insert_operation_inner,
    performance_attachment_check::checked_publication_receipt,
    performance_source_actions::{append_in_transaction, load_history},
    save_in_transaction_with_budget,
    takes::publication_request,
};
use omniusong_recording::InspectionBudget;
use rusqlite::TransactionBehavior;

pub const SOURCE_ATTACHMENT_OPERATION_PREFIX: &str = "os-src-attach:";
pub(crate) fn reject_reserved_operation(id: &str) -> Result<()> {
    if id.starts_with(SOURCE_ATTACHMENT_OPERATION_PREFIX) {
        Err(StoreError::Invalid("Source attachment operation requires its dedicated history-fenced bridge (restore this song as a copy)".into()))
    } else {
        Ok(())
    }
}
pub struct SourceAttachmentCommit<'a> {
    pub publication: &'a TakePublication,
    pub id: &'a str,
    pub expected_revision: u64,
    pub review: &'a [u8],
    pub committed_payload: &'a [u8],
    pub expected_history: &'a PerformanceSourceActionHistory,
    pub budget: &'a InspectionBudget,
}
impl NativeStore {
    /// One metadata snapshot for the validated action history and receipt.
    pub fn take_publication_receipt_if_source_history_matches(
        &self,
        publication: &TakePublication,
        expected_history: &PerformanceSourceActionHistory,
    ) -> Result<Option<EditReceipt>> {
        let tx = self.connection.unchecked_transaction()?;
        if &load_history(&tx)? != expected_history {
            return Err(StoreError::Conflict);
        }
        let result = checked_publication_receipt(&tx, publication)?;
        tx.rollback()?;
        Ok(result)
    }
    /// Absence and retirement append are one fenced transaction. An old bridge
    /// can no longer publish after this succeeds, even if ingestion finishes.
    pub fn save_performance_source_action_if_history_matches_and_unpublished(
        &mut self,
        id: &str,
        expected_revision: Option<u64>,
        review: &[u8],
        payload: &[u8],
        expected_history: &PerformanceSourceActionHistory,
        publication: &TakePublication,
    ) -> Result<StoredPerformanceSourceAction> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if &load_history(&tx)? != expected_history {
            return Err(StoreError::Conflict);
        }
        if checked_publication_receipt(&tx, publication)?.is_some() {
            return Err(StoreError::Invalid(
                "Committed source publication cannot be claimed or retired".into(),
            ));
        }
        let row = append_in_transaction(
            &tx,
            id,
            expected_revision,
            review,
            payload,
            Some(expected_history),
        )?;
        tx.commit()?;
        Ok(row)
    }
    /// The caller authenticates the complete typed choice and holds its source
    /// view. All resulting owned-file reproof uses that same source governor.
    pub fn publish_source_attachment(
        &mut self,
        commit: SourceAttachmentCommit<'_>,
    ) -> Result<(EditReceipt, StoredPerformanceSourceAction)> {
        let SourceAttachmentCommit {
            publication,
            id,
            expected_revision,
            review,
            committed_payload,
            expected_history,
            budget,
        } = commit;
        let op = &publication.operation_id.0;
        if !op.starts_with(SOURCE_ATTACHMENT_OPERATION_PREFIX)
            || op.len() != SOURCE_ATTACHMENT_OPERATION_PREFIX.len() + 64
            || !op[SOURCE_ATTACHMENT_OPERATION_PREFIX.len()..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(StoreError::Invalid(
                "Invalid dedicated source attachment operation".into(),
            ));
        }
        let request = publication_request(publication)?;
        budget.checkpoint()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if &load_history(&tx)? != expected_history {
            return Err(StoreError::Conflict);
        }
        if checked_publication_receipt(&tx, publication)?.is_some() {
            return Err(StoreError::Invalid(
                "An existing publication must be reconciled before fresh source attachment".into(),
            ));
        }
        let current = load_document(&tx, &publication.project_id.0)?;
        if current.revision() != publication.expected_revision {
            return Err(StoreError::Conflict);
        }
        let before = current.to_json()?;
        let cursor: i64 = tx.query_row(
            "SELECT cursor FROM projects WHERE id=?1",
            [current.id()],
            |r| r.get(0),
        )?;
        let expected_cursor = cursor
            .checked_add(1)
            .ok_or_else(|| StoreError::Invalid("Edit cursor exhausted".into()))?;
        let mut next = if current.schema_version() == 1 {
            current.upgrade_native()?
        } else {
            current
        };
        next.append_take_clip(
            &publication.track_id.0,
            &publication.track_name,
            publication.clip.clone(),
        )?;
        let document = save_in_transaction_with_budget(
            &tx,
            &self.root,
            &next,
            publication.expected_revision,
            Some(budget),
        )?;
        insert_operation_inner(&tx, op, publication.expected_revision, &request, &document)?;
        let row = append_in_transaction(
            &tx,
            id,
            Some(expected_revision),
            review,
            committed_payload,
            Some(expected_history),
        )?;
        let proof = checked_publication_receipt(&tx, publication)?.ok_or_else(|| {
            StoreError::Invalid("Source attachment receipt disappeared inside transaction".into())
        })?;
        if proof.document != document {
            return Err(StoreError::Invalid(
                "Source attachment postimage changed inside transaction".into(),
            ));
        }
        let current = load_document(&tx, &publication.project_id.0)?;
        let actual_cursor: i64 = tx.query_row(
            "SELECT cursor FROM projects WHERE id=?1",
            [current.id()],
            |r| r.get(0),
        )?;
        let (edit_before, edit_after): (String, String) = tx.query_row(
            "SELECT before_json,after_json FROM edits WHERE project_id=?1 AND sequence=?2",
            rusqlite::params![current.id(), expected_cursor],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let extra: i64 = tx.query_row(
            "SELECT count(*) FROM edits WHERE project_id=?1 AND sequence>?2",
            rusqlite::params![current.id(), expected_cursor],
            |r| r.get(0),
        )?;
        if current != document
            || actual_cursor != expected_cursor
            || edit_before.as_bytes() != before
            || edit_after.as_bytes() != document.to_json()?
            || extra != 0
        {
            return Err(StoreError::Invalid(
                "Source attachment project/edit postimage changed inside transaction".into(),
            ));
        }

        budget.checkpoint()?;
        tx.commit()?;
        Ok((
            EditReceipt {
                operation_id: op.clone(),
                previous_revision: publication.expected_revision,
                document,
                replayed: false,
            },
            row,
        ))
    }
}

#[cfg(test)]
#[path = "source_attachment_tests.rs"]
mod tests;

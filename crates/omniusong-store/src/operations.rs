//! Durable admission for the implemented project-edit operations.
//!
//! An operation retry returns its original committed result, even when newer
//! edits exist. It never overwrites those edits or silently rebases the retry.

use super::{
    NativeStore, Result, StoreError, check_references, checksum, load_document,
    navigate_in_transaction, save_in_transaction,
};
use omniusong_core::{
    ProjectDocument,
    contracts::{EditOperation, ProjectEditIntent, Validate, WORKER_MAX_BYTES},
    native::MAX_NATIVE_PROJECT_BYTES,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

#[derive(Debug, Clone, PartialEq)]
pub struct EditReceipt {
    pub operation_id: String,
    pub previous_revision: u64,
    /// Snapshot produced by this operation, not necessarily the latest project.
    pub document: ProjectDocument,
    pub replayed: bool,
}

struct StoredOperation {
    project_id: String,
    request: String,
    request_sha256: String,
    previous_revision: i64,
    committed_revision: i64,
    result: String,
    result_sha256: String,
}

pub(crate) fn replay_operation(
    connection: &rusqlite::Connection,
    operation_id: &str,
    project_id: &str,
    expected_revision: u64,
    request: &str,
) -> Result<Option<EditReceipt>> {
    let copy_identity: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM archive_copies WHERE operation_id=?1)",
        [operation_id],
        |row| row.get(0),
    )?;
    if copy_identity {
        return Err(StoreError::Invalid(
            "Operation identity was already used for an archive copy".into(),
        ));
    }
    let existing = connection.query_row(
        "SELECT project_id,CASE WHEN octet_length(request_json)<=?2 THEN request_json END,CASE WHEN octet_length(request_sha256)=64 THEN request_sha256 END,previous_revision,committed_revision,CASE WHEN octet_length(result_json)<=?3 THEN result_json END,CASE WHEN octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE operation_id=?1",
        params![operation_id,WORKER_MAX_BYTES as i64,MAX_NATIVE_PROJECT_BYTES as i64],
        |row| Ok(StoredOperation {
            project_id: row.get(0)?, request: row.get(1)?, request_sha256: row.get(2)?,
            previous_revision: row.get(3)?, committed_revision: row.get(4)?,
            result: row.get(5)?, result_sha256: row.get(6)?,
        }),
    ).optional()?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    if checksum(existing.request.as_bytes()) != existing.request_sha256
        || checksum(existing.result.as_bytes()) != existing.result_sha256
    {
        return Err(StoreError::Invalid(
            "Stored operation receipt is damaged".into(),
        ));
    }
    if existing.request != request || existing.project_id != project_id {
        return Err(StoreError::Invalid(
            "Operation identity was already used for different content".into(),
        ));
    }
    let document = ProjectDocument::from_json(existing.result.as_bytes())?;
    if document.id() != project_id
        || u64::try_from(existing.previous_revision).ok() != Some(expected_revision)
        || existing.previous_revision.checked_add(1) != Some(existing.committed_revision)
        || u64::try_from(existing.committed_revision).ok() != Some(document.revision())
    {
        return Err(StoreError::Invalid(
            "Stored operation identity/revision is inconsistent".into(),
        ));
    }
    check_references(connection, &document)?;
    Ok(Some(EditReceipt {
        operation_id: operation_id.into(),
        previous_revision: expected_revision,
        document,
        replayed: true,
    }))
}

pub(crate) fn insert_operation(
    connection: &rusqlite::Connection,
    operation_id: &str,
    expected_revision: u64,
    request: &str,
    document: &ProjectDocument,
) -> Result<()> {
    let previous = i64::try_from(expected_revision)
        .map_err(|_| StoreError::Invalid("Operation revision is out of range".into()))?;
    let committed = i64::try_from(document.revision())
        .map_err(|_| StoreError::Invalid("Operation revision is out of range".into()))?;
    if previous.checked_add(1) != Some(committed) || request.len() > WORKER_MAX_BYTES {
        return Err(StoreError::Invalid(
            "Invalid operation receipt boundary".into(),
        ));
    }
    let result = String::from_utf8(document.to_json()?).expect("JSON is UTF-8");
    connection.execute(
        "INSERT INTO operations(operation_id,project_id,request_json,request_sha256,previous_revision,committed_revision,result_json,result_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![operation_id,document.id(),request,checksum(request.as_bytes()),previous,committed,result,checksum(result.as_bytes())],
    )?;
    Ok(())
}

impl NativeStore {
    /// Supported basic edits publish atomically with a globally unique receipt.
    /// Prepared edits and candidates still require their own semantic admission.
    pub fn apply_edit(&mut self, intent: &ProjectEditIntent) -> Result<EditReceipt> {
        intent
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let request = serde_json::to_string(intent)
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if request.len() > WORKER_MAX_BYTES {
            return Err(StoreError::Invalid(
                "Project edit exceeds wire limit".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(receipt) = replay_operation(
            &tx,
            &intent.operation_id.0,
            &intent.project_id.0,
            intent.expected_revision,
            &request,
        )? {
            super::takes::verify_document_files(&self.root, &tx, &receipt.document)?;
            tx.commit()?;
            return Ok(receipt);
        }
        let current = load_document(&tx, &intent.project_id.0)?;
        if current.revision() != intent.expected_revision {
            return Err(StoreError::Conflict);
        }
        let document=match &intent.edit {
            EditOperation::Rename{name}=>{let mut next=current;next.rename(name)?;save_in_transaction(&tx,&self.root,&next,intent.expected_revision)?},
            EditOperation::SetTempo{bpm}=>{let mut next=current;next.set_tempo(*bpm)?;save_in_transaction(&tx,&self.root,&next,intent.expected_revision)?},
            EditOperation::UpdateSession { name, tempo } => {
                let mut next = current;
                next.rename(name)?;
                next.set_tempo(*tempo)?;
                save_in_transaction(&tx, &self.root, &next, intent.expected_revision)?
            }
            EditOperation::Undo|EditOperation::Redo=>navigate_in_transaction(&tx,&self.root,&intent.project_id.0,intent.expected_revision,matches!(intent.edit,EditOperation::Redo))?,
            EditOperation::ApplyPreparedEdit{..}|EditOperation::AcceptCandidate{..}=>return Err(StoreError::Invalid("Prepared edits and candidate acceptance are not implemented in the native store".into())),
        };
        insert_operation(
            &tx,
            &intent.operation_id.0,
            intent.expected_revision,
            &request,
            &document,
        )?;
        tx.commit()?;
        Ok(EditReceipt {
            operation_id: intent.operation_id.0.clone(),
            previous_revision: intent.expected_revision,
            document,
            replayed: false,
        })
    }
}

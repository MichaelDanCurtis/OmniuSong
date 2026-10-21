//! Durable admission for the implemented project-edit operations.
//!
//! An operation retry returns its original committed result, even when newer
//! edits exist. It never overwrites those edits or silently rebases the retry.

use super::{
    NativeStore, Result, StoreError, check_references, checksum, load_document,
    navigate_in_transaction, save_in_transaction,
};
use omniusong_core::{
    ClipPatch, DocumentClipAddress, DocumentError, ProjectDocument, ValueChange,
    contracts::{EditOperation, EditScope, ProjectEditIntent, Validate, WORKER_MAX_BYTES},
    native::{MAX_NATIVE_PROJECT_BYTES, TakeRange},
};
use omniusong_recording::{SourcePosition, StreamKind};
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

/// Called only after current references and owned bytes have been authenticated.
/// A new requested window outside that same proven audio prefix is a known user
/// rejection. Every identity, source-integrity and storage error stays conservative.
fn check_requested_audio_window(
    connection: &rusqlite::Connection,
    current: &ProjectDocument,
    candidate: &ProjectDocument,
    address: &DocumentClipAddress,
    patch: &ClipPatch,
) -> Result<()> {
    let (DocumentClipAddress::NativeAudio { clip_id, .. }, ClipPatch::NativeAudio(patch)) =
        (address, patch)
    else {
        return Ok(());
    };
    let ValueChange::Set { value: requested } = &patch.source_window else {
        return Ok(());
    };
    let before = current
        .take_clips()?
        .into_iter()
        .find(|clip| &clip.id == clip_id)
        .ok_or_else(|| StoreError::Invalid("Current audio clip disappeared".into()))?;
    let after = candidate
        .take_clips()?
        .into_iter()
        .find(|clip| &clip.id == clip_id)
        .ok_or_else(|| StoreError::Invalid("Candidate audio clip disappeared".into()))?;
    if before.source != after.source
        || before.position.sample_rate != after.position.sample_rate
        || !matches!(before.range, TakeRange::AudioFrames { .. })
        || after.range
            != (TakeRange::AudioFrames {
                start: requested.start_frame,
                count: requested.frame_count,
            })
    {
        return Err(StoreError::Invalid(
            "Clip edit changed its authenticated audio source identity or unit".into(),
        ));
    }
    let object = super::takes::load_take(connection, &before.source.take_id)?;
    let stream = object
        .manifest
        .body
        .descriptor
        .streams
        .iter()
        .find(|stream| stream.stream_id == before.source.stream_id)
        .ok_or_else(|| StoreError::Invalid("Authenticated audio stream is missing".into()))?;
    let prefix = object
        .manifest
        .body
        .streams
        .iter()
        .find(|prefix| prefix.stream_id == before.source.stream_id)
        .ok_or_else(|| StoreError::Invalid("Authenticated audio prefix is missing".into()))?;
    let (StreamKind::Audio { format, .. }, SourcePosition::AudioFrames(end)) =
        (&stream.kind, &prefix.source_end)
    else {
        return Err(StoreError::Invalid(
            "Authenticated source is not an audio frame stream".into(),
        ));
    };
    if object.manifest_sha256 != before.source.manifest_sha256
        || format.sample_rate != before.position.sample_rate
    {
        return Err(StoreError::Invalid(
            "Authenticated audio identity or rate changed".into(),
        ));
    }
    if after.range.start() < stream.initial_source_position || after.range.end()? > *end {
        return Err(StoreError::EditRejected(DocumentError(format!(
            "Requested source window must stay within verified audio frames {}–{}",
            stream.initial_source_position, end
        ))));
    }
    Ok(())
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
        if matches!(
            intent.edit,
            EditOperation::UpdateClip { .. } | EditOperation::RestoreClip { .. }
        ) {
            check_references(&tx, &current)?;
            super::takes::verify_document_files(&self.root, &tx, &current)?;
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
            EditOperation::AddNote {values,timeline}|EditOperation::UpdateNote {values,timeline}=>{
                let EditScope::DocumentNote{address}=&intent.scope else{return Err(StoreError::Invalid("Note edit requires document-note scope".into()));};
                let next=if matches!(intent.edit,EditOperation::AddNote{..}){current.add_note(address,*values,*timeline)}else{current.update_note(address,*values,*timeline)}.map_err(StoreError::EditRejected)?;
                save_in_transaction(&tx,&self.root,&next,intent.expected_revision)?
            },
            EditOperation::DeleteNote|EditOperation::RestoreNote{..}=>{
                let EditScope::DocumentNote{address}=&intent.scope else{return Err(StoreError::Invalid("Note edit requires document-note scope".into()));};
                let next=match &intent.edit {EditOperation::RestoreNote{timeline}=>current.restore_note(address,*timeline),_=>current.delete_note(address)}.map_err(StoreError::EditRejected)?;
                save_in_transaction(&tx,&self.root,&next,intent.expected_revision)?
            },
            EditOperation::UpdateClip{patch,timeline}=>{
                let EditScope::DocumentClip{address}=&intent.scope else{return Err(StoreError::Invalid("Clip edit requires document-clip scope".into()));};
                let next=current.update_clip(address,patch,*timeline).map_err(StoreError::EditRejected)?;
                check_requested_audio_window(&tx, &current, &next, address, patch)?;
                save_in_transaction(&tx,&self.root,&next,intent.expected_revision)?
            },
            EditOperation::RestoreClip{timeline}=>{
                let EditScope::DocumentClip{address}=&intent.scope else{return Err(StoreError::Invalid("Clip edit requires document-clip scope".into()));};
                let next=current.restore_clip(address,*timeline).map_err(StoreError::EditRejected)?;
                save_in_transaction(&tx,&self.root,&next,intent.expected_revision)?
            },
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

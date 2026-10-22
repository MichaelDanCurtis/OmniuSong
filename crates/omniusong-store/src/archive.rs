//! Complete, bounded per-project archives. All work belongs on storage threads.

use crate::{
    NativeStore, Result, StoreError, checksum, load_document, read_asset,
    takes::{
        TakeObject, TakePublication, load_take, object_path, publication_request, verify_owned_take,
    },
};
use omniusong_core::{
    ClipPatch, ClipTimelinePolicy, DocumentClipAddress, FieldChange, MAX_NATIVE_PROJECT_BYTES,
    MAX_PROJECT_BYTES, NoteTimelinePolicy, NoteValues, PPQ, ProjectDocument, ValueChange,
    contracts::{EditOperation, EditScope, ProjectEditIntent, TakeId, Validate, WORKER_MAX_BYTES},
    native::TakeReference,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const ARCHIVE_APPLICATION_ID: u32 = 0x4f53_4152;
pub const ARCHIVE_VERSION: u32 = 2;
pub const ARCHIVE_CHUNK_BYTES: usize = 1024 * 1024;
pub const ARCHIVE_METADATA_BYTES: u64 = 256 * 1024 * 1024;
pub const ARCHIVE_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
pub(super) const MAX_ARCHIVE_FILE_BYTES: u64 = 2 * ARCHIVE_PAYLOAD_BYTES;
const MAX_EDITS: u64 = 10_000;
const MAX_OPERATIONS: u64 = 100_000;
const MAX_OBJECTS: usize = 4096;
const MAX_MANIFEST_BYTES: usize = 512 * 1024;

const TABLES: &[(&str, &str)] = &[
    (
        "archive_meta",
        "CREATE TABLE archive_meta(id INTEGER PRIMARY KEY CHECK(id=1),project_id TEXT NOT NULL,metadata_bytes INTEGER NOT NULL,asset_bytes INTEGER NOT NULL,take_bytes INTEGER NOT NULL,edit_count INTEGER NOT NULL,operation_count INTEGER NOT NULL,import_count INTEGER NOT NULL,asset_count INTEGER NOT NULL,take_count INTEGER NOT NULL,chunk_count INTEGER NOT NULL,index_sha256 TEXT NOT NULL)",
    ),
    (
        "projects",
        "CREATE TABLE projects(id TEXT PRIMARY KEY NOT NULL,payload TEXT NOT NULL,revision INTEGER NOT NULL,cursor INTEGER NOT NULL,payload_sha256 TEXT NOT NULL)",
    ),
    (
        "edits",
        "CREATE TABLE edits(project_id TEXT NOT NULL,sequence INTEGER NOT NULL,before_json TEXT NOT NULL,after_json TEXT NOT NULL,before_sha256 TEXT NOT NULL,after_sha256 TEXT NOT NULL,PRIMARY KEY(project_id,sequence))",
    ),
    (
        "operations",
        "CREATE TABLE operations(operation_id TEXT PRIMARY KEY NOT NULL,project_id TEXT NOT NULL,request_json TEXT NOT NULL,request_sha256 TEXT NOT NULL,previous_revision INTEGER NOT NULL,committed_revision INTEGER NOT NULL,result_json TEXT NOT NULL,result_sha256 TEXT NOT NULL)",
    ),
    (
        "imports",
        "CREATE TABLE imports(project_id TEXT PRIMARY KEY NOT NULL,source_sha256 TEXT NOT NULL,original_json BLOB NOT NULL)",
    ),
    (
        "assets",
        "CREATE TABLE assets(id TEXT PRIMARY KEY NOT NULL,payload BLOB NOT NULL)",
    ),
    (
        "native_takes",
        "CREATE TABLE native_takes(take_id TEXT PRIMARY KEY NOT NULL,manifest_sha256 TEXT NOT NULL,object_sha256 TEXT NOT NULL,byte_length INTEGER NOT NULL,manifest_json TEXT NOT NULL)",
    ),
    (
        "take_chunks",
        "CREATE TABLE take_chunks(object_sha256 TEXT NOT NULL,sequence INTEGER NOT NULL,byte_length INTEGER NOT NULL,sha256 TEXT NOT NULL,payload BLOB NOT NULL,PRIMARY KEY(object_sha256,sequence))",
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveReceipt {
    pub path: PathBuf,
    pub project_id: String,
    pub sha256: String,
    pub byte_length: u64,
    pub history_rows: u64,
    pub operation_rows: u64,
    pub asset_count: u64,
    pub take_count: u64,
}

pub(super) fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::Invalid(message.into())
}
pub(super) fn ensure(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}
pub(super) fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn unsigned(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| invalid(format!("Negative archive {label}")))
}
pub(super) fn add(value: &mut u64, amount: u64, maximum: u64, label: &str) -> Result<()> {
    *value = value
        .checked_add(amount)
        .filter(|v| *v <= maximum)
        .ok_or_else(|| invalid(format!("Archive {label} exceeds its explicit budget")))?;
    Ok(())
}
pub(super) fn count(connection: &Connection, table: &str) -> Result<u64> {
    // Call sites supply only this module's static table names.
    unsigned(
        connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?,
        "row count",
    )
}
pub(super) fn readonly(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; BEGIN DEFERRED;")?;
    Ok(connection)
}
pub(super) fn private_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    let path = parent.join(format!(
        ".omniusong-{prefix}-{}.partial",
        uuid::Uuid::new_v4()
    ));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&path)?;
    Ok(path)
}
pub(super) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub(super) fn hash_file(path: &Path, maximum: u64) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut length = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        add(&mut length, read as u64, maximum, "file bytes")?;
        hash.update(&buffer[..read]);
    }
    Ok((length, format!("{:x}", hash.finalize())))
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(super) struct Totals {
    pub(super) metadata: u64,
    pub(super) assets: u64,
    pub(super) takes: u64,
    pub(super) edits: u64,
    pub(super) operations: u64,
    pub(super) imports: u64,
    pub(super) asset_count: u64,
    pub(super) take_count: u64,
    pub(super) chunks: u64,
}
pub(super) struct Bundle {
    pub(super) project: ProjectDocument,
    pub(super) assets: BTreeSet<String>,
    pub(super) takes: BTreeSet<TakeReference>,
    pub(super) totals: Totals,
    pub(super) index: Sha256,
}
impl Bundle {
    pub(super) fn record(&mut self, tag: &[u8], fields: &[&[u8]]) {
        self.index.update((tag.len() as u64).to_le_bytes());
        self.index.update(tag);
        self.index.update((fields.len() as u64).to_le_bytes());
        for field in fields {
            self.index.update((field.len() as u64).to_le_bytes());
            self.index.update(field);
        }
    }
    pub(super) fn metadata(&mut self, length: usize) -> Result<()> {
        add(
            &mut self.totals.metadata,
            length as u64,
            ARCHIVE_METADATA_BYTES,
            "metadata",
        )
    }
    pub(super) fn references(&mut self, document: &ProjectDocument) -> Result<()> {
        self.assets.extend(document.referenced_assets()?);
        self.takes.extend(document.referenced_takes()?);
        ensure(
            self.assets.len() <= MAX_OBJECTS && self.takes.len() <= MAX_OBJECTS * 64,
            "Archive reference closure exceeds its object budget",
        )
    }
    fn payload_budget(&self) -> Result<()> {
        ensure(
            self.totals
                .assets
                .checked_add(self.totals.takes)
                .is_some_and(|n| n <= ARCHIVE_PAYLOAD_BYTES),
            "Archive total payload exceeds its budget",
        )
    }
    pub(super) fn index_digest(&self) -> String {
        format!("{:x}", self.index.clone().finalize())
    }
}
#[derive(Clone, Copy)]
pub(super) enum Destination<'a> {
    Archive(&'a Connection),
    Store(&'a Connection),
}
impl Destination<'_> {
    fn connection(&self) -> &Connection {
        match self {
            Self::Archive(c) | Self::Store(c) => c,
        }
    }
}

// Verify only the declared note postimage. A receipt alone cannot establish
// a historical preimage, prior loop length, or whether a schema upgrade was needed.
fn validate_note_postimage(intent: &ProjectEditIntent, result: &ProjectDocument) -> Result<()> {
    if !matches!(
        &intent.edit,
        EditOperation::AddNote { .. }
            | EditOperation::UpdateNote { .. }
            | EditOperation::DeleteNote
            | EditOperation::RestoreNote { .. }
    ) {
        return Ok(());
    }
    let EditScope::DocumentNote { address } = &intent.scope else {
        return Err(invalid("Archived note operation has the wrong scope"));
    };
    let track = result
        .tracks()
        .iter()
        .find(|track| track["id"].as_str() == Some(address.track_id.as_str()))
        .ok_or_else(|| invalid("Archived note operation lost its track"))?;
    ensure(
        track["kind"] == "notes" && track["locked"] == false,
        "Archived note operation requires an unlocked note track",
    )?;
    let notes = track["notes"]
        .as_array()
        .ok_or_else(|| invalid("Archived note operation has invalid notes"))?;
    let note = notes
        .iter()
        .find(|note| note["id"].as_str() == Some(address.note_id.as_str()));
    if matches!(&intent.edit, EditOperation::DeleteNote) {
        ensure(
            note.is_none(),
            "Archived delete-note result still contains the target",
        )?;
        return Ok(());
    }
    let note = note.ok_or_else(|| invalid("Archived note operation lost its target"))?;
    let values_match = |values: &NoteValues| {
        note["pitch"].as_u64() == Some(u64::from(values.pitch))
            && note["startTick"].as_u64() == Some(values.start_tick)
            && note["durationTicks"].as_u64() == Some(values.duration_ticks)
            && note["velocity16"].as_u64() == Some(u64::from(values.velocity16))
    };
    let policy = match &intent.edit {
        EditOperation::AddNote { values, timeline } => {
            let expected = serde_json::json!({"id":address.note_id,"pitch":values.pitch,"startTick":values.start_tick,"durationTicks":values.duration_ticks,"velocity16":values.velocity16,"expression":[]});
            ensure(
                note == &expected,
                "Archived add-note result is not the exact authored note",
            )?;
            *timeline
        }
        EditOperation::UpdateNote { values, timeline } => {
            ensure(
                values_match(values),
                "Archived update-note values differ from the request",
            )?;
            *timeline
        }
        EditOperation::RestoreNote { timeline } => {
            let source = note.get("editSource").ok_or_else(|| {
                invalid("Archived restore-note result lost its original snapshot")
            })?;
            ensure(
                ["pitch", "startTick", "durationTicks", "velocity16"]
                    .iter()
                    .all(|field| note.get(*field) == source.get(*field))
                    && note.get("expression") == source.get("expression"),
                "Archived restore-note fields/expression differ from the retained original",
            )?;
            *timeline
        }
        _ => return Err(invalid("Invalid note postimage dispatch")),
    };
    if matches!(policy, NoteTimelinePolicy::ExtendLoop { .. }) {
        let end = note["startTick"]
            .as_u64()
            .and_then(|start| {
                note["durationTicks"]
                    .as_u64()
                    .and_then(|duration| start.checked_add(duration))
            })
            .ok_or_else(|| invalid("Archived note end tick overflow"))?;
        let loop_end = result
            .bars()
            .checked_mul(PPQ)
            .and_then(|ticks| ticks.checked_mul(4))
            .ok_or_else(|| invalid("Archived loop tick overflow"))?;
        ensure(
            loop_end >= end,
            "Archived extended loop does not cover the note",
        )?;
    }
    Ok(())
}

/// Only explicit authored postconditions are provable from this result alone.
/// Keep fields, source originality and the complete preimage require history;
/// never infer them from a valid digest or a claimed editSource snapshot.
fn validate_clip_postimage(intent: &ProjectEditIntent, result: &ProjectDocument) -> Result<()> {
    let policy = match &intent.edit {
        EditOperation::UpdateClip { timeline, .. } | EditOperation::RestoreClip { timeline } => {
            *timeline
        }
        _ => return Ok(()),
    };
    let EditScope::DocumentClip { address } = &intent.scope else {
        return Err(invalid("Archived clip operation has the wrong scope"));
    };
    let (track_id, kind) = match address {
        DocumentClipAddress::LegacyAudio { track_id } => (track_id, "audio"),
        DocumentClipAddress::NativeAudio { track_id, .. } => (track_id, "recording"),
    };
    let track = result
        .tracks()
        .iter()
        .find(|track| track["id"].as_str() == Some(track_id.as_str()))
        .ok_or_else(|| invalid("Archived clip operation lost its track"))?;
    ensure(
        track["kind"] == kind && track["locked"] == false,
        "Archived clip operation requires an unlocked track of the declared kind",
    )?;
    let clip = match address {
        DocumentClipAddress::LegacyAudio { .. } => &track["audio"],
        DocumentClipAddress::NativeAudio { clip_id, .. } => {
            let clips = track["recording"]["clips"]
                .as_array()
                .ok_or_else(|| invalid("Archived clip operation lost its payload"))?;
            let clip = clips
                .iter()
                .find(|clip| clip["id"].as_str() == Some(clip_id.as_str()))
                .ok_or_else(|| invalid("Archived clip operation lost its target"))?;
            ensure(
                clip["range"]["unit"] == "audio_frames",
                "Archived audio edit targets a non-audio source range",
            )?;
            clip
        }
    };
    let field_matches = |field: &str, change: &FieldChange<serde_json::Value>| -> Result<()> {
        let matches = match change {
            FieldChange::Keep => true,
            FieldChange::Set { value } => clip.get(field) == Some(value),
            FieldChange::Remove => clip.get(field).is_none(),
        };
        ensure(
            matches,
            "Archived clip field differs from the explicit request",
        )
    };
    let as_value = |change: &FieldChange<serde_json::Number>| match change {
        FieldChange::Keep => FieldChange::Keep,
        FieldChange::Set { value } => FieldChange::Set {
            value: serde_json::Value::Number(value.clone()),
        },
        FieldChange::Remove => FieldChange::Remove,
    };
    match &intent.edit {
        EditOperation::UpdateClip {
            patch: ClipPatch::LegacyAudio(patch),
            ..
        } => {
            let start = match patch.start_tick {
                FieldChange::Keep => FieldChange::Keep,
                FieldChange::Set { value } => FieldChange::Set {
                    value: value.into(),
                },
                FieldChange::Remove => FieldChange::Remove,
            };
            field_matches("startTick", &start)?;
            for (field, change) in [
                ("trimStartSeconds", &patch.trim_start_seconds),
                ("trimEndSeconds", &patch.trim_end_seconds),
                ("fadeInSeconds", &patch.fade_in_seconds),
                ("fadeOutSeconds", &patch.fade_out_seconds),
            ] {
                field_matches(field, &as_value(change))?;
            }
        }
        EditOperation::UpdateClip {
            patch: ClipPatch::NativeAudio(patch),
            ..
        } => {
            if let ValueChange::Set { value } = patch.position_frame {
                ensure(
                    clip["position"]["frame"] == value,
                    "Archived clip placement differs from the explicit request",
                )?;
            }
            if let ValueChange::Set { value } = &patch.source_window {
                let expected = serde_json::json!({"unit":"audio_frames","start":value.start_frame,"count":value.frame_count});
                ensure(
                    clip["range"] == expected,
                    "Archived clip source window differs from the explicit request",
                )?;
            }
            let fades = match &patch.fades {
                FieldChange::Keep => FieldChange::Keep,
                FieldChange::Set { value } => FieldChange::Set {
                    value: serde_json::to_value(value)
                        .map_err(|error| invalid(error.to_string()))?,
                },
                FieldChange::Remove => FieldChange::Remove,
            };
            field_matches("fades", &fades)?;
        }
        EditOperation::RestoreClip { .. } => {
            let original = clip
                .get("editSource")
                .and_then(|source| source.get("original"))
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| {
                    invalid("Archived restore-clip result lost its original snapshot")
                })?;
            let fields: &[&str] = match address {
                DocumentClipAddress::LegacyAudio { .. } => &[
                    "startTick",
                    "trimStartSeconds",
                    "trimEndSeconds",
                    "fadeInSeconds",
                    "fadeOutSeconds",
                ],
                DocumentClipAddress::NativeAudio { .. } => &["range", "position", "fades"],
            };
            ensure(
                fields
                    .iter()
                    .all(|field| clip.get(*field) == original.get(*field)),
                "Archived restored clip settings differ from the retained original",
            )?;
        }
        _ => return Err(invalid("Invalid clip postimage dispatch")),
    }
    if matches!(policy, ClipTimelinePolicy::ExtendLoop { .. }) {
        ensure(
            result.bars() >= result.clip_required_bars(address)?,
            "Archived extended loop does not cover the clip extent",
        )?;
    }
    Ok(())
}

pub(super) fn validate_request(
    request: &str,
    operation_id: &str,
    project_id: &str,
    previous: u64,
    result: &ProjectDocument,
) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_str(request).map_err(|e| invalid(e.to_string()))?;
    if value.get("operation_id").is_none() {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("Invalid operation wrapper"))?;
        ensure(
            object.len() == 3 && value["kind"] == "take_publication" && value["version"] == 1,
            "Unknown archived operation wrapper",
        )?;
        let intent: TakePublication =
            serde_json::from_value(value["intent"].clone()).map_err(|e| invalid(e.to_string()))?;
        intent.validate()?;
        ensure(
            publication_request(&intent)? == request,
            "Noncanonical archived take operation",
        )?;
        ensure(
            intent.operation_id.0 == operation_id
                && intent.project_id.0 == project_id
                && intent.expected_revision == previous,
            "Archived take operation identity/revision mismatch",
        )?;
        ensure(
            result.take_clips()?.contains(&intent.clip),
            "Take operation result lost its published clip",
        )?;
    } else {
        let intent: ProjectEditIntent =
            serde_json::from_str(request).map_err(|e| invalid(e.to_string()))?;
        intent.validate().map_err(|e| invalid(e.to_string()))?;
        ensure(
            serde_json::to_string(&intent).map_err(|e| invalid(e.to_string()))? == request,
            "Noncanonical archived edit operation",
        )?;
        ensure(
            intent.operation_id.0 == operation_id
                && intent.project_id.0 == project_id
                && intent.expected_revision == previous,
            "Archived edit operation identity/revision mismatch",
        )?;
        ensure(
            matches!(
                intent.edit,
                EditOperation::Rename { .. }
                    | EditOperation::SetTempo { .. }
                    | EditOperation::UpdateSession { .. }
                    | EditOperation::AddNote { .. }
                    | EditOperation::UpdateNote { .. }
                    | EditOperation::DeleteNote
                    | EditOperation::RestoreNote { .. }
                    | EditOperation::UpdateClip { .. }
                    | EditOperation::RestoreClip { .. }
                    | EditOperation::Undo
                    | EditOperation::Redo
            ),
            "Unsupported operation in archive",
        )?;
        validate_note_postimage(&intent, result)?;
        validate_clip_postimage(&intent, result)?;
    }
    Ok(())
}

/// Visit bounded metadata one row at a time; never collect the history/results.
pub(super) fn copy_metadata(
    source: &Connection,
    destination: Option<Destination<'_>>,
    project_id: &str,
) -> Result<Bundle> {
    let project = load_document(source, project_id)?;
    let (payload, revision, cursor): (String, i64, i64) = source.query_row(
        "SELECT CASE WHEN octet_length(payload)<=?2 THEN payload END,revision,cursor FROM projects WHERE id=?1",
        params![project_id, MAX_NATIVE_PROJECT_BYTES as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let cursor = unsigned(cursor, "cursor")?;
    let mut bundle = Bundle {
        project: project.clone(),
        assets: BTreeSet::new(),
        takes: BTreeSet::new(),
        totals: Totals::default(),
        index: Sha256::new(),
    };
    bundle.index.update(b"OmniuSong.project-archive.v1\0");
    bundle.metadata(payload.len() + project_id.len() + 16)?;
    bundle.references(&project)?;
    bundle.record(
        b"project",
        &[
            project_id.as_bytes(),
            payload.as_bytes(),
            &revision.to_le_bytes(),
            &cursor.to_le_bytes(),
        ],
    );
    if let Some(target) = destination {
        match target {
            Destination::Archive(c) => {
                c.execute(
                    "INSERT INTO projects VALUES(?1,?2,?3,?4,?5)",
                    params![
                        project_id,
                        payload,
                        revision,
                        cursor as i64,
                        checksum(payload.as_bytes())
                    ],
                )?;
            }
            Destination::Store(c) => {
                c.execute(
                    "INSERT INTO projects(id,payload,revision,cursor) VALUES(?1,?2,?3,?4)",
                    params![project_id, payload, revision, cursor as i64],
                )?;
            }
        }
    }
    let mut statement = source.prepare("SELECT sequence,CASE WHEN octet_length(before_json)<=?2 THEN before_json END,CASE WHEN octet_length(after_json)<=?2 THEN after_json END FROM edits WHERE project_id=?1 ORDER BY sequence")?;
    let mut rows = statement.query(params![project_id, MAX_NATIVE_PROJECT_BYTES as i64])?;
    while let Some(row) = rows.next()? {
        add(&mut bundle.totals.edits, 1, MAX_EDITS, "history row count")?;
        let sequence = unsigned(row.get(0)?, "history sequence")?;
        ensure(
            sequence == bundle.totals.edits,
            "Archive history sequences are not contiguous",
        )?;
        let before: String = row.get(1)?;
        let after: String = row.get(2)?;
        bundle.metadata(before.len() + after.len() + project_id.len() + 8)?;
        let before_doc = ProjectDocument::from_json(before.as_bytes())?;
        let after_doc = ProjectDocument::from_json(after.as_bytes())?;
        ensure(
            before_doc.id() == project_id
                && after_doc.id() == project_id
                && before_doc.revision().checked_add(1) == Some(after_doc.revision())
                && after_doc.revision() <= project.revision(),
            "Archive history identity/revision mismatch",
        )?;
        bundle.references(&before_doc)?;
        bundle.references(&after_doc)?;
        bundle.record(
            b"edit",
            &[&sequence.to_le_bytes(), before.as_bytes(), after.as_bytes()],
        );
        if let Some(target) = destination {
            match target {
                Destination::Archive(c) => {
                    c.execute(
                        "INSERT INTO edits VALUES(?1,?2,?3,?4,?5,?6)",
                        params![
                            project_id,
                            sequence as i64,
                            before,
                            after,
                            checksum(before.as_bytes()),
                            checksum(after.as_bytes())
                        ],
                    )?;
                }
                Destination::Store(c) => {
                    c.execute("INSERT INTO edits(project_id,sequence,before_json,after_json) VALUES(?1,?2,?3,?4)", params![project_id,sequence as i64,before,after])?;
                }
            }
        }
    }
    ensure(
        cursor <= bundle.totals.edits,
        "Archive cursor exceeds retained history",
    )?;
    let mut statement = source.prepare("SELECT CASE WHEN octet_length(operation_id)<=96 THEN operation_id END,CASE WHEN octet_length(request_json)<=?2 THEN request_json END,CASE WHEN octet_length(request_sha256)=64 THEN request_sha256 END,previous_revision,committed_revision,CASE WHEN octet_length(result_json)<=?3 THEN result_json END,CASE WHEN octet_length(result_sha256)=64 THEN result_sha256 END FROM operations WHERE project_id=?1 ORDER BY operation_id")?;
    let mut rows = statement.query(params![
        project_id,
        WORKER_MAX_BYTES as i64,
        MAX_NATIVE_PROJECT_BYTES as i64
    ])?;
    while let Some(row) = rows.next()? {
        add(
            &mut bundle.totals.operations,
            1,
            MAX_OPERATIONS,
            "operation row count",
        )?;
        let id: String = row.get(0)?;
        let request: String = row.get(1)?;
        let request_sha: String = row.get(2)?;
        let previous = unsigned(row.get(3)?, "previous revision")?;
        let committed = unsigned(row.get(4)?, "committed revision")?;
        let result: String = row.get(5)?;
        let result_sha: String = row.get(6)?;
        bundle.metadata(request.len() + result.len() + id.len() + project_id.len() + 144)?;
        ensure(
            checksum(request.as_bytes()) == request_sha
                && checksum(result.as_bytes()) == result_sha,
            "Archived operation checksum mismatch",
        )?;
        let document = ProjectDocument::from_json(result.as_bytes())?;
        ensure(
            document.id() == project_id
                && document.revision() == committed
                && previous.checked_add(1) == Some(committed)
                && committed <= project.revision(),
            "Archived operation result identity/revision mismatch",
        )?;
        validate_request(&request, &id, project_id, previous, &document)?;
        bundle.references(&document)?;
        bundle.record(
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
        if let Some(target) = destination {
            if matches!(target, Destination::Store(_)) {
                super::source_attachment::reject_reserved_operation(&id)?;
            }
            target.connection().execute(
                "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    id,
                    project_id,
                    request,
                    request_sha,
                    previous as i64,
                    committed as i64,
                    result,
                    result_sha
                ],
            )?;
        }
    }
    let import: Option<(String, Vec<u8>)> = source.query_row(
        "SELECT CASE WHEN octet_length(source_sha256)=64 THEN source_sha256 END,CASE WHEN octet_length(original_json)<=?2 THEN original_json END FROM imports WHERE project_id=?1",
        params![project_id,MAX_PROJECT_BYTES as i64], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    if let Some((sha, bytes)) = import {
        ensure(
            checksum(&bytes) == sha,
            "Archived original import checksum mismatch",
        )?;
        let original = ProjectDocument::from_json(&bytes)?;
        ensure(
            original.schema_version() == 1,
            "Original legacy import must use musical schema 1",
        )?;
        bundle.metadata(bytes.len() + sha.len() + project_id.len())?;
        bundle.references(&original)?;
        bundle.totals.imports = 1;
        bundle.record(b"import", &[sha.as_bytes(), &bytes]);
        if let Some(target) = destination {
            target.connection().execute(
                "INSERT INTO imports VALUES(?1,?2,?3)",
                params![project_id, sha, bytes],
            )?;
        }
    }
    Ok(bundle)
}

fn take_metadata(source: &Connection, take_id: &str) -> Result<(TakeObject, String)> {
    let object = load_take(source, &TakeId(take_id.into()))?;
    let json: String = source.query_row("SELECT CASE WHEN octet_length(manifest_json)<=?2 THEN manifest_json END FROM native_takes WHERE take_id=?1", params![take_id,MAX_MANIFEST_BYTES as i64], |row| row.get(0))?;
    ensure(
        digest_valid(&object.object_sha256)
            && object.byte_length > 0
            && object.byte_length <= ARCHIVE_PAYLOAD_BYTES,
        "Invalid archive take object identity/size",
    )?;
    Ok((object, json))
}
pub(super) fn take_ids(bundle: &Bundle) -> Result<BTreeMap<String, String>> {
    let mut identities = BTreeMap::new();
    for reference in &bundle.takes {
        if let Some(old) = identities.insert(
            reference.take_id.0.clone(),
            reference.manifest_sha256.clone(),
        ) {
            ensure(
                old == reference.manifest_sha256,
                "A take identity refers to conflicting sealed manifests",
            )?;
        }
    }
    ensure(
        identities.len() <= MAX_OBJECTS,
        "Too many native takes in archive",
    )?;
    Ok(identities)
}
fn register_take(connection: &Connection, object: &TakeObject, manifest: &str) -> Result<()> {
    connection.execute("INSERT INTO native_takes(take_id,manifest_sha256,object_sha256,byte_length,manifest_json) VALUES(?1,?2,?3,?4,?5)",
        params![object.take_id.0,object.manifest_sha256,object.object_sha256,object.byte_length as i64,manifest])?;
    Ok(())
}
fn account_take(bundle: &mut Bundle, object: &TakeObject, manifest: &str) -> Result<()> {
    bundle.metadata(manifest.len() + object.take_id.0.len() + 136)?;
    add(
        &mut bundle.totals.take_count,
        1,
        MAX_OBJECTS as u64,
        "take count",
    )?;
    add(
        &mut bundle.totals.takes,
        object.byte_length,
        ARCHIVE_PAYLOAD_BYTES,
        "take bytes",
    )?;
    bundle.payload_budget()?;
    bundle.record(
        b"take",
        &[
            object.take_id.0.as_bytes(),
            object.manifest_sha256.as_bytes(),
            object.object_sha256.as_bytes(),
            &object.byte_length.to_le_bytes(),
            manifest.as_bytes(),
        ],
    );
    Ok(())
}
fn account_asset(bundle: &mut Bundle, id: &str, payload: &[u8]) -> Result<()> {
    bundle.metadata(id.len() + 72)?;
    add(
        &mut bundle.totals.asset_count,
        1,
        MAX_OBJECTS as u64,
        "asset count",
    )?;
    add(
        &mut bundle.totals.assets,
        payload.len() as u64,
        ARCHIVE_PAYLOAD_BYTES,
        "asset bytes",
    )?;
    bundle.payload_budget()?;
    bundle.record(
        b"asset",
        &[
            id.as_bytes(),
            &(payload.len() as u64).to_le_bytes(),
            checksum(payload).as_bytes(),
        ],
    );
    Ok(())
}

fn export_objects(
    source: &Connection,
    archive: &Connection,
    root: &Path,
    bundle: &mut Bundle,
) -> Result<()> {
    for id in bundle.assets.clone() {
        let payload = read_asset(source, &id)?;
        account_asset(bundle, &id, &payload)?;
        archive.execute("INSERT INTO assets VALUES(?1,?2)", params![id, payload])?;
    }
    let mut object_ids = BTreeSet::new();
    for (id, expected_manifest) in take_ids(bundle)? {
        let (object, json) = take_metadata(source, &id)?;
        ensure(
            object.manifest_sha256 == expected_manifest,
            "Reference differs from registered manifest",
        )?;
        ensure(
            object_ids.insert(object.object_sha256.clone()),
            "Duplicate native object identity",
        )?;
        account_take(bundle, &object, &json)?;
        let verified = verify_owned_take(root, &object)?;
        let mut file = File::open(verified.path())?;
        let mut buffer = vec![0; ARCHIVE_CHUNK_BYTES];
        let mut length = 0;
        let mut hash = Sha256::new();
        let mut sequence = 0_i64;
        loop {
            let mut filled = 0;
            while filled < buffer.len() {
                let n = file.read(&mut buffer[filled..])?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            add(
                &mut length,
                filled as u64,
                object.byte_length,
                "object length",
            )?;
            hash.update(&buffer[..filled]);
            archive.execute(
                "INSERT INTO take_chunks VALUES(?1,?2,?3,?4,?5)",
                params![
                    object.object_sha256,
                    sequence,
                    filled as i64,
                    checksum(&buffer[..filled]),
                    &buffer[..filled]
                ],
            )?;
            add(
                &mut bundle.totals.chunks,
                1,
                ARCHIVE_PAYLOAD_BYTES / ARCHIVE_CHUNK_BYTES as u64 + MAX_OBJECTS as u64,
                "chunk count",
            )?;
            bundle.metadata(144)?;
            sequence += 1;
        }
        ensure(
            length == object.byte_length
                && format!("{:x}", hash.finalize()) == object.object_sha256,
            "Owned take file changed or differs from its registered digest",
        )?;
        register_take(archive, &object, &json)?;
    }
    Ok(())
}

fn insert_meta(connection: &Connection, bundle: &Bundle) -> Result<()> {
    let t = &bundle.totals;
    connection.execute(
        "INSERT INTO archive_meta VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            bundle.project.id(),
            t.metadata as i64,
            t.assets as i64,
            t.takes as i64,
            t.edits as i64,
            t.operations as i64,
            t.imports as i64,
            t.asset_count as i64,
            t.take_count as i64,
            t.chunks as i64,
            bundle.index_digest()
        ],
    )?;
    Ok(())
}

impl NativeStore {
    /// Export a complete retained project closure without overwriting a name.
    pub fn export_archive(&self, project_id: &str, destination: &Path) -> Result<ArchiveReceipt> {
        let _writer = crate::retained_file_coordination::writer_with_file(&self.root, destination)?;
        let mut staging: Option<PathBuf> = None;
        let mut published = false;
        let outcome = (|| {
            let parent = fs::canonicalize(
                destination
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new(".")),
            )?;
            let destination = parent.join(
                destination
                    .file_name()
                    .ok_or_else(|| invalid("Archive destination requires a file name"))?,
            );
            ensure(
                !destination.try_exists()? && fs::symlink_metadata(&destination).is_err(),
                "Archive destination already exists",
            )?;
            let stage = private_directory(&parent, "archive-export")?;
            staging = Some(stage.clone());
            let path = stage.join("archive.sqlite3");
            let reservation = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)?;
            let archive = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            archive.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; BEGIN IMMEDIATE;")?;
            archive.pragma_update(None, "application_id", ARCHIVE_APPLICATION_ID)?;
            archive.pragma_update(None, "user_version", ARCHIVE_VERSION)?;
            for (_, sql) in TABLES
                .iter()
                .chain(crate::archive_copy::LINEAGE_TABLES.iter())
            {
                archive.execute_batch(sql)?;
            }
            let source = readonly(&self.root.join("studio.sqlite3"))?;
            let mut bundle =
                copy_metadata(&source, Some(Destination::Archive(&archive)), project_id)?;
            crate::archive_copy::read_lineage(
                &source,
                Some((&archive, project_id, true)),
                project_id,
                &mut bundle,
                false,
            )?;
            export_objects(&source, &archive, &self.root, &mut bundle)?;
            // Check every declared native source/range before publishing export.
            validate_all_reference_snapshots(&archive, project_id)?;
            insert_meta(&archive, &bundle)?;
            archive.execute_batch("COMMIT")?;
            archive.close().map_err(|(_, e)| StoreError::Database(e))?;
            reservation.sync_all()?;
            drop(reservation);
            let (byte_length, sha256) = hash_file(&path, MAX_ARCHIVE_FILE_BYTES)?;
            fs::hard_link(&path, &destination)?;
            published = true;
            sync_directory(&parent)?;
            fs::remove_file(&path)?;
            fs::remove_dir(&stage)?;
            staging = None;
            Ok(ArchiveReceipt {
                path: destination,
                project_id: project_id.into(),
                sha256,
                byte_length,
                history_rows: bundle.totals.edits,
                operation_rows: bundle.totals.operations,
                asset_count: bundle.totals.asset_count,
                take_count: bundle.totals.take_count,
            })
        })();
        outcome.map_err(|error: StoreError| invalid(format!("Archive export failed; staging={staging:?}; destination={}; published={published}: {error}",destination.display())))
    }
}

pub(super) fn validate_all_reference_snapshots(
    connection: &Connection,
    project_id: &str,
) -> Result<()> {
    crate::check_references(connection, &load_document(connection, project_id)?)?;
    for sql in [
        "SELECT CASE WHEN octet_length(before_json)<=?2 THEN before_json END FROM edits WHERE project_id=?1",
        "SELECT CASE WHEN octet_length(after_json)<=?2 THEN after_json END FROM edits WHERE project_id=?1",
        "SELECT CASE WHEN octet_length(result_json)<=?2 THEN result_json END FROM operations WHERE project_id=?1",
    ] {
        let mut statement = connection.prepare(sql)?;
        let mut rows = statement.query(params![project_id, MAX_NATIVE_PROJECT_BYTES as i64])?;
        while let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            crate::check_references(connection, &ProjectDocument::from_json(json.as_bytes())?)?;
        }
    }
    crate::archive_copy::check_lineage_references(connection, project_id)?;
    Ok(())
}

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    let application_id: u32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure(
        application_id == ARCHIVE_APPLICATION_ID && (version == 1 || version == ARCHIVE_VERSION),
        "Unknown native archive application or format version",
    )?;
    let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    ensure(
        mode.eq_ignore_ascii_case("delete"),
        "Archive must be a standalone rollback-journal SQLite file",
    )?;
    let tables: Vec<_> = TABLES
        .iter()
        .chain(if version == 2 {
            crate::archive_copy::LINEAGE_TABLES.iter()
        } else {
            [].iter()
        })
        .copied()
        .collect();
    let objects: u64 = unsigned(
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?,
        "schema object count",
    )?;
    ensure(
        objects <= 3 * tables.len() as u64,
        "Unexpected archive schema objects",
    )?;
    let expected: BTreeMap<_, _> = tables.iter().copied().collect();
    let mut seen = BTreeSet::new();
    let mut statement=connection.prepare("SELECT CASE WHEN octet_length(type)<=16 THEN type END,CASE WHEN octet_length(name)<=128 THEN name END,CASE WHEN octet_length(tbl_name)<=128 THEN tbl_name END,CASE WHEN octet_length(sql)<=8192 THEN sql END FROM sqlite_schema ORDER BY name")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let name: String = row.get(1)?;
        let table: String = row.get(2)?;
        let sql: Option<String> = row.get(3)?;
        if kind == "index"
            && name.starts_with("sqlite_autoindex_")
            && sql.is_none()
            && expected.contains_key(table.as_str())
        {
            continue;
        }
        ensure(
            kind == "table"
                && name == table
                && expected
                    .get(name.as_str())
                    .is_some_and(|expected| sql.as_deref() == Some(*expected)),
            "Archive table definition is not the supported format",
        )?;
        seen.insert(name);
    }
    ensure(seen.len() == tables.len(), "Archive tables are missing")?;
    let check: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    ensure(check == "ok", "SQLite archive integrity check failed")
}

pub(super) struct ArchiveMeta {
    pub(super) project_id: String,
    pub(super) totals: Totals,
    pub(super) index: String,
}
pub(super) fn load_meta(connection: &Connection) -> Result<ArchiveMeta> {
    ensure(
        count(connection, "archive_meta")? == 1,
        "Archive requires exactly one metadata row",
    )?;
    let (project_id,counters,index):(String,[i64;9],String)=connection.query_row(
        "SELECT CASE WHEN octet_length(project_id)<=80 THEN project_id END,metadata_bytes,asset_bytes,take_bytes,edit_count,operation_count,import_count,asset_count,take_count,chunk_count,CASE WHEN octet_length(index_sha256)=64 THEN index_sha256 END FROM archive_meta WHERE id=1",[],
        |row|Ok((row.get(0)?,[row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?],row.get(10)?)))?;
    let totals = Totals {
        metadata: unsigned(counters[0], "metadata bytes")?,
        assets: unsigned(counters[1], "asset bytes")?,
        takes: unsigned(counters[2], "take bytes")?,
        edits: unsigned(counters[3], "edits")?,
        operations: unsigned(counters[4], "operations")?,
        imports: unsigned(counters[5], "imports")?,
        asset_count: unsigned(counters[6], "assets")?,
        take_count: unsigned(counters[7], "takes")?,
        chunks: unsigned(counters[8], "chunks")?,
    };
    ensure(
        totals.metadata <= ARCHIVE_METADATA_BYTES
            && totals.edits <= MAX_EDITS
            && totals.operations <= MAX_OPERATIONS
            && totals.imports <= 1
            && totals.asset_count <= MAX_OBJECTS as u64
            && totals.take_count <= MAX_OBJECTS as u64
            && totals
                .assets
                .checked_add(totals.takes)
                .is_some_and(|n| n <= ARCHIVE_PAYLOAD_BYTES)
            && totals.chunks
                <= ARCHIVE_PAYLOAD_BYTES / ARCHIVE_CHUNK_BYTES as u64 + MAX_OBJECTS as u64
            && digest_valid(&index),
        "Archive metadata advertises unsupported limits",
    )?;
    for (table, expected) in [
        ("projects", 1),
        ("edits", totals.edits),
        ("operations", totals.operations),
        ("imports", totals.imports),
        ("assets", totals.asset_count),
        ("native_takes", totals.take_count),
        ("take_chunks", totals.chunks),
    ] {
        ensure(
            count(connection, table)? == expected,
            "Archive row count differs from its metadata",
        )?;
    }
    Ok(ArchiveMeta {
        project_id,
        totals,
        index,
    })
}

pub(super) fn validate_snapshot_hashes(connection: &Connection, project_id: &str) -> Result<()> {
    for sql in [
        "SELECT CASE WHEN octet_length(payload)<=?2 THEN payload END,CASE WHEN octet_length(payload_sha256)=64 THEN payload_sha256 END FROM projects WHERE id=?1",
        "SELECT CASE WHEN octet_length(before_json)<=?2 THEN before_json END,CASE WHEN octet_length(before_sha256)=64 THEN before_sha256 END FROM edits WHERE project_id=?1",
        "SELECT CASE WHEN octet_length(after_json)<=?2 THEN after_json END,CASE WHEN octet_length(after_sha256)=64 THEN after_sha256 END FROM edits WHERE project_id=?1",
    ] {
        let mut statement = connection.prepare(sql)?;
        let mut rows = statement.query(params![project_id, MAX_NATIVE_PROJECT_BYTES as i64])?;
        while let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            let expected: String = row.get(1)?;
            ensure(
                checksum(payload.as_bytes()) == expected,
                "Archive snapshot checksum mismatch",
            )?;
        }
    }
    Ok(())
}

pub(super) fn restore_object_files(
    archive: &Connection,
    stage: &Path,
    bundle: &mut Bundle,
) -> Result<()> {
    fs::create_dir(stage.join("takes"))?;
    for id in bundle.assets.clone() {
        let payload = read_asset(archive, &id)?;
        account_asset(bundle, &id, &payload)?;
    }
    let mut object_ids = BTreeSet::new();
    for (id, expected_manifest) in take_ids(bundle)? {
        let (object, json) = take_metadata(archive, &id)?;
        ensure(
            object.manifest_sha256 == expected_manifest,
            "Reference differs from archived manifest",
        )?;
        ensure(
            object_ids.insert(object.object_sha256.clone()),
            "Duplicate archived object identity",
        )?;
        account_take(bundle, &object, &json)?;
        let path = object_path(stage, &object.object_sha256)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut statement=archive.prepare("SELECT sequence,byte_length,CASE WHEN octet_length(sha256)=64 THEN sha256 END,CASE WHEN octet_length(payload)<=?2 THEN payload END FROM take_chunks WHERE object_sha256=?1 ORDER BY sequence")?;
        let mut rows =
            statement.query(params![object.object_sha256, ARCHIVE_CHUNK_BYTES as i64])?;
        let mut sequence = 0;
        let mut length = 0;
        let mut hash = Sha256::new();
        while let Some(row) = rows.next()? {
            let actual = unsigned(row.get(0)?, "chunk sequence")?;
            let announced = unsigned(row.get(1)?, "chunk length")?;
            let sha: String = row.get(2)?;
            let payload: Vec<u8> = row.get(3)?;
            let remaining = object
                .byte_length
                .checked_sub(length)
                .ok_or_else(|| invalid("Archive chunk length overflow"))?;
            let expected = remaining.min(ARCHIVE_CHUNK_BYTES as u64);
            ensure(
                actual == sequence
                    && expected > 0
                    && announced == expected
                    && payload.len() as u64 == expected
                    && checksum(&payload) == sha,
                "Archive chunk sequence, size or checksum mismatch",
            )?;
            add(
                &mut length,
                payload.len() as u64,
                object.byte_length,
                "object bytes",
            )?;
            add(
                &mut bundle.totals.chunks,
                1,
                ARCHIVE_PAYLOAD_BYTES / ARCHIVE_CHUNK_BYTES as u64 + MAX_OBJECTS as u64,
                "chunk count",
            )?;
            bundle.metadata(144)?;
            file.write_all(&payload)?;
            hash.update(&payload);
            sequence += 1;
        }
        ensure(
            length == object.byte_length
                && format!("{:x}", hash.finalize()) == object.object_sha256,
            "Archived native object is missing, truncated or damaged",
        )?;
        file.sync_all()?;
        drop(file);
        let verified = verify_owned_take(stage, &object)?;
        ensure(
            verified.manifest() == &object.manifest,
            "Restored object sealed identity mismatch",
        )?;
    }
    sync_directory(&stage.join("takes"))?;
    Ok(())
}

fn collision_checks(
    destination: &Connection,
    archive: &Connection,
    project_id: &str,
) -> Result<()> {
    let project_exists: bool = destination.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
        [project_id],
        |row| row.get(0),
    )?;
    ensure(
        !project_exists,
        "Identity-preserving restore conflicts with an existing project",
    )?;
    // Corrupt orphan rows are conflicts too; restoration never replaces them.
    for table in ["edits", "imports", "operations"] {
        let exists: bool = destination.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE project_id=?1)"),
            [project_id],
            |row| row.get(0),
        )?;
        ensure(
            !exists,
            "Restore project identity has preexisting retained rows",
        )?;
    }
    let mut statement=archive.prepare("SELECT CASE WHEN octet_length(operation_id)<=96 THEN operation_id END FROM operations ORDER BY operation_id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let exists: bool = destination.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
            [id],
            |row| row.get(0),
        )?;
        ensure(
            !exists,
            "Restore conflicts with a global operation identity",
        )?;
    }
    Ok(())
}

pub(super) fn install_objects(
    destination: &Connection,
    archive: &Connection,
    root: &Path,
    stage: &Path,
    bundle: &Bundle,
    published: &mut usize,
) -> Result<()> {
    let take_directory = root.join("takes");
    match fs::symlink_metadata(&take_directory) {
        Ok(metadata) => ensure(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Native take directory is not an owned regular directory",
        )?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&take_directory)?
        }
        Err(error) => return Err(error.into()),
    }
    for id in &bundle.assets {
        let payload = read_asset(archive, id)?;
        let exists: bool = destination.query_row(
            "SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1)",
            [id],
            |row| row.get(0),
        )?;
        if exists {
            ensure(
                read_asset(destination, id)? == payload,
                "Shared legacy asset differs from archived content",
            )?;
        } else {
            destination.execute(
                "INSERT INTO assets(id,payload) VALUES(?1,?2)",
                params![id, payload],
            )?;
        }
    }
    for (id, _) in take_ids(bundle)? {
        let (object, json) = take_metadata(archive, &id)?;
        let exists: bool = destination.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_takes WHERE take_id=?1)",
            [&id],
            |row| row.get(0),
        )?;
        if exists {
            let (existing, existing_json) = take_metadata(destination, &id)?;
            ensure(
                existing.manifest_sha256 == object.manifest_sha256
                    && existing.object_sha256 == object.object_sha256
                    && existing.byte_length == object.byte_length
                    && existing_json == json,
                "Shared take identity has different metadata",
            )?;
            verify_owned_take(root, &existing)?;
        } else {
            let path = object_path(root, &object.object_sha256)?;
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    verify_owned_take(root, &object)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let staged = object_path(stage, &object.object_sha256)?;
                    fs::hard_link(&staged, &path)?;
                    *published += 1;
                    // The verified object contract requires a single link.
                    // Remove only our staging link; never replace destination.
                    fs::remove_file(&staged)?;
                    File::open(&path)?.sync_all()?;
                    verify_owned_take(root, &object)?;
                }
                Err(error) => return Err(error.into()),
            }
            register_take(destination, &object, &json)?;
        }
    }
    sync_directory(&take_directory)?;
    sync_directory(root)?;
    Ok(())
}

impl NativeStore {
    /// Restore exact identities without replacing projects or operation receipts.
    /// Restore-as-copy uses its separate typed and idempotent admission API.
    pub fn restore_archive(&mut self, archive_path: &Path) -> Result<ProjectDocument> {
        let _writer =
            crate::retained_file_coordination::writer_with_file(&self.root, archive_path)?;
        let mut staging: Option<PathBuf> = None;
        let mut published_objects = 0;
        let mut committed = false;
        let outcome = (|| {
            let path = fs::canonicalize(archive_path)?;
            let metadata = fs::metadata(&path)?;
            ensure(
                metadata.is_file() && metadata.len() <= MAX_ARCHIVE_FILE_BYTES,
                "Archive is not a supported bounded file",
            )?;
            let archive = readonly(&path)?;
            validate_schema(&archive)?;
            let meta = load_meta(&archive)?;
            validate_snapshot_hashes(&archive, &meta.project_id)?;
            let mut bundle = copy_metadata(&archive, None, &meta.project_id)?;
            crate::archive_copy::read_lineage(&archive, None, &meta.project_id, &mut bundle, true)?;
            ensure(
                bundle.totals.edits == meta.totals.edits
                    && bundle.totals.operations == meta.totals.operations
                    && bundle.totals.imports == meta.totals.imports,
                "Archive contains unrelated project rows",
            )?;
            let stage = private_directory(&self.root, "archive-restore")?;
            staging = Some(stage.clone());
            let diagnostic = serde_json::json!({"formatVersion":ARCHIVE_VERSION,"sourceArchive":path,"projectId":meta.project_id,"destinationRoot":self.root,"mode":"preserve_identity"});
            let mut report = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(stage.join("restore.json"))?;
            report.write_all(
                &serde_json::to_vec_pretty(&diagnostic).map_err(|e| invalid(e.to_string()))?,
            )?;
            report.sync_all()?;
            restore_object_files(&archive, &stage, &mut bundle)?;
            ensure(
                bundle.totals == meta.totals && bundle.index_digest() == meta.index,
                "Archive closure, byte budget or metadata index differs from its receipt",
            )?;
            validate_all_reference_snapshots(&archive, &meta.project_id)?;
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            collision_checks(&tx, &archive, &meta.project_id)?;
            install_objects(
                &tx,
                &archive,
                &self.root,
                &stage,
                &bundle,
                &mut published_objects,
            )?;
            copy_metadata(&archive, Some(Destination::Store(&tx)), &meta.project_id)?;
            let mut lineage_bundle = copy_metadata(&archive, None, &meta.project_id)?;
            crate::archive_copy::read_lineage(
                &archive,
                Some((&tx, &meta.project_id, true)),
                &meta.project_id,
                &mut lineage_bundle,
                true,
            )?;
            validate_all_reference_snapshots(&tx, &meta.project_id)?;
            tx.commit()?;
            committed = true;
            // Only this UUID-owned staging directory is removed on success.
            // Failed publication retains it and any content-addressed orphans.
            fs::remove_dir_all(&stage)?;
            staging = None;
            Ok(bundle.project)
        })();
        outcome.map_err(|error:StoreError|invalid(format!("Archive restore failed; staging={staging:?}; destination={}; published_objects={published_objects}; committed={committed}: {error}",self.root.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::contracts::{EditScope, Extensions, OperationId, ProjectId};
    use serde_json::json;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/archive-owned")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn store(&self, name: &str) -> NativeStore {
            NativeStore::open(self.0.join(name)).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!("Archive fixture retained at {}", self.0.display());
            } else {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn rename(project: &ProjectDocument, operation: &str, name: &str) -> ProjectEditIntent {
        ProjectEditIntent {
            operation_id: OperationId(operation.into()),
            project_id: ProjectId(project.id().into()),
            expected_revision: project.revision(),
            scope: EditScope::Project,
            edit: EditOperation::Rename { name: name.into() },
            extensions: Extensions::new(),
        }
    }

    #[test]
    fn complete_legacy_import_history_and_operation_receipts_survive_restore() {
        let fixture = Fixture::new();
        let mut source = fixture.store("source");
        let asset = b"exact immutable source bytes";
        let asset_id = format!("{}.wav", checksum(asset));
        let input = fixture.0.join("input");
        fs::create_dir(&input).unwrap();
        fs::write(input.join(&asset_id), asset).unwrap();
        let mut value = ProjectDocument::new("Legacy original")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["kind"] = "audio".into();
        value["tracks"][0]["assetId"] = asset_id.clone().into();
        value["tracks"][0]["audio"] = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":1.0,"byteLength":asset.len()});
        value["vendor:metadata"] = json!({"recording":{"takeId":"foreign"}});
        let mut original = serde_json::to_vec(&value).unwrap();
        original.extend(b"\n  ");
        let source_json = input.join("source.json");
        fs::write(&source_json, &original).unwrap();
        let imported = source.import_legacy(&source_json, &input).unwrap();
        let mut request = rename(&imported, "rename-original", "Renamed");
        request
            .extensions
            .insert("kind".into(), json!("vendor-operation"));
        request
            .extensions
            .insert("version".into(), json!({"opaque":1}));
        request.extensions.insert("intent".into(),serde_json::from_str(r#"{"large":184467440737095516160000001,"precise":0.123456789012345678901234567890}"#).unwrap());
        let renamed = source.apply_edit(&request).unwrap().document;
        let mut value = renamed.value().clone();
        value["tracks"][0]
            .as_object_mut()
            .unwrap()
            .remove("assetId");
        value["tracks"][0].as_object_mut().unwrap().remove("audio");
        let without_asset = source
            .save(
                &ProjectDocument::from_value(value).unwrap(),
                renamed.revision(),
            )
            .unwrap();
        assert!(without_asset.referenced_assets().unwrap().is_empty());
        let source_db = fs::read(source.root.join("studio.sqlite3")).unwrap();
        let archive = fixture.0.join("complete.sqlite3");
        let receipt = source.export_archive(imported.id(), &archive).unwrap();
        assert_eq!(receipt.asset_count, 1);
        assert_eq!(receipt.history_rows, 2);
        assert_eq!(receipt.operation_rows, 1);
        assert_eq!(receipt.sha256, checksum(&fs::read(&archive).unwrap()));
        assert_eq!(
            fs::read(source.root.join("studio.sqlite3")).unwrap(),
            source_db
        );
        assert_eq!(fs::read(&source_json).unwrap(), original);
        let mut destination = fixture.store("destination");
        let unrelated = destination
            .create(&ProjectDocument::new("Unrelated").unwrap())
            .unwrap();
        assert_eq!(
            destination.restore_archive(&archive).unwrap(),
            without_asset
        );
        assert_eq!(destination.load(unrelated.id()).unwrap(), unrelated);
        assert_eq!(
            destination.imported_original(imported.id()).unwrap(),
            Some(original)
        );
        assert_eq!(destination.asset(&asset_id).unwrap(), asset);
        let retry = destination.apply_edit(&request).unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.document, renamed);
        assert_eq!(destination.load(imported.id()).unwrap(), without_asset);
        let undone = destination
            .undo(imported.id(), without_asset.revision())
            .unwrap();
        assert!(undone.referenced_assets().unwrap().contains(&asset_id));
        let redone = destination.redo(imported.id(), undone.revision()).unwrap();
        assert!(redone.referenced_assets().unwrap().is_empty());
    }

    #[test]
    fn export_never_overwrites_and_restore_identity_conflicts_do_not_mutate() {
        let fixture = Fixture::new();
        let mut source = fixture.store("source");
        let document = source
            .create(&ProjectDocument::new("Original").unwrap())
            .unwrap();
        let archive = fixture.0.join("project.sqlite3");
        source.export_archive(document.id(), &archive).unwrap();
        let bytes = fs::read(&archive).unwrap();
        assert!(source.export_archive(document.id(), &archive).is_err());
        assert_eq!(fs::read(&archive).unwrap(), bytes);
        #[cfg(unix)]
        {
            let link = fixture.0.join("dangling");
            std::os::unix::fs::symlink(fixture.0.join("missing"), &link).unwrap();
            assert!(source.export_archive(document.id(), &link).is_err());
            assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
        }
        let before = source.load(document.id()).unwrap();
        assert!(source.restore_archive(&archive).is_err());
        assert_eq!(source.load(document.id()).unwrap(), before);
        assert_eq!(source.list().unwrap().len(), 1);
    }

    #[test]
    fn global_operation_collision_and_final_sql_failure_roll_back_publication() {
        let fixture = Fixture::new();
        let mut source = fixture.store("source");
        let project = source
            .create(&ProjectDocument::new("Source").unwrap())
            .unwrap();
        source
            .apply_edit(&rename(&project, "shared-operation", "Changed"))
            .unwrap();
        let archive = fixture.0.join("project.sqlite3");
        source.export_archive(project.id(), &archive).unwrap();
        let mut destination = fixture.store("collision");
        let other = destination
            .create(&ProjectDocument::new("Other").unwrap())
            .unwrap();
        let other = destination
            .apply_edit(&rename(&other, "shared-operation", "Other changed"))
            .unwrap()
            .document;
        assert!(destination.restore_archive(&archive).is_err());
        assert_eq!(destination.list().unwrap().len(), 1);
        assert_eq!(destination.load(other.id()).unwrap(), other);

        let mut fail = fixture.store("failure");
        fail.connection.execute_batch("CREATE TRIGGER archive_final_failure BEFORE INSERT ON operations BEGIN SELECT RAISE(ABORT,'fixture final operation failure'); END;").unwrap();
        let error = fail.restore_archive(&archive).unwrap_err().to_string();
        assert!(error.contains("committed=false"));
        assert!(error.contains("staging=Some"));
        for table in [
            "projects",
            "edits",
            "operations",
            "imports",
            "assets",
            "native_takes",
        ] {
            assert_eq!(count(&fail.connection, table).unwrap(), 0, "{table}");
        }
        assert!(fail.root.read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".omniusong-archive-restore-")
        }));
    }

    #[test]
    fn malformed_schema_checksum_counts_and_budget_fail_before_publication() {
        let fixture = Fixture::new();
        let mut source = fixture.store("source");
        let project = source
            .create(&ProjectDocument::new("Original").unwrap())
            .unwrap();
        let archive = fixture.0.join("original.sqlite3");
        source.export_archive(project.id(), &archive).unwrap();
        for (index, sql) in [
            "PRAGMA application_id=0",
            "PRAGMA user_version=3",
            "CREATE TABLE unexpected(value TEXT)",
            "UPDATE projects SET payload_sha256=printf('%064d',0)",
            "UPDATE archive_meta SET edit_count=1",
            "UPDATE archive_meta SET metadata_bytes=268435457",
            "UPDATE archive_meta SET index_sha256=printf('%064d',0)",
            "UPDATE projects SET payload=zeroblob(8388609)",
        ]
        .into_iter()
        .enumerate()
        {
            let corrupted = fixture.0.join(format!("corrupt-{index}.sqlite3"));
            fs::copy(&archive, &corrupted).unwrap();
            let db = Connection::open(&corrupted).unwrap();
            db.execute_batch(sql).unwrap();
            drop(db);
            let mut destination = fixture.store(&format!("destination-{index}"));
            assert!(destination.restore_archive(&corrupted).is_err(), "{sql}");
            assert!(destination.list().unwrap().is_empty());
        }
        let truncated = fixture.0.join("truncated.sqlite3");
        fs::write(&truncated, &fs::read(&archive).unwrap()[..64]).unwrap();
        assert!(
            fixture
                .store("truncated-destination")
                .restore_archive(&truncated)
                .is_err()
        );
    }

    #[test]
    fn unsupported_retained_operation_is_rejected_instead_of_losing_history() {
        let fixture = Fixture::new();
        let mut store = fixture.store("source");
        let project = store
            .create(&ProjectDocument::new("Original").unwrap())
            .unwrap();
        let request = rename(&project, "operation-1", "Changed");
        store.apply_edit(&request).unwrap();
        let unknown = r#"{"kind":"future_operation","version":1,"intent":{}}"#;
        store
            .connection
            .execute(
                "UPDATE operations SET request_json=?1,request_sha256=?2 WHERE operation_id=?3",
                params![
                    unknown,
                    checksum(unknown.as_bytes()),
                    request.operation_id.0
                ],
            )
            .unwrap();
        let archive = fixture.0.join("must-not-exist.sqlite3");
        let error = store
            .export_archive(project.id(), &archive)
            .unwrap_err()
            .to_string();
        assert!(!archive.exists());
        assert!(error.contains("Unknown archived operation wrapper"));
        assert!(error.contains("staging=Some"));
    }
}

//! Owned immutable take objects and atomic publication into native projects.
//! Files become durable before registration; project/history/receipt share SQL.
use super::{
    EditReceipt, NativeStore, Result, StoreError, load_document,
    operations::{insert_operation, replay_operation},
    save_in_transaction,
};
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TakeId, TrackId, Validate, WORKER_MAX_BYTES},
    native::{TakeClip, TakeRange, TakeReference},
};
use omniusong_recording::{SealedJournal, SealedManifest, SourcePosition, StreamKind};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const MAX_MANIFEST_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeObject {
    pub take_id: TakeId,
    pub manifest_sha256: String,
    pub object_sha256: String,
    pub byte_length: u64,
    pub manifest: SealedManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakePublication {
    pub operation_id: OperationId,
    pub project_id: ProjectId,
    pub expected_revision: u64,
    pub track_id: TrackId,
    pub track_name: String,
    pub clip: TakeClip,
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(StoreError::Invalid(message.into()))
    }
}

fn is_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl TakePublication {
    pub fn validate(&self) -> Result<()> {
        self.operation_id
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        self.project_id
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        self.track_id
            .validate()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        require(
            self.track_id
                .0
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)),
            "Invalid native recording track identity",
        )?;
        require(
            i64::try_from(self.expected_revision)
                .ok()
                .and_then(|value| value.checked_add(1))
                .is_some(),
            "Take publication revision is out of range",
        )?;
        require(
            !self.track_name.trim().is_empty() && self.track_name.chars().count() <= 160,
            "Invalid native recording track name",
        )?;
        self.clip.validate()?;
        require(
            self.clip.edit_source.is_none(),
            "Publishing an original take cannot supply a claimed edit snapshot",
        )?;
        Ok(())
    }
}

pub(crate) fn publication_request(intent: &TakePublication) -> Result<String> {
    intent.validate()?;
    #[derive(Serialize)]
    struct Request<'a> {
        kind: &'static str,
        version: u32,
        intent: &'a TakePublication,
    }
    let request = serde_json::to_string(&Request {
        kind: "take_publication",
        version: 1,
        intent,
    })
    .map_err(|error| StoreError::Invalid(error.to_string()))?;
    require(
        request.len() <= WORKER_MAX_BYTES,
        "Take publication exceeds its operation size limit",
    )?;
    Ok(request)
}

fn validate_object(object: &TakeObject) -> Result<()> {
    object
        .take_id
        .validate()
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    require(
        is_sha(&object.object_sha256) && is_sha(&object.manifest_sha256),
        "Invalid owned take digest",
    )?;
    require(
        object.byte_length > 0 && i64::try_from(object.byte_length).is_ok(),
        "Invalid owned take byte length",
    )?;
    object.manifest.verify_hash()?;
    require(
        object.manifest.sha256 == object.manifest_sha256
            && object.manifest.body.descriptor.take_id == object.take_id,
        "Registered take differs from its sealed identity",
    )?;
    Ok(())
}

/// Bounded registry load. Availability/authenticity of its file is a separate check.
pub(crate) fn load_take(connection: &Connection, take_id: &TakeId) -> Result<TakeObject> {
    take_id
        .validate()
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    let row: Option<(String, String, i64, Option<String>)> = connection.query_row(
        "SELECT substr(manifest_sha256,1,65),substr(object_sha256,1,65),byte_length,CASE WHEN octet_length(manifest_json)<=?2 THEN manifest_json END FROM native_takes WHERE take_id=?1",
        params![take_id.0, MAX_MANIFEST_BYTES as i64],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?;
    let (manifest_sha256, object_sha256, byte_length, json) = row.ok_or_else(|| {
        StoreError::Invalid(format!("Native take is not registered: {}", take_id.0))
    })?;
    let json = json.ok_or_else(|| {
        StoreError::Invalid("Registered take manifest exceeds its byte limit".into())
    })?;
    let manifest: SealedManifest = serde_json::from_str(&json)
        .map_err(|error| StoreError::Invalid(format!("Invalid registered manifest: {error}")))?;
    let byte_length = u64::try_from(byte_length)
        .map_err(|_| StoreError::Invalid("Negative registered take byte length".into()))?;
    let object = TakeObject {
        take_id: take_id.clone(),
        manifest_sha256,
        object_sha256,
        byte_length,
        manifest,
    };
    validate_object(&object)?;
    require(
        object.manifest.canonical_bytes()? == json.as_bytes(),
        "Registered manifest is not canonical",
    )?;
    Ok(object)
}

fn has_take(connection: &Connection, take_id: &TakeId) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_takes WHERE take_id=?1)",
        [&take_id.0],
        |row| row.get(0),
    )?)
}

/// Resolve only the real owned directory; neither a symlink nor a redirect is admitted.
fn takes_directory(root: &Path, create: bool) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(root)?;
    require(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Native store root is not a real directory",
    )?;
    let root = fs::canonicalize(root)?;
    let directory = root.join("takes");
    if create {
        match fs::create_dir(&directory) {
            Ok(()) => File::open(&root)?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let metadata = fs::symlink_metadata(&directory)?;
    require(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && fs::canonicalize(&directory)? == directory,
        "Owned take directory is redirected or invalid",
    )?;
    Ok(directory)
}

pub(crate) fn object_path(root: &Path, sha: &str) -> Result<PathBuf> {
    require(is_sha(sha), "Invalid take object filename digest")?;
    Ok(takes_directory(root, false)?.join(format!("{sha}.sqlite3")))
}

fn ordinary_object(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Owned take is not an ordinary nonsymlink file",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.nlink() == 1,
            "Hardlinked owned take objects are unsupported",
        )?;
    }
    Ok(metadata)
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            left.dev() == right.dev() && left.ino() == right.ino(),
            "Owned take path changed during verification",
        )?;
    }
    require(
        left.len() == right.len(),
        "Owned take length changed during verification",
    )
}

fn hash_file(mut file: &File) -> Result<(u64, String)> {
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        count = count
            .checked_add(size as u64)
            .ok_or_else(|| StoreError::Invalid("Owned take byte count overflow".into()))?;
        hash.update(&buffer[..size]);
    }
    Ok((count, format!("{:x}", hash.finalize())))
}

pub(crate) fn verify_owned_take(root: &Path, object: &TakeObject) -> Result<SealedJournal> {
    validate_object(object)?;
    let path = object_path(root, &object.object_sha256)?;
    let before = ordinary_object(&path)?;
    require(
        before.len() == object.byte_length,
        "Owned take file length differs from its registration",
    )?;
    let file = File::open(&path)?;
    same_file(&before, &file.metadata()?)?;
    let sealed = SealedJournal::open(&path)?;
    require(
        sealed.manifest() == &object.manifest && sealed.descriptor().take_id == object.take_id,
        "Owned take differs from its registered sealed manifest",
    )?;
    same_file(&before, &ordinary_object(&path)?)?;
    let (count, sha) = hash_file(&file)?;
    require(
        count == object.byte_length && sha == object.object_sha256,
        "Owned take file hash is damaged",
    )?;
    same_file(&before, &file.metadata()?)?;
    same_file(&before, &ordinary_object(&path)?)?;
    Ok(sealed)
}

fn check_reference(object: &TakeObject, reference: &TakeReference) -> Result<()> {
    reference.validate()?;
    require(
        reference.take_id == object.take_id && reference.manifest_sha256 == object.manifest_sha256,
        "Take reference does not match its registered sealed identity",
    )?;
    require(
        object
            .manifest
            .body
            .descriptor
            .streams
            .iter()
            .any(|stream| stream.stream_id == reference.stream_id),
        "Take reference selects an unknown stream",
    )
}

fn check_clip(object: &TakeObject, clip: &TakeClip) -> Result<()> {
    check_clip_window(object, clip)?;
    if let Some(original) = clip.original_view()? {
        check_clip_window(object, &original)?;
    }
    Ok(())
}

fn check_clip_window(object: &TakeObject, clip: &TakeClip) -> Result<()> {
    clip.validate()?;
    check_reference(object, &clip.source)?;
    let stream = object
        .manifest
        .body
        .descriptor
        .streams
        .iter()
        .find(|stream| stream.stream_id == clip.source.stream_id)
        .ok_or_else(|| StoreError::Invalid("Unknown take clip stream".into()))?;
    let prefix = object
        .manifest
        .body
        .streams
        .iter()
        .find(|prefix| prefix.stream_id == clip.source.stream_id)
        .ok_or_else(|| StoreError::Invalid("Sealed take has no selected stream prefix".into()))?;
    require(
        clip.range.start() >= stream.initial_source_position
            && clip.range.end()? <= prefix.source_end.value(),
        "Take clip exceeds the proven original source window",
    )?;
    match (&stream.kind, &prefix.source_end, &clip.range) {
        (
            StreamKind::Audio { format, .. },
            SourcePosition::AudioFrames(_),
            TakeRange::AudioFrames { .. },
        ) => require(
            clip.position.sample_rate == format.sample_rate,
            "Take source and arrangement sample rates differ; resampling requires an explicit policy",
        ),
        (StreamKind::RawEvents { .. }, SourcePosition::Events(_), TakeRange::Events { .. }) => {
            Ok(())
        }
        _ => Err(StoreError::Invalid(
            "Take source range has the wrong unit for its stream".into(),
        )),
    }
}

/// Group only lightweight clip references, keeping one bounded manifest in memory.
fn visit_document_takes<F>(
    connection: &Connection,
    document: &ProjectDocument,
    mut visitor: F,
) -> Result<()>
where
    F: FnMut(&TakeObject) -> Result<()>,
{
    let clips = document.take_clips()?;
    let mut grouped: BTreeMap<&TakeId, Vec<&TakeClip>> = BTreeMap::new();
    for clip in &clips {
        grouped.entry(&clip.source.take_id).or_default().push(clip);
    }
    for (take_id, clips) in grouped {
        let object = load_take(connection, take_id)?;
        for clip in clips {
            check_clip(&object, clip)?;
        }
        visitor(&object)?;
    }
    Ok(())
}

pub(crate) fn check_take_references(
    connection: &Connection,
    document: &ProjectDocument,
) -> Result<()> {
    visit_document_takes(connection, document, |_| Ok(()))
}

pub(crate) fn verify_document_files(
    root: &Path,
    connection: &Connection,
    document: &ProjectDocument,
) -> Result<()> {
    visit_document_takes(connection, document, |object| {
        verify_owned_take(root, object)?;
        Ok(())
    })
}

impl NativeStore {
    /// Own a durable complete SQLite backup before adding immutable registry metadata.
    /// A later registry failure retains its owned content object as an orphan.
    pub fn ingest_take(&mut self, source: &SealedJournal) -> Result<TakeObject> {
        let take_id = &source.descriptor().take_id;
        if has_take(&self.connection, take_id)? {
            let existing = load_take(&self.connection, take_id)?;
            require(
                existing.manifest == *source.manifest(),
                "Take identity was already registered with different content",
            )?;
            verify_owned_take(&self.root, &existing)?;
            return Ok(existing);
        }
        let directory = takes_directory(&self.root, true)?;
        let staging = directory.join(format!(".ingest-{}.partial", uuid::Uuid::new_v4()));
        let backup = source.export_backup(&staging)?;
        let object = TakeObject {
            take_id: take_id.clone(),
            manifest_sha256: backup.manifest_sha256,
            object_sha256: backup.sha256,
            byte_length: backup.byte_len,
            manifest: source.manifest().clone(),
        };
        validate_object(&object)?;
        require(
            backup.take_id == object.take_id,
            "Backup receipt take identity is inconsistent",
        )?;
        let destination = object_path(&self.root, &object.object_sha256)?;
        match fs::hard_link(&staging, &destination) {
            Ok(()) => fs::remove_file(&staging)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // This may be an earlier failed registration's durable orphan.
                // Authenticate the existing name before retiring our own stage.
                verify_owned_take(&self.root, &object)?;
                fs::remove_file(&staging)?;
            }
            Err(error) => return Err(error.into()),
        }
        File::open(&directory)?.sync_all()?;
        let _verified = verify_owned_take(&self.root, &object)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if has_take(&tx, take_id)? {
            let existing = load_take(&tx, take_id)?;
            require(
                existing.manifest == object.manifest,
                "Take identity was concurrently registered with different content",
            )?;
            verify_owned_take(&self.root, &existing)?;
            tx.commit()?;
            return Ok(existing);
        }
        let byte_length = i64::try_from(object.byte_length)
            .map_err(|_| StoreError::Invalid("Owned take byte length is out of range".into()))?;
        let manifest_json = String::from_utf8(object.manifest.canonical_bytes()?)
            .map_err(|_| StoreError::Invalid("Manifest is not UTF-8".into()))?;
        tx.execute("INSERT INTO native_takes(take_id,manifest_sha256,object_sha256,byte_length,manifest_json) VALUES(?1,?2,?3,?4,?5)",
            params![object.take_id.0, object.manifest_sha256, object.object_sha256, byte_length, manifest_json])?;
        tx.commit()?;
        Ok(object)
    }

    pub fn registered_take(&self, take_id: &TakeId) -> Result<TakeObject> {
        let object = load_take(&self.connection, take_id)?;
        verify_owned_take(&self.root, &object)?;
        Ok(object)
    }

    pub fn open_take(&self, reference: &TakeReference) -> Result<SealedJournal> {
        let object = load_take(&self.connection, &reference.take_id)?;
        check_reference(&object, reference)?;
        verify_owned_take(&self.root, &object)
    }

    /// Attach a completed capture without depending on its staging pathname on
    /// retry. Admission and authenticated owned originals remain authoritative.
    /// A new original is durably ingested before the separate project commit;
    /// a conflict retains it for explicit recovery rather than removing audio.
    pub fn attach_capture(
        &mut self,
        source: impl AsRef<Path>,
        intent: &TakePublication,
    ) -> Result<EditReceipt> {
        let request = publication_request(intent)?;
        {
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
                verify_document_files(&self.root, &tx, &receipt.document)?;
                tx.commit()?;
                return Ok(receipt);
            }
            // Release the project transaction before reading/copying a capture.
            tx.commit()?;
        }
        let registered: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_takes WHERE take_id=?1)",
            [&intent.clip.source.take_id.0],
            |row| row.get(0),
        )?;
        if registered {
            let object = self.registered_take(&intent.clip.source.take_id)?;
            check_reference(&object, &intent.clip.source)?;
        } else {
            let journal = SealedJournal::open(source)?;
            if journal.descriptor().take_id != intent.clip.source.take_id
                || journal.manifest().sha256 != intent.clip.source.manifest_sha256
            {
                return Err(StoreError::Invalid(
                    "Completed capture differs from publication identity".into(),
                ));
            }
            self.ingest_take(&journal)?;
        }
        self.publish_take(intent)
    }

    pub fn publish_take(&mut self, intent: &TakePublication) -> Result<EditReceipt> {
        let request = publication_request(intent)?;
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
            verify_document_files(&self.root, &tx, &receipt.document)?;
            tx.commit()?;
            return Ok(receipt);
        }
        let current = load_document(&tx, &intent.project_id.0)?;
        if current.revision() != intent.expected_revision {
            return Err(StoreError::Conflict);
        }
        // The shared save helper authenticates every resulting distinct take
        // exactly once before any project/history SQL mutation.
        let mut next = if current.schema_version() == 1 {
            current.upgrade_native()?
        } else {
            current
        };
        next.append_take_clip(&intent.track_id.0, &intent.track_name, intent.clip.clone())?;
        let document = save_in_transaction(&tx, &self.root, &next, intent.expected_revision)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_core::native::TimelinePosition;
    use omniusong_recording::{
        ChunkHeader, ChunkKind, ClockDomainId, ClockEpochId, GapReason, HostClock, HostTimestamp,
        Journal, PcmEncoding, PcmFormat, PcmLayout, RawEventFormat, SourceRange, StreamDescriptor,
        TakeDescriptor,
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/rust-migration/store-takes-tests");
            fs::create_dir_all(&parent).unwrap();
            let directory = parent.join(uuid::Uuid::new_v4().to_string());
            fs::create_dir(&directory).unwrap();
            Self(directory)
        }
        fn root(&self) -> PathBuf {
            self.0.join("store")
        }
        fn source(&self, events: bool) -> SealedJournal {
            let kind = if events {
                StreamKind::RawEvents {
                    format: RawEventFormat::LengthPrefixedCaptureV1,
                }
            } else {
                StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::S16Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("audio".into()),
                }
            };
            let descriptor = TakeDescriptor {
                version: 1,
                take_id: TakeId(uuid::Uuid::new_v4().to_string()),
                streams: vec![StreamDescriptor {
                    stream_id: "source".into(),
                    segment_id: "segment".into(),
                    source_id: "endpoint".into(),
                    source_name: "Synthetic input".into(),
                    source_connection_generation: 1,
                    host_clock: HostClock {
                        domain: ClockDomainId("host".into()),
                        epoch: ClockEpochId("boot".into()),
                        ticks_per_second_numerator: 1_000_000_000,
                        ticks_per_second_denominator: 1,
                    },
                    kind: kind.clone(),
                    initial_source_position: 7,
                }],
                provenance: BTreeMap::new(),
            };
            let clock = HostTimestamp {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("boot".into()),
                ticks: u64::MAX,
            };
            let header = ChunkHeader {
                version: 1,
                take_id: descriptor.take_id.clone(),
                stream_id: "source".into(),
                segment_id: "segment".into(),
                sequence: 0,
                range: if events {
                    SourceRange::Events { start: 7, count: 3 }
                } else {
                    SourceRange::AudioFrames { start: 7, count: 2 }
                },
                interpretation: kind,
                observed_start: clock.clone(),
                observed_end: clock,
                clock_anchors: vec![],
                kind: if events {
                    ChunkKind::Gap {
                        reason: GapReason::TransferOverflow,
                    }
                } else {
                    ChunkKind::Content
                },
            };
            let path = self.0.join("source.sqlite3");
            let mut journal = Journal::create(&path, &descriptor).unwrap();
            journal
                .append(&header, if events { &[] } else { &[4; 4] })
                .unwrap();
            journal.seal("seal").unwrap();
            drop(journal);
            SealedJournal::open(path).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn semantic_ingest_retry_reuses_verified_registered_bytes_despite_different_source_layout() {
        let fixture = Fixture::new();
        let source = fixture.source(false);
        let mut store = NativeStore::open(fixture.root()).unwrap();
        let first = store.ingest_take(&source).unwrap();
        let second_path = fixture.0.join("same-manifest-different-layout.sqlite3");
        source.export_backup(&second_path).unwrap();
        let database = Connection::open(&second_path).unwrap();
        database.execute_batch("CREATE TABLE unrelated_retained_data(value BLOB); INSERT INTO unrelated_retained_data VALUES(zeroblob(4096));").unwrap();
        drop(database);
        let second = SealedJournal::open(&second_path).unwrap();
        assert_eq!(source.manifest(), second.manifest());
        let second_hash = hash_file(&File::open(second_path).unwrap()).unwrap().1;
        assert_ne!(first.object_sha256, second_hash);
        assert_eq!(store.ingest_take(&second).unwrap(), first);
        assert_eq!(store.registered_take(&first.take_id).unwrap(), first);
        assert_eq!(
            fs::read_dir(fixture.root().join("takes")).unwrap().count(),
            1
        );
    }

    #[test]
    fn valid_event_window_uses_event_bounds_without_adding_an_audio_duration() {
        let fixture = Fixture::new();
        let source = fixture.source(true);
        let mut store = NativeStore::open(fixture.root()).unwrap();
        store.ingest_take(&source).unwrap();
        let project = store
            .create(&ProjectDocument::new("Raw gesture source").unwrap())
            .unwrap();
        let intent = TakePublication {
            operation_id: OperationId("event-publication".into()),
            project_id: ProjectId(project.id().into()),
            expected_revision: 0,
            track_id: TrackId("event-track".into()),
            track_name: "Original event extent".into(),
            clip: TakeClip {
                edit_source: None,
                id: "event-clip".into(),
                source: TakeReference {
                    take_id: source.descriptor().take_id.clone(),
                    manifest_sha256: source.manifest().sha256.clone(),
                    stream_id: "source".into(),
                },
                range: TakeRange::Events { start: 8, count: 2 },
                position: TimelinePosition {
                    frame: i64::MAX,
                    sample_rate: 96_000,
                },
                fades: None,
            },
        };
        let receipt = store.publish_take(&intent).unwrap();
        assert_eq!(
            receipt.document.take_clips().unwrap().as_slice(),
            std::slice::from_ref(&intent.clip)
        );
        assert_eq!(store.load(project.id()).unwrap(), receipt.document);
        assert_eq!(
            store.open_take(&intent.clip.source).unwrap().manifest(),
            source.manifest()
        );
        assert_eq!(
            receipt.document.take_clips().unwrap()[0].position.frame,
            i64::MAX
        );
    }

    #[test]
    fn registry_caps_metadata_before_materializing_and_rejects_signed_length_damage() {
        for (mutation, expected) in [
            (
                "UPDATE native_takes SET manifest_json=zeroblob(524289)",
                "exceeds its byte limit",
            ),
            (
                "PRAGMA ignore_check_constraints=ON; UPDATE native_takes SET byte_length=-1",
                "Negative registered take byte length",
            ),
        ] {
            let fixture = Fixture::new();
            let source = fixture.source(false);
            let mut store = NativeStore::open(fixture.root()).unwrap();
            let object = store.ingest_take(&source).unwrap();
            let database = Connection::open(fixture.root().join("studio.sqlite3")).unwrap();
            database
                .execute_batch("DROP TRIGGER immutable_take_registration_update")
                .unwrap();
            database.execute_batch(mutation).unwrap();
            let error = store
                .registered_take(&object.take_id)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "Unexpected error: {error}");
            assert!(object_path(&fixture.root(), "../source").is_err());
        }
    }
}

//! Durable local intention review; these commands never start/resume/finish
//! engine recording. Attachment executes only the exact persisted publication.
use crate::{NativeBackend, Reply, ServiceError, store_failure};
use omniusong_core::{
    contracts::{ProjectId, Validate},
    native::TakeRange,
};
use omniusong_engine::{LiveId, LiveScope, PerformanceStart};
use omniusong_store::{StoredPerformanceIntent, TakePublication};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 16 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceFinishIntent {
    pub id: LiveId,
    pub scope: LiveScope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceIntent {
    pub version: u32,
    pub start: PerformanceStart,
    pub project_id: ProjectId,
    pub journal_path: PathBuf,
    pub descriptor_sha256: Option<String>,
    pub finish: Option<PerformanceFinishIntent>,
    pub attachment: Option<TakePublication>,
    pub released: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceIntentReceipt {
    pub revision: u64,
    pub intent: PerformanceIntent,
    pub payload_sha256: String,
}
fn sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn require(ok: bool, message: &str) -> Result<(), ServiceError> {
    if ok {
        Ok(())
    } else {
        Err(ServiceError::rejected(message))
    }
}
pub(super) fn intent_rejection(mut error: ServiceError) -> ServiceError {
    if error.kind == crate::FailureKind::Rejected {
        error.kind = crate::FailureKind::IntentRejected;
        error.outcome_unknown = false;
    }
    error
}
impl PerformanceIntent {
    /// Shape and bounded serialization only: safe at nonblocking submission.
    /// The backend additionally binds the opened store's canonical root.
    pub fn validate(&self) -> Result<(), ServiceError> {
        require(
            self.version == 1 && self.start.is_valid(),
            "Invalid performance intent version or immutable Start",
        )?;
        self.project_id
            .validate()
            .map_err(|e| ServiceError::rejected(e.to_string()))?;
        require(
            self.journal_path.is_absolute()
                && self.journal_path.as_os_str().len() <= 4096
                && !self.journal_path.to_string_lossy().contains('\0'),
            "Performance intent needs a bounded absolute journal path",
        )?;
        require(
            self.descriptor_sha256.as_ref().is_none_or(|s| sha(s)),
            "Invalid original performance descriptor SHA-256",
        )?;
        require(
            self.finish
                .as_ref()
                .is_none_or(|f| f.id.is_valid() && f.scope.is_valid()),
            "Invalid immutable performance Finish ID or scope",
        )?;
        if let Some(publication) = &self.attachment {
            require(
                self.descriptor_sha256.is_some(),
                "Persist the original descriptor identity before choosing performance attachment",
            )?;
            publication
                .validate()
                .map_err(|e| ServiceError::rejected(e.to_string()))?;
            require(
                publication.project_id == self.project_id
                    && publication.clip.source.take_id.0 == self.start.take_id()
                    && publication.clip.source.stream_id == "audio"
                    && sha(&publication.clip.source.manifest_sha256)
                    && matches!(publication.clip.range,TakeRange::AudioFrames { start:0,count } if count>0)
                    && publication.clip.position.frame == 0,
                "Performance attachment differs from its project/take/audio/frame-zero choice",
            )?;
        }
        encode(self).map(|_| ())
    }
    fn validate_root(&self, root: &Path) -> Result<(), ServiceError> {
        require(
            self.journal_path
                == root
                    .join("captures")
                    .join(self.start.directory_name())
                    .join("take.sqlite3"),
            "Performance intention path differs from the opened store's exact owned stage",
        )
    }
}
struct Budget(Vec<u8>);
impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("Performance intent exceeds16KiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode(intent: &PerformanceIntent) -> Result<Vec<u8>, ServiceError> {
    let mut writer = Budget(Vec::new());
    serde_json::to_writer(&mut writer, intent)
        .map_err(|e| ServiceError::rejected(e.to_string()))?;
    Ok(writer.0)
}
fn transition(previous: &PerformanceIntent, next: &PerformanceIntent) -> Result<(), ServiceError> {
    require(
        previous.version == next.version
            && previous.start == next.start
            && previous.project_id == next.project_id
            && previous.journal_path == next.journal_path,
        "Performance Start, target project and original path are immutable",
    )?;
    require(
        previous.descriptor_sha256.is_none()
            || previous.descriptor_sha256 == next.descriptor_sha256,
        "Original descriptor identity cannot change or disappear",
    )?;
    require(
        previous.finish.is_none() || previous.finish == next.finish,
        "Persisted performance Finish cannot change or disappear",
    )?;
    require(
        previous.attachment.is_none() || previous.attachment == next.attachment,
        "Persisted performance attachment cannot change or disappear",
    )?;
    require(
        !previous.released || next.released,
        "Released performance intention cannot be reopened",
    )?;
    if previous.released || next.released {
        require(
            previous.descriptor_sha256 == next.descriptor_sha256
                && previous.finish == next.finish
                && previous.attachment == next.attachment,
            "Release cannot add or replace any retained performance field",
        )?;
    }
    Ok(())
}
fn decode_history(
    rows: &[StoredPerformanceIntent],
    root: &Path,
    store: &omniusong_store::NativeStore,
) -> Result<Vec<PerformanceIntentReceipt>, ServiceError> {
    let mut current: BTreeMap<String, PerformanceIntentReceipt> = BTreeMap::new();
    let mut proofs: BTreeMap<String, (TakePublication, bool)> = BTreeMap::new();
    for row in rows {
        let mut decode = || -> Result<PerformanceIntentReceipt, ServiceError> {
            let intent: PerformanceIntent = serde_json::from_slice(&row.payload)
                .map_err(|e| ServiceError::rejected(e.to_string()))?;
            intent.validate()?;
            intent.validate_root(root)?;
            require(
                intent.start.directory_name() == row.id
                    && (1..=16).contains(&row.revision)
                    && sha(&row.payload_sha256),
                "Stored performance intention identity/revision/hash is inconsistent",
            )?;
            if let Some(previous) = current.get(&row.id) {
                require(
                    previous.revision + 1 == row.revision,
                    "Stored typed performance history is not contiguous",
                )?;
                transition(&previous.intent, &intent)?;
            } else {
                require(
                    row.revision == 1,
                    "Stored typed performance history has no initial revision",
                )?;
            }
            if intent.released
                && let Some(publication) = &intent.attachment
            {
                let committed = if let Some((previous, known)) =
                    proofs.get(&publication.operation_id.0)
                {
                    require(
                        previous == publication,
                        "One durable attachment operation is claimed by different performance publications",
                    )?;
                    *known
                } else {
                    let known = store
                        .performance_attachment_committed(publication)
                        .map_err(|e| store_failure(e, false))?;
                    proofs.insert(
                        publication.operation_id.0.clone(),
                        (publication.clone(), known),
                    );
                    known
                };
                require(
                    committed,
                    "Released attached performance has no exact verified historical publication",
                )?;
            }
            Ok(PerformanceIntentReceipt {
                revision: row.revision,
                intent,
                payload_sha256: row.payload_sha256.clone(),
            })
        };
        let receipt=decode().map_err(|e|if e.kind==crate::FailureKind::StorageUnavailable {e} else {ServiceError::backend(format!("Stored performance intentions are damaged or unsupported: {}. Originals and requests were retained; no engine action was started.",e.message),false)})?;
        current.insert(row.id.clone(), receipt);
    }
    Ok(current.into_values().collect())
}
impl NativeBackend {
    pub(super) fn list_performance_intentions(
        &mut self,
    ) -> Result<Vec<PerformanceIntentReceipt>, ServiceError> {
        let store = self.store()?;
        let root = store.root().to_owned();
        let rows = store
            .list_performance_intents()
            .map_err(|e| store_failure(e, false))?;
        decode_history(&rows, &root, store)
    }
    pub(super) fn save_performance_intention(
        &mut self,
        expected_revision: Option<u64>,
        intent: &PerformanceIntent,
    ) -> Result<Reply, ServiceError> {
        intent.validate()?;
        let root = self.store()?.root().to_owned();
        intent.validate_root(&root)?;
        let bytes = encode(intent)?;
        let rows = self
            .store()?
            .list_performance_intents()
            .map_err(|e| store_failure(e, false))?;
        let previous = decode_history(&rows, &root, self.store()?)?;
        if let Some(publication) = &intent.attachment {
            require(
                !publication
                    .operation_id
                    .0
                    .starts_with(omniusong_store::SOURCE_ATTACHMENT_OPERATION_PREFIX)
                    || previous.iter().any(|p| {
                        p.intent.start.id == intent.start.id
                            && p.intent.attachment == intent.attachment
                    }),
                "Reserved source attachment operations cannot be newly claimed by recording intentions",
            )?;
        }
        if let Some(previous) = previous
            .iter()
            .find(|p| p.intent.start.id == intent.start.id)
        {
            transition(&previous.intent, intent)?;
        }
        if intent.released
            && let Some(publication) = &intent.attachment
        {
            let committed = self
                .store()?
                .performance_attachment_committed(publication)
                .map_err(|e| store_failure(e, false))?;
            require(
                committed,
                "An attached performance cannot be released before its exact durable publication is verified",
            )?;
        }
        let key = intent.start.directory_name();
        let observed = rows.iter().rev().find(|row| row.id == key);
        let normalized = if let Some(observed) = observed {
            if observed.payload != bytes && expected_revision != Some(observed.revision) {
                return Err(store_failure(omniusong_store::StoreError::Conflict, false));
            }
            if observed.payload != bytes {
                require(
                    observed.revision < 16,
                    "Performance intention reached its16 retained revisions; no request was removed",
                )?;
            }
            Some(observed.revision)
        } else {
            if expected_revision.is_some() {
                return Err(store_failure(omniusong_store::StoreError::Conflict, false));
            }
            require(
                previous.len() < 32,
                "All32 retained performance intentions are occupied; no original request was removed",
            )?;
            None
        };
        // Normalize replay to the observed revision too. If another writer adds
        // different immutable fields between validation and save, atomic CAS
        // rejects instead of treating a guessed future expectation as permission.
        let row = self
            .store()?
            .save_performance_intent(&key, normalized, &bytes)
            .map_err(|e| store_failure(e, true))?;
        // Verify the committed exact bytes and typed ALL-history transitions,
        // including an independent current-byte CAS replay after a lost reply.
        let latest = self.list_performance_intentions()?;
        if row.payload != bytes
            || !latest.iter().any(|receipt| {
                receipt.intent.start.id == intent.start.id
                    && receipt.revision == row.revision
                    && receipt.intent == *intent
                    && receipt.payload_sha256 == row.payload_sha256
            })
        {
            return Err(ServiceError::backend(
                "Saved performance intention was superseded or differs from its exact committed request; inspect the retained history before any audio continuation",
                true,
            ));
        }
        let receipt = PerformanceIntentReceipt {
            revision: row.revision,
            intent: intent.clone(),
            payload_sha256: row.payload_sha256,
        };
        Ok(Reply::PerformanceIntent(Box::new(receipt)))
    }
    fn reviewed_performance_attachment(
        &mut self,
        id: LiveId,
        revision: u64,
    ) -> Result<(PerformanceIntent, TakePublication), ServiceError> {
        let current = self.list_performance_intentions()?;
        let receipt = current
            .into_iter()
            .find(|p| p.intent.start.id == id)
            .ok_or_else(|| {
                ServiceError::rejected("Choose an existing durable performance intention")
            })?;
        require(
            receipt.revision == revision,
            "Performance intention changed; review the exact latest stored attachment",
        )?;
        require(
            !receipt.intent.released,
            "Released performance intention cannot attach",
        )?;
        let publication = receipt.intent.attachment.clone().ok_or_else(|| {
            ServiceError::rejected(
                "Persist an explicit attachment choice before attaching a performance",
            )
        })?;
        let committed = self
            .store()?
            .performance_attachment_committed(&publication)
            .map_err(|e| store_failure(e, false))?;
        if !committed {
            // Never treat an owned-original authentication failure as absence.
            // An owned source, when present, is authoritative over staging.
            let registered = self
                .store()?
                .has_registered_take(&publication.clip.source.take_id)
                .map_err(|e| store_failure(e, false))?;
            let sealed = if registered {
                self.store()?
                    .open_take(&publication.clip.source)
                    .map_err(|e| store_failure(e, false))?
            } else {
                guard_stage(self.store()?.root(), &receipt.intent.journal_path)?;
                omniusong_recording::SealedJournal::open(&receipt.intent.journal_path)
                    .map_err(|e| ServiceError::backend(e.to_string(), false))?
            };
            let descriptor = sealed.descriptor();
            let profile = omniusong_engine::performance_records::descriptor_profile(descriptor)
                .map_err(ServiceError::rejected)?;
            let config = omniusong_engine::performance_records::from_descriptor(descriptor)
                .map_err(ServiceError::rejected)?;
            require(
                profile == omniusong_engine::performance_records::PerformanceProfile::FinishV2
                    && Some(
                        descriptor
                            .sha256()
                            .map_err(|e| ServiceError::backend(e.to_string(), false))?,
                    ) == receipt.intent.descriptor_sha256
                    && config.take_id.0 == receipt.intent.start.take_id()
                    && config.gui_clock == receipt.intent.start.gui_clock
                    && sealed.manifest().sha256 == publication.clip.source.manifest_sha256,
                "Sealed performance original does not match its saved v2 descriptor, Start clock or manifest",
            )?;
            require(
                sealed.manifest().body.interrupted.is_none(),
                "Recovered performance attachment requires a separate performance-aware recovery workflow",
            )?;
            let inspection =
                omniusong_engine::performance_writer::inspect_sealed_performance(&sealed)
                    .map_err(ServiceError::rejected)?;
            let manifest = &sealed.manifest().body;
            let audio_end = manifest
                .streams
                .iter()
                .find(|stream| stream.stream_id == "audio")
                .map(|stream| stream.source_end.value());
            require(
                inspection.selection.as_ref().is_some_and(|prefix| {
                    prefix.chunk_count == manifest.chunk_count
                        && prefix.chunk_index_sha256 == manifest.chunk_index_sha256
                }) && inspection.problem.is_none()
                    && inspection.capture_fault.is_none()
                    && !inspection.capture_fault_pending
                    && inspection.finish_cut.is_some_and(|cut| {
                        audio_end == Some(cut.cut_frame)
                            && receipt.intent.finish.as_ref().is_some_and(|finish| {
                                finish.id == cut.fence.id && finish.scope == cut.fence.scope
                            })
                    }),
                "Sealed performance lacks its exact complete fault-free v2 Finish/checkpoint and saved Finish intention",
            )?;
        }
        Ok((receipt.intent, publication))
    }
    pub(super) fn attach_performance_intention(
        &mut self,
        id: LiveId,
        revision: u64,
    ) -> Result<Reply, ServiceError> {
        let (intent, publication) = self
            .reviewed_performance_attachment(id, revision)
            .map_err(intent_rejection)?;
        let committed = self
            .store()?
            .attach_capture(&intent.journal_path, &publication)
            .map_err(|e| store_failure(e, true))?;
        let replayed = committed.replayed;
        let document = if replayed {
            self.store()?
                .load(&publication.project_id.0)
                .map_err(|e| store_failure(e, false))?
        } else {
            committed.document
        };
        self.document(document, replayed)
    }
}

fn guard_stage(root: &Path, path: &Path) -> Result<(), ServiceError> {
    use std::io::Read;
    let captures = root.join("captures");
    let stage = path
        .parent()
        .ok_or_else(|| ServiceError::rejected("Performance stage has no owned parent"))?;
    for directory in [root, captures.as_path(), stage] {
        let metadata = std::fs::symlink_metadata(directory)
            .map_err(|e| ServiceError::backend(e.to_string(), false))?;
        require(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && std::fs::canonicalize(directory)
                    .map_err(|e| ServiceError::backend(e.to_string(), false))?
                    == directory,
            "Performance original directory is redirected or nonordinary",
        )?;
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|e| ServiceError::backend(e.to_string(), false))?;
    require(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && std::fs::canonicalize(path)
                .map_err(|e| ServiceError::backend(e.to_string(), false))?
                == path,
        "Performance journal is redirected or nonordinary",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.nlink() == 1,
            "Hard-linked performance stage originals are unsupported",
        )?;
    }
    // Read-only SQLite can still create shared-memory sidecars for a WAL
    // database. Reject that mode before passing an untrusted stage to SQLite.
    let mut header = [0u8; 20];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| ServiceError::backend(error.to_string(), false))?;
    require(
        &header[..16] == b"SQLite format 3\0" && header[18] == 1 && header[19] == 1,
        "Performance stage requires rollback-journal SQLite; no WAL or repair was opened",
    )?;
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        require(
            std::fs::symlink_metadata(PathBuf::from(sidecar))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "Performance stage has an ambiguous WAL/shared-memory sidecar",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Backend, Command, FailureKind, LegacyScan, Paths};
    use omniusong_core::{
        contracts::{
            ClockEpochId, EditOperation, EditScope, EndpointId, OperationId, ProjectEditIntent,
            TakeId, TrackId,
        },
        native::{TakeClip, TakeReference, TimelinePosition},
    };
    use omniusong_engine::{
        LiveClock, LiveEvent, LiveEventKind, LivePatch, LiveSourceBinding, LiveSourceKind,
        performance_records::{
            PerformanceConfig, PerformanceProfile, PerformanceSource, create_descriptor_for,
        },
        performance_tap::TapInfo,
        performance_writer::PerformanceWriter,
    };
    use omniusong_recording::{
        CHUNK_VERSION, CaptureReader, ChunkHeader, ChunkKind, HostTimestamp, Journal, SourceRange,
    };
    use std::fs;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/performance-intent-session-tests")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn backend(&self) -> NativeBackend {
            let mut backend = NativeBackend {
                paths: Paths {
                    native: self.0.join("native"),
                    legacy: self.0.join("legacy"),
                },
                store: None,
                legacy: LegacyScan::default(),
                legacy_notice: None,
                require_existing: false,
            };
            backend.handle(&Command::Initialize).unwrap();
            backend
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn initial(backend: &mut NativeBackend) -> PerformanceIntent {
        let Reply::Document { view, .. } = backend
            .handle(&Command::Create {
                name: "Synthetic intention target".into(),
            })
            .unwrap()
        else {
            panic!("document")
        };
        let start = PerformanceStart {
            id: LiveId([7; 16]),
            output_device: Some("synthetic-output-only".into()),
            gui_clock: LiveClock {
                domain: "fixture-gui".into(),
                epoch: "synthetic-epoch".into(),
                ticks_per_second_numerator: 1000,
                ticks_per_second_denominator: 1,
            },
        };
        PerformanceIntent {
            version: 1,
            journal_path: backend
                .store()
                .unwrap()
                .root()
                .join("captures")
                .join(start.directory_name())
                .join("take.sqlite3"),
            start,
            project_id: ProjectId(view.document.id().into()),
            descriptor_sha256: None,
            finish: None,
            attachment: None,
            released: false,
        }
    }
    fn save(
        backend: &mut NativeBackend,
        expected: Option<u64>,
        intent: &PerformanceIntent,
    ) -> Result<PerformanceIntentReceipt, ServiceError> {
        let Reply::PerformanceIntent(receipt) =
            backend.handle(&Command::SavePerformanceIntent {
                expected_revision: expected,
                intent: Box::new(intent.clone()),
            })?
        else {
            panic!("receipt")
        };
        Ok(*receipt)
    }
    fn listed(backend: &mut NativeBackend) -> Result<Vec<PerformanceIntentReceipt>, ServiceError> {
        let Reply::PerformanceIntents(rows) = backend.handle(&Command::ListPerformanceIntents)?
        else {
            panic!("rows")
        };
        Ok(rows)
    }
    fn config(intent: &PerformanceIntent) -> PerformanceConfig {
        PerformanceConfig {
            take_id: TakeId(intent.start.take_id()),
            info: TapInfo {
                scope: LiveScope {
                    worker_generation: LiveId([1; 16]),
                    session_id: LiveId([2; 16]),
                    panic_generation: 1,
                },
                sample_rate: 48_000,
                channels: 1,
            },
            gui_clock: intent.start.gui_clock.clone(),
            output_epoch: ClockEpochId("synthetic-output-epoch".into()),
            sources: std::array::from_fn(|index| PerformanceSource {
                binding: LiveSourceBinding {
                    kind: [
                        LiveSourceKind::ComputerKeys,
                        LiveSourceKind::PointerKeys,
                        LiveSourceKind::Pads,
                        LiveSourceKind::AccessiblePulse,
                    ][index],
                    handle: index as u8 + 1,
                    connection_generation: 1,
                },
                endpoint: EndpointId(format!("synthetic-source-{}", index + 1)),
            }),
        }
    }
    /// Build an authentic CPU callback/checkpoint original, then copy its exact
    /// chunks into a normal trusted-storage fixture. This uses the PUBLIC lower
    /// primitive deliberately; it does NOT mint native lifecycle/seal authority.
    fn normal_source(fixture: &Fixture, intent: &mut PerformanceIntent, recovered: bool) {
        let config = config(intent);
        let seed = fixture.0.join("seed.sqlite3");
        let mut writer = PerformanceWriter::create_finishable(&seed, config.clone()).unwrap();
        let (mut admission, mut renderer, mut owner) =
            omniusong_engine::live::recording_live_pair(config.info.scope, 48_000, 1).unwrap();
        let mut batch = omniusong_engine::LiveBatch::empty(config.info.scope, 1, None);
        batch
            .try_push(LiveEvent {
                source: 1,
                connection_generation: 1,
                observed_ticks: 1,
                event: LiveEventKind::NoteOn {
                    note: 1,
                    voice: 1,
                    key: 60,
                    velocity16: 49_152,
                    patch: LivePatch::Keys,
                },
            })
            .unwrap();
        admission.admit(batch).unwrap();
        let fence = admission
            .begin_finish(config.info.scope, LiveId([8; 16]))
            .unwrap();
        renderer.render_into(&mut [0.0; 128]);
        while let Some(packet) = owner.try_pop().unwrap() {
            writer.push_packet(packet).unwrap();
        }
        writer.flush().unwrap();
        let descriptor = writer.descriptor().clone();
        drop((renderer, admission, owner, writer));
        let reader = CaptureReader::open(&seed).unwrap();
        let count = reader.inspection().report.verified_chunks;
        let chunks: Vec<_> = (1..=count)
            .map(|ordinal| reader.read_ordinal(ordinal).unwrap())
            .collect();
        drop(reader);
        let mut journal = Journal::create(&intent.journal_path, &descriptor).unwrap();
        for chunk in &chunks {
            journal.append(&chunk.header, &chunk.payload).unwrap();
        }
        let complete = journal.complete_prefix().unwrap();
        let manifest = if recovered {
            drop(journal);
            CaptureReader::open(&intent.journal_path)
                .unwrap()
                .recover_prefix("synthetic-recovered", &complete.prefix)
                .unwrap()
                .0
        } else {
            journal
                .seal_complete_prefix("synthetic-storage-only", &complete)
                .unwrap()
        };
        intent.descriptor_sha256 = Some(descriptor.sha256().unwrap());
        intent.finish = Some(PerformanceFinishIntent {
            id: fence.id,
            scope: fence.scope,
        });
        intent.attachment = Some(publication(intent, manifest.sha256, 128));
    }
    fn publication(intent: &PerformanceIntent, manifest: String, frames: u64) -> TakePublication {
        TakePublication {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            project_id: intent.project_id.clone(),
            expected_revision: 0,
            track_id: TrackId(uuid::Uuid::new_v4().to_string()),
            track_name: "Synthetic persisted performance audio".into(),
            clip: TakeClip {
                id: uuid::Uuid::new_v4().to_string(),
                source: TakeReference {
                    take_id: TakeId(intent.start.take_id()),
                    manifest_sha256: manifest,
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 0,
                    count: frames,
                },
                position: TimelinePosition {
                    frame: 0,
                    sample_rate: 48_000,
                },
                fades: None,
                edit_source: None,
            },
        }
    }
    #[test]
    fn typed_intention_cas_restart_and_immutable_fields_preserve_existing_project() {
        let f = Fixture::new();
        let mut backend = f.backend();
        let mut intent = initial(&mut backend);
        let before = backend
            .store()
            .unwrap()
            .load(&intent.project_id.0)
            .unwrap()
            .to_json()
            .unwrap();
        let one = save(&mut backend, None, &intent).unwrap();
        assert_eq!(one.revision, 1);
        assert_eq!(save(&mut backend, Some(999), &intent).unwrap(), one);
        let mut changed = intent.clone();
        changed.descriptor_sha256 = Some("a".repeat(64));
        let error = save(&mut backend, Some(2), &changed).unwrap_err();
        assert_eq!(error.kind, FailureKind::RevisionConflict);
        assert!(!error.outcome_unknown);
        intent = changed;
        let two = save(&mut backend, Some(1), &intent).unwrap();
        assert_eq!(two.revision, 2);
        for case in 0..5 {
            let mut invalid = intent.clone();
            match case {
                0 => invalid.start.gui_clock.epoch = "other".into(),
                1 => invalid.journal_path = PathBuf::from("/wrong/owned/stage.sqlite3"),
                2 => invalid.project_id = ProjectId("another-project".into()),
                3 => invalid.descriptor_sha256 = None,
                _ => invalid.descriptor_sha256 = Some("b".repeat(64)),
            };
            let error = save(&mut backend, Some(2), &invalid).unwrap_err();
            assert_eq!(error.kind, FailureKind::IntentRejected);
            assert!(!error.outcome_unknown);
        }
        assert_eq!(listed(&mut backend).unwrap(), vec![two.clone()]);
        let paths = backend.paths.clone();
        drop(backend);
        let mut reopened = NativeBackend {
            paths,
            store: None,
            legacy: LegacyScan::default(),
            legacy_notice: None,
            require_existing: true,
        };
        reopened.handle(&Command::Initialize).unwrap();
        assert_eq!(listed(&mut reopened).unwrap(), vec![two]);
        assert_eq!(
            reopened
                .store()
                .unwrap()
                .load(&intent.project_id.0)
                .unwrap()
                .to_json()
                .unwrap(),
            before
        );
    }
    #[test]
    fn released_intent_cannot_gain_fields_or_reopen_and_plain_listing_has_no_side_effects() {
        let f = Fixture::new();
        let mut backend = f.backend();
        let mut intent = initial(&mut backend);
        save(&mut backend, None, &intent).unwrap();
        intent.released = true;
        let receipt = save(&mut backend, Some(1), &intent).unwrap();
        let before = backend.store().unwrap().list().unwrap();
        assert_eq!(listed(&mut backend).unwrap(), vec![receipt]);
        assert_eq!(backend.store().unwrap().list().unwrap(), before);
        assert!(!intent.journal_path.exists());
        for descriptor in [false, true] {
            let mut changed = intent.clone();
            if descriptor {
                changed.descriptor_sha256 = Some("a".repeat(64));
            } else {
                changed.released = false;
            }
            let error = save(&mut backend, Some(2), &changed).unwrap_err();
            assert_eq!(error.kind, FailureKind::IntentRejected);
            assert!(!error.outcome_unknown);
        }
    }
    #[test]
    fn attach_executes_only_persisted_choice_and_exact_replay_returns_current_document_after_stage_loss()
     {
        let f = Fixture::new();
        let mut backend = f.backend();
        let mut intent = initial(&mut backend);
        save(&mut backend, None, &intent).unwrap();
        normal_source(&f, &mut intent, false);
        let two = save(&mut backend, Some(1), &intent).unwrap();
        let mut premature = intent.clone();
        premature.released = true;
        let error = save(&mut backend, Some(2), &premature).unwrap_err();
        assert_eq!(error.kind, FailureKind::IntentRejected);
        assert!(!error.outcome_unknown);
        assert_eq!(
            backend
                .store()
                .unwrap()
                .load(&intent.project_id.0)
                .unwrap()
                .revision(),
            0
        );
        let command = Command::AttachPerformance {
            id: intent.start.id,
            revision: two.revision,
        };
        let Reply::Document { view, replayed, .. } = backend.handle(&command).unwrap() else {
            panic!("document")
        };
        assert!(!replayed);
        assert_eq!(view.document.revision(), 1);
        assert_eq!(
            view.document.take_clips().unwrap(),
            vec![intent.attachment.as_ref().unwrap().clip.clone()]
        );
        fs::remove_file(&intent.journal_path).unwrap();
        backend
            .handle(&Command::Edit {
                intent: ProjectEditIntent {
                    operation_id: OperationId("later-metadata".into()),
                    project_id: intent.project_id.clone(),
                    expected_revision: 1,
                    scope: EditScope::Project,
                    edit: EditOperation::Rename {
                        name: "Later current document".into(),
                    },
                    extensions: Default::default(),
                },
            })
            .unwrap();
        let Reply::Document { view, replayed, .. } = backend.handle(&command).unwrap() else {
            panic!("document")
        };
        assert!(replayed);
        assert_eq!(view.document.revision(), 2);
        assert_eq!(view.document.name(), "Later current document");
        intent.released = true;
        let released = save(&mut backend, Some(2), &intent).unwrap();
        assert_eq!(released.revision, 3);
        assert!(released.intent.released);
        let error = backend
            .handle(&Command::AttachPerformance {
                id: intent.start.id,
                revision: 3,
            })
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::IntentRejected);
        assert!(!error.outcome_unknown);
    }
    #[test]
    fn staged_wal_headers_and_sidecars_are_rejected_before_sqlite_can_open_them() {
        for case in 0..3 {
            let f = Fixture::new();
            let mut backend = f.backend();
            let mut intent = initial(&mut backend);
            normal_source(&f, &mut intent, false);
            let saved = save(&mut backend, None, &intent).unwrap();
            let path = intent.journal_path.clone();
            if case == 0 {
                let mut bytes = fs::read(&path).unwrap();
                bytes[18] = 2;
                bytes[19] = 2;
                fs::write(&path, bytes).unwrap();
            } else {
                let mut sidecar = path.as_os_str().to_owned();
                sidecar.push(if case == 1 { "-wal" } else { "-shm" });
                fs::write(PathBuf::from(sidecar), b"ambiguous retained evidence").unwrap();
            }
            let before = fs::read(&path).unwrap();
            let inventory = || {
                let mut entries: Vec<_> = fs::read_dir(path.parent().unwrap())
                    .unwrap()
                    .map(|entry| {
                        let entry = entry.unwrap();
                        (entry.file_name(), fs::read(entry.path()).unwrap())
                    })
                    .collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                entries
            };
            let files = inventory();
            let error = backend
                .handle(&Command::AttachPerformance {
                    id: intent.start.id,
                    revision: saved.revision,
                })
                .unwrap_err();
            assert_eq!(error.kind, FailureKind::IntentRejected);
            assert!(!error.outcome_unknown);
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(
                inventory(),
                files,
                "rejection must not create or alter any source sidecar"
            );
            assert!(
                !backend
                    .store()
                    .unwrap()
                    .has_registered_take(&TakeId(intent.start.take_id()))
                    .unwrap()
            );
            assert_eq!(
                backend
                    .store()
                    .unwrap()
                    .load(&intent.project_id.0)
                    .unwrap()
                    .revision(),
                0
            );
        }
    }

    #[test]
    fn invalid_or_recovered_sources_are_not_ingested_or_published_as_normal_performances() {
        for case in 0..4 {
            let f = Fixture::new();
            let mut backend = f.backend();
            let mut intent = initial(&mut backend);
            normal_source(&f, &mut intent, case == 3);
            match case {
                0 => intent.descriptor_sha256 = Some("b".repeat(64)),
                1 => intent.start.gui_clock.epoch = "different-original-clock".into(),
                2 => intent.finish.as_mut().unwrap().id = LiveId([9; 16]),
                _ => (),
            };
            let receipt = save(&mut backend, None, &intent).unwrap();
            let original = fs::read(&intent.journal_path).unwrap();
            let error = backend
                .handle(&Command::AttachPerformance {
                    id: intent.start.id,
                    revision: receipt.revision,
                })
                .unwrap_err();
            assert_eq!(
                error.kind,
                FailureKind::IntentRejected,
                "case {case}: {error}"
            );
            assert!(!error.outcome_unknown);
            assert_eq!(
                backend
                    .store()
                    .unwrap()
                    .load(&intent.project_id.0)
                    .unwrap()
                    .revision(),
                0
            );
            assert!(
                !backend
                    .store()
                    .unwrap()
                    .has_registered_take(&TakeId(intent.start.take_id()))
                    .unwrap()
            );
            assert_eq!(fs::read(&intent.journal_path).unwrap(), original);
        }
    }
    #[test]
    fn generic_v1_and_semantically_incomplete_v2_named_performance_sources_are_rejected() {
        for case in 0..3 {
            let f = Fixture::new();
            let mut backend = f.backend();
            let mut intent = initial(&mut backend);
            let profile = if case == 1 {
                PerformanceProfile::OriginalV1
            } else {
                PerformanceProfile::FinishV2
            };
            let config = config(&intent);
            let mut descriptor = create_descriptor_for(&config, profile).unwrap();
            if case == 0 {
                descriptor.provenance.clear();
            }
            let stream = &descriptor.streams[0];
            let stamp = HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: 0,
            };
            let header = ChunkHeader {
                version: CHUNK_VERSION,
                take_id: descriptor.take_id.clone(),
                stream_id: stream.stream_id.clone(),
                segment_id: stream.segment_id.clone(),
                sequence: 0,
                range: SourceRange::AudioFrames { start: 0, count: 2 },
                interpretation: stream.kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            };
            let mut journal = Journal::create(&intent.journal_path, &descriptor).unwrap();
            journal
                .append(
                    &header,
                    &[0.25f32.to_le_bytes(), 0.5f32.to_le_bytes()].concat(),
                )
                .unwrap();
            let manifest = if case == 0 {
                journal.seal("generic-fixture").unwrap()
            } else if case == 2 {
                let complete = journal.complete_prefix().unwrap();
                journal
                    .seal_complete_prefix("incomplete-v2-storage", &complete)
                    .unwrap()
            } else {
                drop(journal);
                let reader = CaptureReader::open(&intent.journal_path).unwrap();
                let report = &reader.inspection().report;
                let prefix = omniusong_recording::RecoveryPrefix {
                    chunk_count: report.verified_chunks,
                    chunk_index_sha256: report.chunk_index_sha256.clone(),
                };
                reader.recover_prefix("v1-recovered", &prefix).unwrap().0
            };
            intent.descriptor_sha256 = Some(descriptor.sha256().unwrap());
            intent.finish = Some(PerformanceFinishIntent {
                id: LiveId([8; 16]),
                scope: config.info.scope,
            });
            intent.attachment = Some(publication(&intent, manifest.sha256, 2));
            let receipt = save(&mut backend, None, &intent).unwrap();
            let error = backend
                .handle(&Command::AttachPerformance {
                    id: intent.start.id,
                    revision: receipt.revision,
                })
                .unwrap_err();
            assert_eq!(error.kind, FailureKind::IntentRejected, "case {case}");
            assert!(!error.outcome_unknown);
            assert_eq!(
                backend
                    .store()
                    .unwrap()
                    .load(&intent.project_id.0)
                    .unwrap()
                    .revision(),
                0
            );
        }
    }
    #[test]
    fn stored_hash_valid_but_typed_invalid_history_fails_all_listing_and_new_save_without_hiding_it()
     {
        let f = Fixture::new();
        let mut backend = f.backend();
        let intent = initial(&mut backend);
        let first = save(&mut backend, None, &intent).unwrap();
        let before = backend
            .store()
            .unwrap()
            .load(&intent.project_id.0)
            .unwrap()
            .to_json()
            .unwrap();
        let mut changed = intent.clone();
        changed.start.gui_clock.epoch = "rebound by opaque test writer".into();
        let bytes = serde_json::to_vec(&changed).unwrap();
        backend
            .store()
            .unwrap()
            .save_performance_intent(&intent.start.directory_name(), Some(1), &bytes)
            .unwrap();
        let error = backend
            .handle(&Command::ListPerformanceIntents)
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::Backend);
        assert!(!error.outcome_unknown);
        let error = save(&mut backend, Some(2), &changed).unwrap_err();
        assert_eq!(error.kind, FailureKind::Backend);
        assert!(error.outcome_unknown);
        assert_eq!(
            backend
                .store()
                .unwrap()
                .list_performance_intents()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            backend
                .store()
                .unwrap()
                .load(&intent.project_id.0)
                .unwrap()
                .to_json()
                .unwrap(),
            before
        );
        assert_eq!(first.revision, 1);
    }
    #[test]
    fn command_contracts_are_strict_bounded_correlated_and_do_not_start_an_engine() {
        let f = Fixture::new();
        let mut backend = f.backend();
        let intent = initial(&mut backend);
        let save = Command::SavePerformanceIntent {
            expected_revision: None,
            intent: Box::new(intent.clone()),
        };
        assert_eq!(save.kind(), crate::CommandKind::SavePerformanceIntent);
        assert!(save.retry_safe() && save.mutates());
        assert!(!Command::ListPerformanceIntents.mutates());
        assert!(Command::ListPerformanceIntents.retry_safe());
        let mut raw = serde_json::to_value(&intent).unwrap();
        raw["unknown_field"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<PerformanceIntent>(raw).is_err());
        assert!(
            Command::AttachPerformance {
                id: intent.start.id,
                revision: 0
            }
            .validate()
            .is_err()
        );
        let Reply::PerformanceIntent(receipt) = backend.handle(&save).unwrap() else {
            panic!("receipt")
        };
        assert!(
            Reply::PerformanceIntent(receipt).matches(crate::CommandKind::SavePerformanceIntent)
        );
        assert!(!Reply::PerformanceIntents(vec![]).matches(crate::CommandKind::AttachPerformance));
        assert!(!intent.journal_path.exists());
    }
    #[test]
    fn reserved_publication_new_intent_refuses_but_historical_exact_metadata_remains_readable() {
        let fixture = Fixture::new();
        let mut backend = fixture.backend();
        let mut intent = initial(&mut backend);
        intent.descriptor_sha256 = Some("b".repeat(64));
        intent.attachment = Some(TakePublication {
            operation_id: OperationId(format!("os-src-attach:{}", "a".repeat(64))),
            project_id: intent.project_id.clone(),
            expected_revision: 0,
            track_id: TrackId("legacy-recording".into()),
            track_name: "Legacy metadata".into(),
            clip: TakeClip {
                id: "legacy-clip".into(),
                source: TakeReference {
                    take_id: TakeId(intent.start.take_id()),
                    manifest_sha256: "c".repeat(64),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames {
                    start: 0,
                    count: 64,
                },
                position: TimelinePosition {
                    frame: 0,
                    sample_rate: 48000,
                },
                fades: None,
                edit_source: None,
            },
        });
        assert!(backend.save_performance_intention(None, &intent).is_err());
        assert!(
            backend
                .store()
                .unwrap()
                .list_performance_intents()
                .unwrap()
                .is_empty()
        );
        let bytes = encode(&intent).unwrap();
        let saved = backend
            .store()
            .unwrap()
            .save_performance_intent(&intent.start.directory_name(), None, &bytes)
            .unwrap();
        let before = backend.store().unwrap().list_performance_intents().unwrap();
        assert_eq!(
            backend.list_performance_intentions().unwrap()[0].intent,
            intent
        );
        let Reply::PerformanceIntent(replayed) =
            backend.save_performance_intention(None, &intent).unwrap()
        else {
            panic!("historical intent replay")
        };
        assert_eq!(replayed.payload_sha256, saved.payload_sha256);
        assert_eq!(replayed.intent, intent);
        assert_eq!(
            backend.store().unwrap().list_performance_intents().unwrap(),
            before
        );
    }
}

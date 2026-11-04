//! Explicit bounded raw exports over synthetic recovered copies. No devices,
//! durable session authorization, publication or evidence-archive claims.
#![cfg(unix)]
use omniusong_recording::*;
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const ACTION: &str = "44444444444444444444444444444444";
const FRAMES: u64 = 16_384;

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    actions: PathBuf,
    image_sha: String,
    image_bytes: u64,
}
impl Fixture {
    fn new(bad_tail: bool) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/recovered-export-tests")
            .join(uuid::Uuid::new_v4().to_string());
        for name in ["source", "actions", "exports"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let root = fs::canonicalize(root).unwrap();
        let source = root.join("source/take.sqlite3");
        let descriptor = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "synthetic-export".into(),
                source_id: "synthetic-pcm".into(),
                source_name: "Synthetic export; no device".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("fixture-clock".into()),
                    epoch: ClockEpochId("fixture-epoch".into()),
                    ticks_per_second_numerator: 48_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("fixture-audio".into()),
                },
                initial_source_position: 0,
            }],
            provenance: BTreeMap::from([(
                "opaque-evidence".into(),
                serde_json::json!({"text":"retained\u{0}metadata","unknown":[1,2,3]}),
            )]),
        };
        let mut journal = Journal::create(&source, &descriptor).unwrap();
        for sequence in 0..3 {
            let stream = &descriptor.streams[0];
            let timestamp = HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: sequence * FRAMES,
            };
            let header = ChunkHeader {
                version: CHUNK_VERSION,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: stream.segment_id.clone(),
                sequence,
                range: SourceRange::AudioFrames {
                    start: sequence * FRAMES,
                    count: FRAMES,
                },
                interpretation: stream.kind.clone(),
                observed_start: timestamp.clone(),
                observed_end: timestamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            };
            journal
                .append(&header, &0.25f32.to_le_bytes().repeat(FRAMES as usize))
                .unwrap();
        }
        drop(journal);
        if bad_tail {
            let connection = Connection::open(&source).unwrap();
            let trigger: String = connection
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='immutable_chunk_update'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            connection.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=3;").unwrap();
            connection.execute_batch(&trigger).unwrap();
        }
        let bytes = fs::read(&source).unwrap();
        let image_sha = digest(&bytes);
        let image_bytes = bytes.len() as u64;
        let actions = root.join("actions");
        let owner = CopyActionOwner::create(&actions, ACTION).unwrap();
        let (budget, _) = governor(InspectionLimits::default());
        let reader = CaptureReader::open_bounded(&source, &budget).unwrap();
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.recording-index.v1\0");
        hash.update(reader.inspection().descriptor_sha256.as_bytes());
        for ordinal in 1u64..=2 {
            hash.update(ordinal.to_le_bytes());
            hash.update(reader.read_ordinal(ordinal).unwrap().sha256.as_bytes());
        }
        let selection = RecoveryPrefix {
            chunk_count: 2,
            chunk_index_sha256: format!("{:x}", hash.finalize()),
        };
        RecoveryCopy::create(reader, &actions, ACTION)
            .unwrap()
            .finalize(&selection, "synthetic-recovered-export")
            .unwrap();
        owner.verify().unwrap();
        drop(owner);
        Self {
            root,
            source,
            actions,
            image_sha,
            image_bytes,
        }
    }
    fn destination(&self) -> PathBuf {
        self.root.join("exports/export.sqlite3")
    }
    fn owner(&self) -> CopyActionOwner {
        CopyActionOwner::open_existing(&self.actions, ACTION).unwrap()
    }
    fn view<'a>(
        &self,
        owner: &'a CopyActionOwner,
        budget: &InspectionBudget,
    ) -> RecoveredCopyView<'a> {
        RecoveredCopyView::open(owner, &self.image_sha, self.image_bytes, budget).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Export fixture retained at {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn governor(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Directory(u64, u64, u32),
    File(u64, u64, u32, Vec<u8>),
    Link(PathBuf),
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let value = if metadata.file_type().is_symlink() {
                Entry::Link(fs::read_link(&path).unwrap())
            } else if metadata.is_dir() {
                visit(root, &path, out);
                Entry::Directory(metadata.dev(), metadata.ino(), metadata.mode())
            } else {
                Entry::File(
                    metadata.dev(),
                    metadata.ino(),
                    metadata.mode(),
                    fs::read(&path).unwrap(),
                )
            };
            out.insert(path.strip_prefix(root).unwrap().to_owned(), value);
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
#[derive(Debug, PartialEq, Eq)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
fn all_rows(path: &Path) -> BTreeMap<String, Vec<Vec<Cell>>> {
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!sidecar(path, suffix).exists());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    connection
        .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;")
        .unwrap();
    let mut result = BTreeMap::new();
    for table in [
        "sqlite_schema",
        "take_meta",
        "stream_prefix",
        "chunks",
        "interruptions",
        "recovery_index",
        "sealed_manifests",
        "sqlite_sequence",
    ] {
        let mut statement = connection
            .prepare(&format!("SELECT rowid,* FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| {
                        Ok(match row.get_ref(column)? {
                            ValueRef::Null => Cell::Null,
                            ValueRef::Integer(n) => Cell::Integer(n),
                            ValueRef::Real(n) => Cell::Real(n.to_bits()),
                            ValueRef::Text(bytes) => Cell::Text(bytes.to_vec()),
                            ValueRef::Blob(bytes) => Cell::Blob(bytes.to_vec()),
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        result.insert(table.into(), rows);
    }
    result
}

#[test]
fn full_working_bytes_rows_manifest_and_unknown_tail_export_under_the_held_owner() {
    for bad_tail in [false, true] {
        let fixture = Fixture::new(bad_tail);
        let owner = fixture.owner();
        let (budget, _) = governor(InspectionLimits::default());
        let view = fixture.view(&owner, &budget);
        let originals = tree(fixture.source.parent().unwrap());
        let artifacts = tree(&fixture.actions);
        let working = fs::read(&owner.paths().working).unwrap();
        let rows = all_rows(&owner.paths().working);
        let before = budget.usage();
        let destination = fixture.destination();
        let receipt = view.export_working_copy(&destination).unwrap();
        assert_eq!(receipt.path, destination);
        assert_eq!(receipt.sha256, digest(&working));
        assert_eq!(receipt.byte_len, working.len() as u64);
        assert_eq!(fs::read(&destination).unwrap(), working);
        assert_eq!(all_rows(&destination), rows);
        let exported = SealedJournal::open(&destination).unwrap();
        assert_eq!(
            exported.manifest(),
            view.working_reader()
                .inspection()
                .manifest
                .as_ref()
                .unwrap()
        );
        assert!(exported.manifest().body.interrupted.is_some());
        assert_eq!(exported.manifest().body.chunk_count, 2);
        assert_eq!(receipt.manifest_sha256, exported.manifest().sha256);
        assert_eq!(&receipt.take_id, &exported.descriptor().take_id);
        assert!(exported.inspect_snapshot().unwrap().report.unknown_tail);
        assert!(!sidecar(&destination, ".owner.lock").exists());
        assert_eq!(tree(fixture.source.parent().unwrap()), originals);
        assert_eq!(tree(&fixture.actions), artifacts);
        assert!(budget.usage().work_bytes >= before.work_bytes + working.len() as u64 * 5);
        let snapshot = tree(&fixture.root);
        assert!(view.export_working_copy(&destination).is_err());
        assert_eq!(tree(&fixture.root), snapshot);
        // Ordinary bounded sealed inspection retains its separate export refusal.
        let (other_budget, _) = governor(InspectionLimits::default());
        let bounded = SealedJournal::open_bounded(&destination, &other_budget).unwrap();
        assert!(
            bounded
                .export_backup(fixture.root.join("exports/forbidden.sqlite3"))
                .is_err()
        );
        assert!(!fixture.root.join("exports/forbidden.sqlite3").exists());
    }
}

#[test]
fn existing_destinations_aliases_noncanonical_parents_and_sidecars_are_unchanged() {
    for case in 0..10 {
        let fixture = Fixture::new(false);
        let owner = fixture.owner();
        let (budget, _) = governor(InspectionLimits::default());
        let view = fixture.view(&owner, &budget);
        let mut destination = fixture.destination();
        match case {
            0 => fs::write(&destination, b"existing bytes").unwrap(),
            1 => symlink(&owner.paths().working, &destination).unwrap(),
            2 => fs::hard_link(&owner.paths().working, &destination).unwrap(),
            3 => {
                let alias = fixture.root.join("alias");
                symlink(fixture.root.join("exports"), &alias).unwrap();
                destination = alias.join("export.sqlite3");
            }
            4..=7 => fs::write(
                sidecar(
                    &destination,
                    ["-wal", "-shm", "-journal", ".owner.lock"][case - 4],
                ),
                b"retained sidecar",
            )
            .unwrap(),
            8 => destination = owner.paths().action.join("unexpected-export.sqlite3"),
            9 => destination = fixture.root.join("missing/export.sqlite3"),
            _ => unreachable!(),
        }
        let before = tree(&fixture.root);
        assert!(
            view.export_working_copy(&destination).is_err(),
            "case {case}"
        );
        assert_eq!(tree(&fixture.root), before, "case {case}");
    }
}

#[test]
fn newly_hot_evidence_or_working_copy_refuses_without_creating_a_destination() {
    for evidence in [false, true] {
        for suffix in ["-wal", "-shm", "-journal"] {
            let fixture = Fixture::new(true);
            let owner = fixture.owner();
            let (budget, _) = governor(InspectionLimits::default());
            let view = fixture.view(&owner, &budget);
            let path = if evidence {
                &owner.paths().evidence
            } else {
                &owner.paths().working
            };
            fs::write(sidecar(path, suffix), b"retained hot sidecar").unwrap();
            let before = tree(&fixture.root);
            assert!(view.export_working_copy(fixture.destination()).is_err());
            assert!(!fixture.destination().exists());
            assert_eq!(tree(&fixture.root), before);
        }
    }
}

#[test]
fn cancellation_is_sticky_before_destination_creation() {
    let fixture = Fixture::new(false);
    let owner = fixture.owner();
    let (budget, cancelled) = governor(InspectionLimits::default());
    let view = fixture.view(&owner, &budget);
    let before = tree(&fixture.root);
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        view.export_working_copy(fixture.destination()),
        Err(RecordingError::InspectionCancelled)
    ));
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        view.export_working_copy(fixture.destination()),
        Err(RecordingError::InspectionCancelled)
    ));
    assert_eq!(tree(&fixture.root), before);
}

#[test]
fn cumulative_work_limit_retains_exact_partial_output_and_never_restarts_it() {
    let fixture = Fixture::new(true);
    let owner = fixture.owner();
    let (calibration, _) = governor(InspectionLimits::default());
    let view = fixture.view(&owner, &calibration);
    let open_work = calibration.usage().work_bytes;
    view.verify_unchanged().unwrap();
    let preflight_work = calibration.usage().work_bytes - open_work;
    drop(view);
    // One 64-KiB read/write/hash succeeds. The following read must stop before
    // touching output, using the same governor that opened both held snapshots.
    let (budget, _) = governor(InspectionLimits {
        max_work_bytes: open_work + preflight_work + 3 * 65_536 + 1,
        ..Default::default()
    });
    let view = fixture.view(&owner, &budget);
    let source = tree(fixture.source.parent().unwrap());
    let actions = tree(&fixture.actions);
    let destination = fixture.destination();
    assert!(matches!(
        view.export_working_copy(&destination),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        })
    ));
    let partial = fs::read(&destination).unwrap();
    assert_eq!(partial.len(), 65_536);
    assert_eq!(partial, fs::read(&owner.paths().working).unwrap()[..65_536]);
    assert!(matches!(
        view.export_working_copy(&destination),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        })
    ));
    assert_eq!(fs::read(&destination).unwrap(), partial);
    assert_eq!(tree(fixture.source.parent().unwrap()), source);
    assert_eq!(tree(&fixture.actions), actions);
    assert!(!sidecar(&destination, ".owner.lock").exists());
}

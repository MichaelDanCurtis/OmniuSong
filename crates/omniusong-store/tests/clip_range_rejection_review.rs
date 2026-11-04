//! Independent admission review: user range mistakes versus damaged retained sources.
//! All journals and catalogs are fresh synthetic fixtures; no devices are accessed.
use omniusong_core::{
    AudioSourceWindow, ClipPatch, ClipTimelinePolicy, DocumentClipAddress, FieldChange,
    NativeAudioClipPatch, ProjectDocument, ValueChange,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, StoreError};
use rusqlite::{Connection, params, types::Value as SqlValue};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf};

const KEEP: ClipTimelinePolicy = ClipTimelinePolicy::PreserveLoop;
const TRACK: &str = "range-track";
const CLIP: &str = "range-clip";

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    object: PathBuf,
}

impl Fixture {
    fn create() -> (Self, NativeStore, ProjectDocument) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/clip-range-rejection-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let source = root.join("synthetic-original.sqlite3");
        let descriptor = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId("range-original".into()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                source_id: "synthetic-only".into(),
                source_name: "Independent frame bounds fixture".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("host".into()),
                    epoch: ClockEpochId("boot".into()),
                    ticks_per_second_numerator: 1_000_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 1,
                        encoding: PcmEncoding::S16Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("audio".into()),
                },
                initial_source_position: 100,
            }],
            provenance: BTreeMap::from([("fixture".into(), json!("no device input"))]),
        };
        let timestamp = HostTimestamp {
            domain: ClockDomainId("host".into()),
            epoch: ClockEpochId("boot".into()),
            ticks: 1_000,
        };
        let header = ChunkHeader {
            version: CHUNK_VERSION,
            take_id: descriptor.take_id.clone(),
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            sequence: 0,
            range: SourceRange::AudioFrames {
                start: 100,
                count: 8,
            },
            interpretation: descriptor.streams[0].kind.clone(),
            observed_start: timestamp.clone(),
            observed_end: timestamp,
            clock_anchors: vec![],
            kind: ChunkKind::Content,
        };
        let mut journal = Journal::create(&source, &descriptor).unwrap();
        journal
            .append(&header, &[1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0])
            .unwrap();
        journal.seal("seal-synthetic-original").unwrap();
        drop(journal);
        let sealed = SealedJournal::open(&source).unwrap();
        let mut store = NativeStore::open(root.join("store")).unwrap();
        let registered = store.ingest_take(&sealed).unwrap();
        let object = root
            .join("store/takes")
            .join(format!("{}.sqlite3", registered.object_sha256));
        let mut document = ProjectDocument::new("Verified range review")
            .unwrap()
            .upgrade_native()
            .unwrap();
        document
            .append_take_clip(
                TRACK,
                "Synthetic audio",
                TakeClip {
                    id: CLIP.into(),
                    source: TakeReference {
                        take_id: descriptor.take_id,
                        manifest_sha256: sealed.manifest().sha256.clone(),
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: 100,
                        count: 8,
                    },
                    position: TimelinePosition {
                        frame: -4,
                        sample_rate: 48_000,
                    },
                    fades: None,
                    edit_source: None,
                },
            )
            .unwrap();
        let document = store.create(&document).unwrap();
        (
            Self {
                root,
                source,
                object,
            },
            store,
            document,
        )
    }

    fn db(&self) -> Connection {
        Connection::open(self.root.join("store/studio.sqlite3")).unwrap()
    }

    fn rows(&self) -> Vec<Vec<Vec<SqlValue>>> {
        let db = self.db();
        [
            "SELECT * FROM projects ORDER BY id",
            "SELECT * FROM edits ORDER BY project_id,sequence",
            "SELECT * FROM operations ORDER BY operation_id",
            "SELECT * FROM native_takes ORDER BY take_id",
        ]
        .into_iter()
        .map(|sql| {
            let mut statement = db.prepare(sql).unwrap();
            let columns = statement.column_count();
            statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get::<_, SqlValue>(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
        .collect()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            rows: self.rows(),
            source: fs::read(&self.source).unwrap(),
            object: fs::read(&self.object).unwrap(),
        }
    }

    fn replace_current(&self, document: &ProjectDocument) {
        let payload = String::from_utf8(document.to_json().unwrap()).unwrap();
        assert_eq!(
            self.db()
                .execute(
                    "UPDATE projects SET payload=?1 WHERE id=?2",
                    params![payload, document.id()],
                )
                .unwrap(),
            1
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Range rejection fixture retained at {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    rows: Vec<Vec<Vec<SqlValue>>>,
    source: Vec<u8>,
    object: Vec<u8>,
}

fn address() -> DocumentClipAddress {
    DocumentClipAddress::NativeAudio {
        track_id: TRACK.into(),
        clip_id: CLIP.into(),
    }
}

fn request(document: &ProjectDocument, id: &str, edit: EditOperation) -> ProjectEditIntent {
    let scope = if matches!(edit, EditOperation::Rename { .. }) {
        EditScope::Project
    } else {
        EditScope::DocumentClip { address: address() }
    };
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope,
        edit,
        extensions: BTreeMap::new(),
    }
}

fn range_request(
    document: &ProjectDocument,
    id: &str,
    start_frame: u64,
    frame_count: u64,
) -> ProjectEditIntent {
    request(
        document,
        id,
        EditOperation::UpdateClip {
            patch: ClipPatch::NativeAudio(NativeAudioClipPatch {
                position_frame: ValueChange::Keep,
                source_window: ValueChange::Set {
                    value: AudioSourceWindow {
                        start_frame,
                        frame_count,
                    },
                },
                fades: FieldChange::Keep,
            }),
            timeline: KEEP,
        },
    )
}

fn position_request(document: &ProjectDocument, id: &str) -> ProjectEditIntent {
    request(
        document,
        id,
        EditOperation::UpdateClip {
            patch: ClipPatch::NativeAudio(NativeAudioClipPatch {
                position_frame: ValueChange::Set { value: -9 },
                source_window: ValueChange::Keep,
                fades: FieldChange::Keep,
            }),
            timeline: KEEP,
        },
    )
}

fn selected_mut(value: &mut Value) -> &mut Value {
    &mut value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|track| track["id"] == TRACK)
        .unwrap()["recording"]["clips"][0]
}

fn assert_range_rejected(error: StoreError) {
    assert!(matches!(error, StoreError::EditRejected(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "Requested source window must stay within verified audio frames 100–108"
    );
}

fn assert_conservative(error: StoreError) {
    assert!(
        !matches!(error, StoreError::EditRejected(_) | StoreError::Conflict),
        "Source/integrity failure must not become an editable rejection: {error:?}"
    );
}

#[test]
fn lower_and_upper_overruns_are_known_rejections_without_any_published_change() {
    let (fixture, mut store, document) = Fixture::create();
    let before = fixture.snapshot();
    for (index, start, count) in [(0, 99, 4), (1, 107, 2), (2, 108, 1)] {
        let intent = range_request(&document, &format!("outside-{index}"), start, count);
        assert_range_rejected(store.apply_edit(&intent).unwrap_err());
        assert_eq!(fixture.snapshot(), before);
        assert_eq!(store.load(document.id()).unwrap(), document);
    }
}

#[test]
fn exact_rejected_retry_and_fresh_corrected_choice_keep_the_original() {
    let (fixture, mut store, document) = Fixture::create();
    let before = fixture.snapshot();
    let rejected = range_request(&document, "bad-window", 107, 2);
    for _ in 0..2 {
        assert_range_rejected(store.apply_edit(&rejected).unwrap_err());
        assert_eq!(fixture.snapshot(), before);
    }
    let corrected = range_request(&document, "corrected-window", 101, 6);
    let receipt = store.apply_edit(&corrected).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.document.revision(), document.revision() + 1);
    let clip = receipt.document.take_clips().unwrap().remove(0);
    assert_eq!(
        clip.range,
        TakeRange::AudioFrames {
            start: 101,
            count: 6
        }
    );
    assert_eq!(clip.position.frame, -4);
    assert_eq!(
        clip.original_view().unwrap().unwrap().range,
        TakeRange::AudioFrames {
            start: 100,
            count: 8
        }
    );
    let count: i64 = fixture
        .db()
        .query_row(
            "SELECT count(*) FROM operations WHERE operation_id='bad-window'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(fs::read(&fixture.source).unwrap(), before.source);
    assert_eq!(fs::read(&fixture.object).unwrap(), before.object);
}

#[test]
fn exact_authenticated_source_boundaries_are_allowed_and_restore_is_unchanged() {
    let (fixture, mut store, document) = Fixture::create();
    let before = fixture.snapshot();
    let trimmed = store
        .apply_edit(&range_request(&document, "inside", 101, 6))
        .unwrap()
        .document;
    let full = store
        .apply_edit(&range_request(&trimmed, "full-boundaries", 100, 8))
        .unwrap()
        .document;
    assert_eq!(
        full.take_clips().unwrap()[0].range,
        TakeRange::AudioFrames {
            start: 100,
            count: 8
        }
    );
    let moved = store
        .apply_edit(&position_request(&full, "move-only"))
        .unwrap()
        .document;
    let restored = store
        .apply_edit(&request(
            &moved,
            "restore-verified-original",
            EditOperation::RestoreClip { timeline: KEEP },
        ))
        .unwrap()
        .document;
    let clip = &restored.take_clips().unwrap()[0];
    assert_eq!(
        clip.range,
        TakeRange::AudioFrames {
            start: 100,
            count: 8
        }
    );
    assert_eq!(clip.position.frame, -4);
    assert!(clip.edit_source.is_some());
    assert_eq!(fs::read(&fixture.source).unwrap(), before.source);
    assert_eq!(fs::read(&fixture.object).unwrap(), before.object);
}

#[test]
fn replay_precedes_revision_and_new_range_admission_after_later_edits() {
    let (fixture, mut store, document) = Fixture::create();
    let intent = range_request(&document, "committed-window", 101, 4);
    let receipt = store.apply_edit(&intent).unwrap();
    let later = store
        .apply_edit(&request(
            &receipt.document,
            "later-name",
            EditOperation::Rename {
                name: "Later revision".into(),
            },
        ))
        .unwrap()
        .document;
    let before = fixture.snapshot();
    let replay = store.apply_edit(&intent).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, receipt.document);
    assert_eq!(store.load(document.id()).unwrap(), later);
    assert_eq!(fixture.snapshot(), before);

    let changed = range_request(&document, "committed-window", 99, 4);
    assert_conservative(store.apply_edit(&changed).unwrap_err());
    assert_eq!(fixture.snapshot(), before);
    let stale = range_request(&document, "fresh-but-stale", 99, 4);
    assert!(matches!(
        store.apply_edit(&stale),
        Err(StoreError::Conflict)
    ));
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn damaged_or_missing_owned_file_wins_over_a_user_range_error() {
    for missing in [false, true] {
        let (fixture, mut store, document) = Fixture::create();
        let original_bytes = fs::read(&fixture.source).unwrap();
        let retained = fixture.root.join("retained-owned-object.sqlite3");
        if missing {
            fs::rename(&fixture.object, &retained).unwrap();
        } else {
            let mut damaged = fs::read(&fixture.object).unwrap();
            *damaged.last_mut().unwrap() ^= 1;
            fs::write(&fixture.object, damaged).unwrap();
        }
        let observed_path = if missing { &retained } else { &fixture.object };
        let observed_bytes = fs::read(observed_path).unwrap();
        let before = fixture.rows();
        assert_conservative(
            store
                .apply_edit(&range_request(&document, "bad-source-and-window", 99, 4))
                .unwrap_err(),
        );
        assert_eq!(fixture.rows(), before);
        assert_eq!(fs::read(observed_path).unwrap(), observed_bytes);
        assert_eq!(fs::read(&fixture.source).unwrap(), original_bytes);
    }
}

#[test]
fn invalid_current_or_retained_original_cannot_be_repaired_by_edit_or_restore() {
    for original in [false, true] {
        for restore in [false, true] {
            let (fixture, mut store, document) = Fixture::create();
            let edited = store
                .apply_edit(&range_request(&document, "make-snapshot", 101, 4))
                .unwrap()
                .document;
            let mut invalid = edited.value().clone();
            let clip = selected_mut(&mut invalid);
            let settings = if original {
                &mut clip["editSource"]["original"]
            } else {
                clip
            };
            settings["range"] = json!({"unit":"audio_frames","start":99,"count":4});
            let invalid = ProjectDocument::from_value(invalid).unwrap();
            fixture.replace_current(&invalid);
            let before = fixture.snapshot();
            let intent = if restore {
                request(
                    &invalid,
                    "restore-invalid-source",
                    EditOperation::RestoreClip { timeline: KEEP },
                )
            } else {
                range_request(&invalid, "set-valid-window-on-invalid-current", 102, 4)
            };
            assert_conservative(store.apply_edit(&intent).unwrap_err());
            assert_eq!(fixture.snapshot(), before);
        }
    }
}

#[test]
fn current_reference_rate_and_unit_failures_never_become_range_rejections() {
    for fault in ["manifest", "stream", "take", "rate", "unit"] {
        let (fixture, mut store, document) = Fixture::create();
        let mut invalid = document.value().clone();
        let clip = selected_mut(&mut invalid);
        match fault {
            "manifest" => clip["source"]["manifestSha256"] = "f".repeat(64).into(),
            "stream" => clip["source"]["streamId"] = "absent-stream".into(),
            "take" => clip["source"]["takeId"] = "absent-take".into(),
            "rate" => clip["position"]["sampleRate"] = 44_100.into(),
            "unit" => clip["range"] = json!({"unit":"events","start":100,"count":8}),
            _ => unreachable!(),
        }
        let invalid = ProjectDocument::from_value(invalid).unwrap();
        fixture.replace_current(&invalid);
        let before = fixture.snapshot();
        assert_conservative(
            store
                .apply_edit(&range_request(
                    &invalid,
                    "invalid-reference-and-window",
                    99,
                    4,
                ))
                .unwrap_err(),
        );
        assert_eq!(fixture.snapshot(), before);
    }
}

#[test]
fn source_window_keep_does_not_reclassify_existing_source_damage() {
    let (fixture, mut store, document) = Fixture::create();
    let edited = store
        .apply_edit(&range_request(&document, "snapshot-before-damage", 101, 4))
        .unwrap()
        .document;
    let mut invalid = edited.value().clone();
    selected_mut(&mut invalid)["editSource"]["original"]["range"] =
        json!({"unit":"audio_frames","start":107,"count":2});
    let invalid = ProjectDocument::from_value(invalid).unwrap();
    fixture.replace_current(&invalid);
    let before = fixture.snapshot();
    assert_conservative(
        store
            .apply_edit(&position_request(&invalid, "keep-invalid-source"))
            .unwrap_err(),
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn valid_range_followed_by_final_sql_failure_is_conservative_and_atomic() {
    let (fixture, mut store, document) = Fixture::create();
    let db = fixture.db();
    db.execute_batch("CREATE TRIGGER fail_range_receipt BEFORE INSERT ON operations WHEN NEW.operation_id='sql-failure' BEGIN SELECT RAISE(ABORT,'Independent final receipt fault'); END;").unwrap();
    let before = fixture.snapshot();
    let intent = range_request(&document, "sql-failure", 101, 4);
    let error = store.apply_edit(&intent).unwrap_err();
    assert!(matches!(error, StoreError::Database(_)), "{error:?}");
    assert_eq!(fixture.snapshot(), before);
    db.execute_batch("DROP TRIGGER fail_range_receipt;")
        .unwrap();
    let receipt = store.apply_edit(&intent).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.document.revision(), document.revision() + 1);
    assert_eq!(fs::read(&fixture.source).unwrap(), before.source);
    assert_eq!(fs::read(&fixture.object).unwrap(), before.object);
}

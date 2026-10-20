//! Independent native take ingestion/publication regressions, all synthetic.
use omniusong_core::{
    ProjectDocument,
    contracts::{
        EditOperation, EditScope, Extensions, OperationId, ProjectEditIntent, ProjectId, TrackId,
    },
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, TakePublication};
use rusqlite::Connection;
use serde_json::json;
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/take-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn root(&self) -> PathBuf {
        self.0.join("store")
    }
    fn source(&self) -> PathBuf {
        self.0.join("source.sqlite3")
    }
    fn store(&self) -> NativeStore {
        NativeStore::open(self.root()).unwrap()
    }
    fn db(&self) -> Connection {
        Connection::open(self.root().join("studio.sqlite3")).unwrap()
    }
    fn object(&self) -> PathBuf {
        let paths: Vec<_> = fs::read_dir(self.root().join("takes"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sqlite3"))
            .collect();
        assert_eq!(paths.len(), 1);
        paths[0].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Take review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn descriptor() -> TakeDescriptor {
    let clock = HostClock {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("boot".into()),
        ticks_per_second_numerator: 24_000_000,
        ticks_per_second_denominator: 1,
    };
    let stream = |name: &str, initial_source_position, kind| StreamDescriptor {
        stream_id: name.into(),
        segment_id: format!("{name}-segment"),
        source_id: format!("{name}-input"),
        source_name: format!("Synthetic {name}"),
        source_connection_generation: 1,
        host_clock: clock.clone(),
        kind,
        initial_source_position,
    };
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("take-publication".into()),
        streams: vec![
            stream(
                "audio",
                100,
                StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        encoding: PcmEncoding::S16Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("audio-epoch".into()),
                },
            ),
            stream(
                "events",
                7,
                StreamKind::RawEvents {
                    format: RawEventFormat::LengthPrefixedCaptureV1,
                },
            ),
        ],
        provenance: BTreeMap::from([(
            "vendor".into(),
            json!({"assetId":"foreign-not-a-local-asset","source":"retained"}),
        )]),
    }
}
fn sealed(fixture: &Fixture) -> SealedJournal {
    let take = descriptor();
    let mut journal = Journal::create(fixture.source(), &take).unwrap();
    let clock = HostTimestamp {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("boot".into()),
        ticks: u64::MAX - 1,
    };
    let header = ChunkHeader {
        version: CHUNK_VERSION,
        take_id: take.take_id.clone(),
        stream_id: "audio".into(),
        segment_id: "audio-segment".into(),
        sequence: 0,
        range: SourceRange::AudioFrames {
            start: 100,
            count: 4,
        },
        interpretation: take.streams[0].kind.clone(),
        observed_start: clock.clone(),
        observed_end: clock,
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    };
    journal.append(&header, &[1; 16]).unwrap();
    // The raw stream has no selected events; its initial ordinal is not duration.
    journal.seal("seal").unwrap();
    drop(journal);
    SealedJournal::open(fixture.source()).unwrap()
}
fn project() -> ProjectDocument {
    let mut value = ProjectDocument::new("Preserved source")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["recording"] =
        json!({"takeId":"opaque-provider-record","clips":{"unfamiliar":true}});
    value["tracks"][0]["takeId"] = json!("opaque-note-take");
    value["vendor"]=serde_json::from_str(r#"{"integer":1234567890123456789012345678901234567890,"decimal":0.123456789012345678901234567890123456789,"recording":{"assetId":"opaque"}}"#).unwrap();
    ProjectDocument::from_value(value).unwrap()
}
fn clip(source: &SealedJournal, id: &str) -> TakeClip {
    TakeClip {
        id: id.into(),
        source: TakeReference {
            take_id: source.descriptor().take_id.clone(),
            manifest_sha256: source.manifest().sha256.clone(),
            stream_id: "audio".into(),
        },
        range: TakeRange::AudioFrames {
            start: 100,
            count: 4,
        },
        position: TimelinePosition {
            frame: -48_000,
            sample_rate: 48_000,
        },
        fades: None,
    }
}
fn request(project: &ProjectDocument, source: &SealedJournal, id: &str) -> TakePublication {
    TakePublication {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        track_id: TrackId("native-track".into()),
        track_name: "Original native take".into(),
        clip: clip(source, "native-clip"),
    }
}
fn edit(project: &ProjectDocument, id: &str) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit: EditOperation::Rename {
            name: "Later edit".into(),
        },
        extensions: Extensions::new(),
    }
}
fn counts(fixture: &Fixture, id: &str) -> (i64, i64, i64, i64, i64) {
    fixture.db().query_row("SELECT revision,cursor,(SELECT count(*) FROM edits WHERE project_id=?1),(SELECT count(*) FROM operations WHERE project_id=?1),(SELECT count(*) FROM native_takes) FROM projects WHERE id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap()
}

#[test]
fn publication_owns_original_and_preserves_opaque_legacy_values_through_history_and_reopen() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let original_bytes = fs::read(fixture.source()).unwrap();
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let intent = request(&original, &source, "publish-original");
    let published = store.publish_take(&intent).unwrap();
    assert!(!published.replayed);
    assert_eq!(published.document.revision(), 1);
    assert_eq!(published.document.schema_version(), 2);
    assert_eq!(
        published.document.value()["vendor"],
        original.value()["vendor"]
    );
    assert_eq!(published.document.tracks()[0], original.tracks()[0]);
    assert_eq!(
        published.document.take_clips().unwrap(),
        vec![intent.clip.clone()]
    );
    assert_eq!(fs::read(fixture.source()).unwrap(), original_bytes);
    let owned = fixture.object();
    assert_ne!(owned, fixture.source());
    drop(source);
    fs::rename(
        fixture.source(),
        fixture.0.join("external-source-moved.sqlite3"),
    )
    .unwrap();
    let undo = store.undo(original.id(), 1).unwrap();
    assert_eq!(undo.schema_version(), 1);
    assert!(undo.referenced_takes().unwrap().is_empty());
    let redo = store.redo(original.id(), 2).unwrap();
    assert_eq!(redo.schema_version(), 2);
    assert_eq!(redo.take_clips().unwrap(), vec![intent.clip]);
    drop(store);
    let reopened = fixture.store();
    assert_eq!(reopened.load(original.id()).unwrap(), redo);
    assert_eq!(
        SealedJournal::open(&owned)
            .unwrap()
            .read_chunk("audio", 0)
            .unwrap()
            .payload,
        [1; 16]
    );
}

#[test]
fn committed_retry_survives_external_source_loss_later_edits_and_redo_branch_truncation() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let intent = request(&original, &source, "publish-once");
    let first = store.publish_take(&intent).unwrap();
    let later = store
        .apply_edit(&edit(&first.document, "rename-later"))
        .unwrap()
        .document;
    drop(source);
    fs::rename(fixture.source(), fixture.0.join("moved-source")).unwrap();
    let replay = store.publish_take(&intent).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, first.document);
    assert_eq!(store.load(original.id()).unwrap(), later);
    let undo_rename = store.undo(original.id(), 2).unwrap();
    let undo_publish = store.undo(original.id(), undo_rename.revision()).unwrap();
    let branched = store
        .apply_edit(&edit(&undo_publish, "truncate-redo"))
        .unwrap()
        .document;
    assert!(branched.take_clips().unwrap().is_empty());
    let replay = store.publish_take(&intent).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, first.document);
    assert_eq!(store.load(original.id()).unwrap(), branched);
    assert!(store.redo(original.id(), branched.revision()).is_err());
    assert!(fixture.object().exists());
}

#[test]
fn forged_wrong_stream_unit_prefix_and_rate_references_publish_no_partial_track() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let base = request(&original, &source, "base");
    let mut cases = vec![];
    let mut wrong = base.clip.clone();
    wrong.source.take_id = TakeId("wrong-take".into());
    cases.push(wrong);
    let mut wrong = base.clip.clone();
    wrong.source.manifest_sha256 = "a".repeat(64);
    cases.push(wrong);
    let mut wrong = base.clip.clone();
    wrong.source.stream_id = "missing-stream".into();
    cases.push(wrong);
    let mut wrong = base.clip.clone();
    wrong.range = TakeRange::Events {
        start: 100,
        count: 1,
    };
    cases.push(wrong);
    let mut wrong = base.clip.clone();
    wrong.range = TakeRange::AudioFrames {
        start: 99,
        count: 1,
    };
    cases.push(wrong);
    let mut wrong = base.clip.clone();
    wrong.range = TakeRange::AudioFrames {
        start: 103,
        count: 2,
    };
    cases.push(wrong);
    let mut wrong = base.clip.clone();
    wrong.position.sample_rate = 96_000;
    cases.push(wrong);
    let mut wrong = base.clip.clone();
    wrong.source.stream_id = "events".into();
    wrong.range = TakeRange::Events { start: 7, count: 1 };
    cases.push(wrong);
    for (index, clip) in cases.into_iter().enumerate() {
        let mut intent = base.clone();
        intent.operation_id = OperationId(format!("reject-{index}"));
        intent.clip = clip;
        assert!(store.publish_take(&intent).is_err());
        assert_eq!(store.load(original.id()).unwrap(), original);
        assert_eq!(counts(&fixture, original.id()), (0, 0, 0, 0, 1));
    }
}

#[test]
fn final_receipt_failure_rolls_back_project_and_history_but_retains_ingested_original() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let intent = request(&original, &source, "fault-publication");
    let object = fixture.object();
    fixture.db().execute_batch("CREATE TRIGGER isolated_receipt_fault BEFORE INSERT ON operations BEGIN SELECT RAISE(ABORT,'isolated take publication fault'); END;").unwrap();
    assert!(store.publish_take(&intent).is_err());
    assert_eq!(counts(&fixture, original.id()), (0, 0, 0, 0, 1));
    assert_eq!(store.load(original.id()).unwrap(), original);
    assert!(object.exists());
    drop(store);
    fixture
        .db()
        .execute_batch("DROP TRIGGER isolated_receipt_fault;")
        .unwrap();
    let mut reopened = fixture.store();
    let committed = reopened.publish_take(&intent).unwrap();
    assert_eq!(committed.document.revision(), 1);
    assert!(!committed.replayed);
    assert_eq!(counts(&fixture, original.id()), (1, 1, 1, 1, 1));
}

#[test]
fn operation_identity_is_global_across_publication_and_regular_edits() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let intent = request(&original, &source, "global-operation");
    let first = store.publish_take(&intent).unwrap().document;
    assert!(store.apply_edit(&edit(&first, "global-operation")).is_err());
    let mut conflict = intent.clone();
    conflict.clip.position.frame += 1;
    assert!(store.publish_take(&conflict).is_err());
    assert_eq!(store.load(original.id()).unwrap(), first);
    let rename = store
        .apply_edit(&edit(&first, "edit-first"))
        .unwrap()
        .document;
    let mut reverse = request(&rename, &source, "edit-first");
    reverse.clip.id = "another-clip".into();
    assert!(store.publish_take(&reverse).is_err());
    assert_eq!(store.load(original.id()).unwrap(), rename);
}

#[test]
fn locked_existing_and_legacy_tracks_reject_without_reinterpreting_them() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let mut legacy = request(&original, &source, "legacy-target");
    legacy.track_id = TrackId(original.tracks()[0]["id"].as_str().unwrap().into());
    assert!(store.publish_take(&legacy).is_err());
    assert_eq!(store.load(original.id()).unwrap(), original);
    let first = store
        .publish_take(&request(&original, &source, "first"))
        .unwrap()
        .document;
    let mut value = first.value().clone();
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|track| track["kind"] == "recording")
        .unwrap()["locked"] = json!(true);
    let locked = store
        .save(
            &ProjectDocument::from_value(value).unwrap(),
            first.revision(),
        )
        .unwrap();
    let mut intent = request(&locked, &source, "locked-target");
    intent.clip.id = "second-clip".into();
    assert!(store.publish_take(&intent).is_err());
    assert_eq!(store.load(original.id()).unwrap(), locked);
}

#[test]
fn native_metadata_and_signed_timeline_do_not_inherit_schema_one_caps() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    store.ingest_take(&source).unwrap();
    let mut value = project().upgrade_native().unwrap().value().clone();
    value["bars"] = json!(u32::MAX);
    value["vendor_payload"] = json!("x".repeat(2_000_000));
    let native = ProjectDocument::from_value(value).unwrap();
    assert!(native.to_json().unwrap().len() > omniusong_core::MAX_PROJECT_BYTES);
    let original = store.create(&native).unwrap();
    let mut intent = request(&original, &source, "large-native");
    intent.clip.position.frame = 10_000_000_000;
    let first = store.publish_take(&intent).unwrap().document;
    assert_eq!(store.load(original.id()).unwrap(), first);
    assert!(store.publish_take(&intent).unwrap().replayed);
    let undo = store.undo(original.id(), 1).unwrap();
    let redo = store.redo(original.id(), undo.revision()).unwrap();
    assert_eq!(redo.take_clips().unwrap()[0].position.frame, 10_000_000_000);
    drop(store);
    assert_eq!(fixture.store().load(original.id()).unwrap(), redo);
}

#[cfg(unix)]
#[test]
fn symlinked_or_hardlinked_owned_objects_and_redirected_take_directory_fail_closed() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let intent = request(&original, &source, "guarded-object");
    let object = fixture.object();
    let retained = fixture.0.join("retained-owned.sqlite3");
    fs::rename(&object, &retained).unwrap();
    symlink(&retained, &object).unwrap();
    assert!(store.publish_take(&intent).is_err());
    fs::remove_file(&object).unwrap();
    fs::hard_link(&retained, &object).unwrap();
    assert!(store.publish_take(&intent).is_err());
    fs::remove_file(&object).unwrap();
    fs::rename(&retained, &object).unwrap();
    let directory = fixture.root().join("takes");
    let moved = fixture.0.join("moved-takes");
    fs::rename(&directory, &moved).unwrap();
    symlink(&moved, &directory).unwrap();
    assert!(store.publish_take(&intent).is_err());
    fs::remove_file(&directory).unwrap();
    fs::rename(&moved, &directory).unwrap();
    assert_eq!(counts(&fixture, original.id()), (0, 0, 0, 0, 1));
    assert_eq!(store.publish_take(&intent).unwrap().document.revision(), 1);
}

#[test]
fn corrupt_owned_snapshot_is_not_accepted_by_load_or_historical_receipt_replay() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let intent = request(&original, &source, "original");
    store.publish_take(&intent).unwrap();
    let object = fixture.object();
    let db = Connection::open(&object).unwrap();
    db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'00000000000000000000000000000000';").unwrap();
    drop(db);
    assert!(store.load(original.id()).is_err());
    assert!(store.publish_take(&intent).is_err());
    assert_eq!(counts(&fixture, original.id()), (1, 1, 1, 1, 1));
}

#[test]
fn failed_registry_insert_retains_a_recoverable_owned_object_without_publishing_a_track() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let original_bytes = fs::read(fixture.source()).unwrap();
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    fixture.db().execute_batch("CREATE TRIGGER isolated_registry_fault BEFORE INSERT ON native_takes BEGIN SELECT RAISE(ABORT,'isolated registry fault'); END;").unwrap();
    assert!(store.ingest_take(&source).is_err());
    assert_eq!(counts(&fixture, original.id()), (0, 0, 0, 0, 0));
    assert_eq!(store.load(original.id()).unwrap(), original);
    let orphans: Vec<_> = fs::read_dir(fixture.root().join("takes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sqlite3"))
        .collect();
    assert!(!orphans.is_empty());
    for path in orphans {
        assert_eq!(
            SealedJournal::open(path).unwrap().manifest(),
            source.manifest()
        );
    }
    assert!(
        store
            .publish_take(&request(&original, &source, "unregistered"))
            .is_err()
    );
    assert_eq!(fs::read(fixture.source()).unwrap(), original_bytes);
    fixture
        .db()
        .execute_batch("DROP TRIGGER isolated_registry_fault;")
        .unwrap();
    store.ingest_take(&source).unwrap();
    assert_eq!(
        store
            .publish_take(&request(&original, &source, "registered"))
            .unwrap()
            .document
            .revision(),
        1
    );
}

#[test]
fn concurrent_identical_publication_requests_commit_one_clip_history_and_receipt() {
    use std::sync::{Arc, Barrier};
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let intent = request(&original, &source, "concurrent-publication");
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let root = fixture.root();
            let intent = intent.clone();
            std::thread::spawn(move || {
                let mut store = NativeStore::open(root).unwrap();
                barrier.wait();
                store.publish_take(&intent).unwrap()
            })
        })
        .collect();
    let receipts: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        receipts.iter().filter(|receipt| receipt.replayed).count(),
        1
    );
    assert_eq!(receipts[0].document, receipts[1].document);
    assert_eq!(counts(&fixture, original.id()), (1, 1, 1, 1, 1));
    assert_eq!(
        fixture
            .store()
            .load(original.id())
            .unwrap()
            .take_clips()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn cached_save_and_regular_edit_cannot_commit_with_missing_or_corrupt_existing_originals() {
    for missing in [false, true] {
        for direct_save in [false, true] {
            let fixture = Fixture::new();
            let source = sealed(&fixture);
            let mut store = fixture.store();
            let original = store.create(&project()).unwrap();
            store.ingest_take(&source).unwrap();
            let cached = store
                .publish_take(&request(&original, &source, "initial-publication"))
                .unwrap()
                .document;
            let object = fixture.object();
            if missing {
                fs::rename(&object, fixture.0.join("retained-missing-original")).unwrap();
            } else {
                let db = Connection::open(&object).unwrap();
                db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'00000000000000000000000000000000';").unwrap();
            }
            if direct_save {
                let mut next = cached.clone();
                next.rename("Cached draft").unwrap();
                assert!(
                    store.save(&next, cached.revision()).is_err(),
                    "direct save committed damaged original"
                );
            } else {
                assert!(
                    store.apply_edit(&edit(&cached, "cached-rename")).is_err(),
                    "edit ledger committed damaged original"
                );
            }
            assert_eq!(counts(&fixture, original.id()), (1, 1, 1, 1, 1));
            let payload: String = fixture
                .db()
                .query_row(
                    "SELECT payload FROM projects WHERE id=?1",
                    [original.id()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                ProjectDocument::from_json(payload.as_bytes()).unwrap(),
                cached
            );
        }
    }
}

#[test]
fn undo_and_redo_cannot_commit_when_their_result_still_references_a_damaged_original() {
    for redo in [false, true] {
        let fixture = Fixture::new();
        let source = sealed(&fixture);
        let mut store = fixture.store();
        let original = store.create(&project()).unwrap();
        store.ingest_take(&source).unwrap();
        let published = store
            .publish_take(&request(&original, &source, "publish-history"))
            .unwrap()
            .document;
        let renamed = store
            .apply_edit(&edit(&published, "rename-history"))
            .unwrap()
            .document;
        let current = if redo {
            store.undo(original.id(), renamed.revision()).unwrap()
        } else {
            renamed
        };
        let before = counts(&fixture, original.id());
        let object = fixture.object();
        fs::rename(&object, fixture.0.join("retained-history-source")).unwrap();
        let result = if redo {
            store.redo(original.id(), current.revision())
        } else {
            store.undo(original.id(), current.revision())
        };
        assert!(result.is_err());
        assert_eq!(counts(&fixture, original.id()), before);
        let payload: String = fixture
            .db()
            .query_row(
                "SELECT payload FROM projects WHERE id=?1",
                [original.id()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            ProjectDocument::from_json(payload.as_bytes()).unwrap(),
            current
        );
    }
}

#[test]
fn opaque_legacy_named_fields_on_recording_tracks_are_not_assets_or_editable_notes() {
    let fixture = Fixture::new();
    let source = sealed(&fixture);
    let mut store = fixture.store();
    let original = store.create(&project()).unwrap();
    store.ingest_take(&source).unwrap();
    let published = store
        .publish_take(&request(&original, &source, "opaque-recording-track"))
        .unwrap()
        .document;
    let mut value = published.value().clone();
    let recording = value["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|track| track["kind"] == "recording")
        .unwrap();
    recording["notes"] = json!(["opaque-vendor-note",{"vendor":true}]);
    recording["audio"] = json!({"unfamiliar":"not legacy audio"});
    recording["assetId"] = json!("foreign-opaque-asset");
    recording["provenance"] = json!({"input":{"assetId":"foreign-opaque-input"}});
    let saved = store
        .save(
            &ProjectDocument::from_value(value).unwrap(),
            published.revision(),
        )
        .unwrap();
    assert!(saved.referenced_assets().unwrap().is_empty());
    assert_eq!(saved.take_clips().unwrap().len(), 1);
    let summary = store
        .list()
        .unwrap()
        .into_iter()
        .find(|summary| summary.id == original.id())
        .unwrap();
    assert_eq!(summary.note_count, 0);
    assert!(summary.problem.is_none());
    assert_eq!(store.load(original.id()).unwrap(), saved);
}

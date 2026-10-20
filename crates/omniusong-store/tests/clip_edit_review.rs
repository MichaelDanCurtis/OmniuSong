//! Independent clip-edit persistence and archive regressions. Synthetic sources only.
use omniusong_core::{
    AudioSourceWindow, ClipPatch, ClipTimelinePolicy, DocumentClipAddress, FieldChange,
    LegacyAudioClipPatch, NativeAudioClipPatch, ProjectDocument, ValueChange,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId, TrackId},
    native::{AudioFades, FadeCurve, TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, RestoreCopyIntent, StoreError, TakePublication};
use rusqlite::{Connection, params};
use serde_json::{Number, Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf};

const KEEP: ClipTimelinePolicy = ClipTimelinePolicy::PreserveLoop;
const LEGACY: &str = " 🎵\0legacy";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/clip-edit-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn store(&self, name: &str) -> NativeStore {
        NativeStore::open(self.0.join(name)).unwrap()
    }
    fn db(&self, name: &str) -> Connection {
        Connection::open(self.0.join(name).join("studio.sqlite3")).unwrap()
    }
    fn source(&self) -> PathBuf {
        self.0.join("original.sqlite3")
    }
    fn object(&self, name: &str, store: &NativeStore) -> PathBuf {
        let take = store
            .registered_take(&TakeId("clip-source".into()))
            .unwrap();
        self.0
            .join(name)
            .join("takes")
            .join(format!("{}.sqlite3", take.object_sha256))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Clip review fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn decimal(text: &str) -> Number {
    serde_json::from_str(text).unwrap()
}
fn native_address() -> DocumentClipAddress {
    DocumentClipAddress::NativeAudio {
        track_id: "native".into(),
        clip_id: "clip".into(),
    }
}
fn legacy_address() -> DocumentClipAddress {
    DocumentClipAddress::LegacyAudio {
        track_id: LEGACY.into(),
    }
}
fn native_patch() -> ClipPatch {
    ClipPatch::NativeAudio(NativeAudioClipPatch {
        position_frame: ValueChange::Set { value: -2 },
        source_window: ValueChange::Set {
            value: AudioSourceWindow {
                start_frame: 101,
                frame_count: 4,
            },
        },
        fades: FieldChange::Set {
            value: AudioFades {
                in_frames: 1,
                out_frames: 1,
                curve: FadeCurve::EqualPower,
            },
        },
    })
}
fn legacy_patch() -> ClipPatch {
    ClipPatch::LegacyAudio(LegacyAudioClipPatch {
        start_tick: FieldChange::Set { value: 9001 },
        trim_start_seconds: FieldChange::Set {
            value: decimal("0.12345678901234567890123456789"),
        },
        trim_end_seconds: FieldChange::Keep,
        fade_in_seconds: FieldChange::Set {
            value: decimal("0.0250000000000000000000000000001"),
        },
        fade_out_seconds: FieldChange::Remove,
    })
}
fn intent(
    doc: &ProjectDocument,
    id: &str,
    address: DocumentClipAddress,
    edit: EditOperation,
) -> ProjectEditIntent {
    let scope = if matches!(
        edit,
        EditOperation::Undo | EditOperation::Redo | EditOperation::Rename { .. }
    ) {
        EditScope::Project
    } else {
        EditScope::DocumentClip { address }
    };
    let mut request = ProjectEditIntent {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(doc.id().into()),
        expected_revision: doc.revision(),
        scope,
        edit,
        extensions: BTreeMap::new(),
    };
    request.extensions.insert(
        "vendor".into(),
        json!({"project_id":doc.id(),"operation_id":id,"clipId":"opaque-not-remapped"}),
    );
    request
}
fn update(doc: &ProjectDocument, id: &str) -> ProjectEditIntent {
    intent(
        doc,
        id,
        native_address(),
        EditOperation::UpdateClip {
            patch: native_patch(),
            timeline: KEEP,
        },
    )
}
fn history(doc: &ProjectDocument, id: &str, edit: EditOperation) -> ProjectEditIntent {
    intent(doc, id, native_address(), edit)
}
fn state(db: &Connection, id: &str) -> (String, i64, i64, i64, i64) {
    db.query_row("SELECT payload,revision,cursor,(SELECT count(*) FROM edits),(SELECT count(*) FROM operations) FROM projects WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap()
}
fn sealed(f: &Fixture, recovered: bool) -> SealedJournal {
    let descriptor = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("clip-source".into()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            source_id: "synthetic-input".into(),
            source_name: "Synthetic original".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("boot".into()),
                ticks_per_second_numerator: 24_000_000,
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
        provenance: BTreeMap::from([(
            "vendor".into(),
            json!({"takeId":"opaque","rawObservation":"unchanged"}),
        )]),
    };
    let stamp = HostTimestamp {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("boot".into()),
        ticks: 123,
    };
    let header = |sequence, start, count| ChunkHeader {
        version: CHUNK_VERSION,
        take_id: descriptor.take_id.clone(),
        stream_id: "audio".into(),
        segment_id: "segment".into(),
        sequence,
        range: SourceRange::AudioFrames { start, count },
        interpretation: descriptor.streams[0].kind.clone(),
        observed_start: stamp.clone(),
        observed_end: stamp.clone(),
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    };
    let mut journal = Journal::create(f.source(), &descriptor).unwrap();
    journal
        .append(
            &header(0, 100, 8),
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        )
        .unwrap();
    if recovered {
        journal
            .append(&header(1, 108, 4), &[17, 18, 19, 20, 21, 22, 23, 24])
            .unwrap();
        drop(journal);
        let db = Connection::open(f.source()).unwrap();
        db.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=x'0000000000000000' WHERE sequence=1;").unwrap();
        drop(db);
        journal = Journal::open(f.source()).unwrap();
        journal.seal_recovered("reviewed-recovery").unwrap();
    } else {
        journal.seal("complete-source").unwrap();
    }
    drop(journal);
    SealedJournal::open(f.source()).unwrap()
}
fn original_clip(source: &SealedJournal) -> TakeClip {
    TakeClip {
        id: "clip".into(),
        source: TakeReference {
            take_id: source.descriptor().take_id.clone(),
            manifest_sha256: source.manifest().sha256.clone(),
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
    }
}
fn native_document(source: &SealedJournal) -> ProjectDocument {
    let mut doc = ProjectDocument::new("Native clip source")
        .unwrap()
        .upgrade_native()
        .unwrap();
    doc.append_take_clip("native", "Selected original", original_clip(source))
        .unwrap();
    let mut sibling = original_clip(source);
    sibling.id = "sibling".into();
    doc.append_take_clip("native", "Selected original", sibling)
        .unwrap();
    let mut other = original_clip(source);
    other.id = "other".into();
    doc.append_take_clip("other-track", "Unchanged original", other)
        .unwrap();
    let mut value = doc.value().clone();
    value["tracks"][2]["recording"]["clips"][0]["fades"] = Value::Null;
    value["tracks"][2]["recording"]["clips"][1]["fades"] = Value::Null;
    value["tracks"][2]["assetId"] = "opaque-not-a-local-reference".into();
    value["vendor"] = serde_json::from_str(r#"{"integer":184467440737095516160001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
    ProjectDocument::from_value(value).unwrap()
}
fn imported_legacy(
    f: &Fixture,
    store: &mut NativeStore,
) -> (ProjectDocument, Vec<u8>, String, Vec<u8>) {
    // Deliberately opaque codec bytes: a semantic edit must not invoke a decoder.
    let asset = b"synthetic encoded original, never decoded by the store".to_vec();
    let id = format!("{}.webm", digest(&asset));
    let assets = f.0.join("legacy-assets");
    fs::create_dir(&assets).unwrap();
    fs::write(assets.join(&id), &asset).unwrap();
    let mut value = ProjectDocument::new("Legacy clip original")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["id"] = LEGACY.into();
    value["tracks"][0]["kind"] = "audio".into();
    value["tracks"][0]["assetId"] = id.clone().into();
    value["tracks"][0]["audio"] = json!({"origin":"microphone","mimeType":"audio/webm","durationSeconds":8,"byteLength":asset.len(),"trimEndSeconds":decimal("7.999999999999999999999999999999"),"fadeOutSeconds":decimal("0.000000000000000000000000000001"),"recordingTiming":{"version":1,"method":"browser-recorder-start-call","calibration":"unmeasured","ppq":960,"captureAudioTimeSeconds":10,"songStartAudioTimeSeconds":10,"tempo":104,"loopTicks":15360,"status":"estimated"},"vendor":{"sourceId":"untouched"}});
    value["tracks"][0]["notes"] = json!([{"id":"opaque-guide","pitch":60,"startTick":0,"durationTicks":960,"velocity16":50001,"expression":null}]);
    value["vendor"] = serde_json::from_str(r#"{"integer":184467440737095516160001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
    let doc = ProjectDocument::from_value(value).unwrap();
    let mut raw = b" \n\t".to_vec();
    raw.extend(doc.to_json().unwrap());
    raw.extend_from_slice(b"\n ");
    let path = f.0.join("original-project.json");
    fs::write(&path, &raw).unwrap();
    (store.import_legacy(&path, &assets).unwrap(), raw, id, asset)
}

#[test]
fn legacy_exact_numbers_omissions_original_and_codec_bytes_survive_restore_and_archives() {
    let f = Fixture::new();
    let mut store = f.store("source");
    let (original, raw, asset_id, asset) = imported_legacy(&f, &mut store);
    let request = intent(
        &original,
        "legacy-update",
        legacy_address(),
        EditOperation::UpdateClip {
            patch: legacy_patch(),
            timeline: KEEP,
        },
    );
    let edited = store.apply_edit(&request).unwrap().document;
    let audio = &edited.tracks()[0]["audio"];
    assert_eq!(
        audio["trimStartSeconds"].to_string(),
        "0.12345678901234567890123456789"
    );
    assert_eq!(
        audio["trimEndSeconds"].to_string(),
        "7.999999999999999999999999999999"
    );
    assert_eq!(
        audio["fadeInSeconds"].to_string(),
        "0.0250000000000000000000000000001"
    );
    assert!(audio.get("fadeOutSeconds").is_none());
    assert!(audio["editSource"]["original"].get("startTick").is_none());
    assert_eq!(
        audio["editSource"]["original"]["fadeOutSeconds"],
        original.tracks()[0]["audio"]["fadeOutSeconds"]
    );
    assert_eq!(edited.tracks()[0]["notes"], original.tracks()[0]["notes"]);
    assert_eq!(
        audio["recordingTiming"],
        original.tracks()[0]["audio"]["recordingTiming"]
    );
    let restored = store
        .apply_edit(&intent(
            &edited,
            "legacy-restore",
            legacy_address(),
            EditOperation::RestoreClip { timeline: KEEP },
        ))
        .unwrap()
        .document;
    let mut expected = original.tracks()[0].clone();
    expected["audio"]["editSource"] = audio["editSource"].clone();
    assert_eq!(restored.tracks()[0], expected);
    assert_eq!(restored.value()["vendor"], original.value()["vendor"]);
    let archive = f.0.join("legacy.omniusong-native");
    assert_eq!(
        store
            .export_archive(original.id(), &archive)
            .unwrap()
            .asset_count,
        1
    );
    let archive_bytes = fs::read(&archive).unwrap();
    let mut identity = f.store("identity");
    assert_eq!(identity.restore_archive(&archive).unwrap(), restored);
    assert_eq!(
        identity.imported_original(original.id()).unwrap().unwrap(),
        raw
    );
    assert_eq!(identity.asset(&asset_id).unwrap(), asset);
    assert_eq!(identity.apply_edit(&request).unwrap().document, edited);
    assert_eq!(identity.load(original.id()).unwrap(), restored);
    let mut copy = f.store("copy");
    let inspection = copy.inspect_archive(&archive).unwrap();
    let copied = copy
        .restore_archive_as_copy(
            &archive,
            &RestoreCopyIntent {
                operation_id: OperationId("legacy-copy".into()),
                expected_archive_sha256: inspection.sha256,
            },
        )
        .unwrap()
        .document;
    assert_eq!(copied.tracks(), restored.tracks());
    assert_eq!(copy.imported_original(copied.id()).unwrap().unwrap(), raw);
    assert_eq!(copy.asset(&asset_id).unwrap(), asset);
    assert_eq!(fs::read(f.0.join("original-project.json")).unwrap(), raw);
    assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
}

#[test]
fn native_update_restore_preserves_preroll_null_siblings_and_entire_owned_original() {
    let f = Fixture::new();
    let source = sealed(&f, true);
    let source_bytes = fs::read(f.source()).unwrap();
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let object = f.object("source", &store);
    let object_bytes = fs::read(&object).unwrap();
    let original = store.create(&native_document(&source)).unwrap();
    let edited = store
        .apply_edit(&update(&original, "trim-native"))
        .unwrap()
        .document;
    let raw = &edited.tracks()[2]["recording"];
    assert_eq!(raw["version"], 2);
    assert_eq!(
        raw["clips"][1],
        original.tracks()[2]["recording"]["clips"][1]
    );
    assert_eq!(edited.tracks()[3], original.tracks()[3]);
    assert_eq!(
        raw["clips"][0]["editSource"]["original"]["fades"],
        Value::Null
    );
    assert!(
        raw["clips"][0]["editSource"]["original"]
            .get("fades")
            .is_some()
    );
    assert_eq!(raw["clips"][0]["position"]["frame"], -2);
    assert_eq!(
        raw["clips"][0]["source"],
        original.tracks()[2]["recording"]["clips"][0]["source"]
    );
    // Existing outer snapshot metadata is immutable even when restoring settings.
    let mut extended = edited.value().clone();
    extended["tracks"][2]["recording"]["clips"][0]["editSource"]["vendor"] =
        original.value()["vendor"].clone();
    let with_extension = store
        .save(
            &ProjectDocument::from_value(extended).unwrap(),
            edited.revision(),
        )
        .unwrap();
    let restored = store
        .apply_edit(&intent(
            &with_extension,
            "restore-native",
            native_address(),
            EditOperation::RestoreClip { timeline: KEEP },
        ))
        .unwrap()
        .document;
    let mut expected = original.tracks()[2]["recording"]["clips"][0].clone();
    expected["editSource"] =
        with_extension.tracks()[2]["recording"]["clips"][0]["editSource"].clone();
    assert_eq!(restored.tracks()[2]["recording"]["clips"][0], expected);
    assert_eq!(restored.tracks()[2]["recording"]["version"], 2);
    assert_eq!(fs::read(&object).unwrap(), object_bytes);
    assert_eq!(fs::read(f.source()).unwrap(), source_bytes);
    let physical = Connection::open(&object).unwrap();
    let bad: Vec<u8> = physical
        .query_row("SELECT payload FROM chunks WHERE sequence=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(bad, [0; 8]);
    assert!(
        store
            .open_take(&original_clip(&source).source)
            .unwrap()
            .read_chunk("audio", 1)
            .is_err()
    );
}

#[test]
fn both_current_and_retained_original_ranges_require_the_authenticated_prefix() {
    let f = Fixture::new();
    let source = sealed(&f, true);
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let initial = store.create(&native_document(&source)).unwrap();
    let edited = store
        .apply_edit(&update(&initial, "valid-window"))
        .unwrap()
        .document;
    let before = state(&f.db("source"), initial.id());
    for original in [false, true] {
        for fault in ["below-origin", "beyond-verified", "rate"] {
            let mut value = edited.value().clone();
            let clip = &mut value["tracks"][2]["recording"]["clips"][0];
            let settings = if original {
                &mut clip["editSource"]["original"]
            } else {
                clip
            };
            match fault {
                "below-origin" => {
                    settings["range"] = json!({"unit":"audio_frames","start":99,"count":4})
                }
                "beyond-verified" => {
                    settings["range"] = json!({"unit":"audio_frames","start":107,"count":2})
                }
                "rate" => {
                    settings["position"]["sampleRate"] = 44_100.into();
                    // Keep both rates internally consistent so the store, not the
                    // structural validator, must reject the authenticated PCM mismatch.
                    value["tracks"][2]["recording"]["clips"][0]["position"]["sampleRate"] =
                        44_100.into();
                    value["tracks"][2]["recording"]["clips"][0]["editSource"]["original"]["position"]
                        ["sampleRate"] = 44_100.into();
                }
                _ => unreachable!(),
            }
            let candidate = ProjectDocument::from_value(value).unwrap();
            let error = store.save(&candidate, edited.revision()).unwrap_err();
            assert!(
                !matches!(error, StoreError::EditRejected(_)),
                "source authentication is not a pure edit error"
            );
            assert_eq!(state(&f.db("source"), initial.id()), before);
        }
    }
    for (start, count) in [(99, 4), (107, 2)] {
        let mut request = update(&edited, &format!("outside-{start}"));
        if let EditOperation::UpdateClip {
            patch: ClipPatch::NativeAudio(patch),
            ..
        } = &mut request.edit
        {
            patch.source_window = ValueChange::Set {
                value: AudioSourceWindow {
                    start_frame: start,
                    frame_count: count,
                },
            };
        }
        assert!(store.apply_edit(&request).is_err());
        assert_eq!(state(&f.db("source"), initial.id()), before);
    }
    assert_eq!(store.load(initial.id()).unwrap(), edited);
}

#[test]
fn exact_retry_conflict_undo_redo_and_reopen_do_not_reapply_or_rebase() {
    let f = Fixture::new();
    let source = sealed(&f, false);
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let original = store.create(&native_document(&source)).unwrap();
    let request = update(&original, "once");
    let first = store.apply_edit(&request).unwrap();
    assert!(!first.replayed);
    let later = store
        .apply_edit(&history(
            &first.document,
            "later",
            EditOperation::Rename {
                name: "Later session".into(),
            },
        ))
        .unwrap()
        .document;
    let replay = store.apply_edit(&request).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, first.document);
    assert_eq!(store.load(original.id()).unwrap(), later);
    let before = state(&f.db("source"), original.id());
    let mut changed = request.clone();
    if let EditOperation::UpdateClip {
        patch: ClipPatch::NativeAudio(patch),
        ..
    } = &mut changed.edit
    {
        patch.position_frame = ValueChange::Set { value: 0 };
    }
    assert!(store.apply_edit(&changed).is_err());
    let mut stale = request.clone();
    stale.operation_id = OperationId("stale-new-id".into());
    assert!(matches!(
        store.apply_edit(&stale),
        Err(StoreError::Conflict)
    ));
    assert_eq!(state(&f.db("source"), original.id()), before);
    let undone = store
        .apply_edit(&history(&later, "undo-rename", EditOperation::Undo))
        .unwrap()
        .document;
    assert_eq!(undone.tracks(), first.document.tracks());
    let undone_clip = store
        .apply_edit(&history(&undone, "undo-clip", EditOperation::Undo))
        .unwrap()
        .document;
    assert_eq!(undone_clip.tracks(), original.tracks());
    let redone = store
        .apply_edit(&history(&undone_clip, "redo-clip", EditOperation::Redo))
        .unwrap()
        .document;
    assert_eq!(redone.tracks(), first.document.tracks());
    drop(store);
    let mut reopened = f.store("source");
    assert_eq!(reopened.load(original.id()).unwrap(), redone);
    assert!(reopened.apply_edit(&request).unwrap().replayed);
    assert_eq!(reopened.load(original.id()).unwrap(), redone);
}

#[test]
fn final_receipt_sql_failure_rolls_back_all_project_state_then_same_intent_succeeds() {
    let f = Fixture::new();
    let source = sealed(&f, false);
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let original = store.create(&native_document(&source)).unwrap();
    let object = f.object("source", &store);
    let bytes = fs::read(&object).unwrap();
    let db = f.db("source");
    let before = state(&db, original.id());
    db.execute_batch("CREATE TRIGGER clip_receipt_failure BEFORE INSERT ON operations WHEN NEW.operation_id='retry-after-sql' BEGIN SELECT RAISE(ABORT,'Injected clip receipt failure'); END;").unwrap();
    let request = update(&original, "retry-after-sql");
    assert!(matches!(
        store.apply_edit(&request),
        Err(StoreError::Database(_))
    ));
    assert_eq!(state(&db, original.id()), before);
    assert_eq!(fs::read(&object).unwrap(), bytes);
    db.execute_batch("DROP TRIGGER clip_receipt_failure")
        .unwrap();
    assert!(!store.apply_edit(&request).unwrap().replayed);
    assert!(store.apply_edit(&request).unwrap().replayed);
}

#[test]
fn scope_lock_representation_and_fade_rejections_never_publish_partial_settings() {
    for fault in [
        "locked",
        "wrong-kind",
        "wrong-scope",
        "wrong-representation",
        "fades",
        "missing",
    ] {
        let f = Fixture::new();
        let source = sealed(&f, false);
        let mut store = f.store("source");
        store.ingest_take(&source).unwrap();
        let mut value = native_document(&source).value().clone();
        if fault == "locked" {
            value["tracks"][2]["locked"] = true.into();
        }
        let original = store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap();
        let mut request = update(&original, "rejected");
        match fault {
            "wrong-kind" => {
                request.scope = EditScope::DocumentClip {
                    address: DocumentClipAddress::NativeAudio {
                        track_id: original.tracks()[0]["id"].as_str().unwrap().into(),
                        clip_id: "clip".into(),
                    },
                }
            }
            "wrong-scope" => request.scope = EditScope::Project,
            "wrong-representation" => {
                request.edit = EditOperation::UpdateClip {
                    patch: legacy_patch(),
                    timeline: KEEP,
                }
            }
            "fades" => {
                if let EditOperation::UpdateClip {
                    patch: ClipPatch::NativeAudio(patch),
                    ..
                } = &mut request.edit
                {
                    patch.fades = FieldChange::Set {
                        value: AudioFades {
                            in_frames: 3,
                            out_frames: 2,
                            curve: FadeCurve::Linear,
                        },
                    };
                }
            }
            "missing" => {
                request.scope = EditScope::DocumentClip {
                    address: DocumentClipAddress::NativeAudio {
                        track_id: "native".into(),
                        clip_id: "missing".into(),
                    },
                }
            }
            _ => {}
        }
        let before = state(&f.db("source"), original.id());
        let error = store.apply_edit(&request).unwrap_err();
        if matches!(fault, "wrong-scope" | "wrong-representation") {
            assert!(matches!(error, StoreError::Invalid(_)));
        } else {
            assert!(
                matches!(error, StoreError::EditRejected(_)),
                "{fault}: {error}"
            );
        }
        assert_eq!(state(&f.db("source"), original.id()), before);
    }
}

#[test]
fn operation_only_native_results_archive_the_complete_bad_tail_and_copy_all_receipts() {
    let f = Fixture::new();
    let source = sealed(&f, true);
    let source_bytes = fs::read(f.source()).unwrap();
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let object = f.object("source", &store);
    let object_bytes = fs::read(&object).unwrap();
    let initial = store
        .create(&ProjectDocument::new("Before any recording").unwrap())
        .unwrap();
    let publish = TakePublication {
        operation_id: OperationId("publish".into()),
        project_id: ProjectId(initial.id().into()),
        expected_revision: initial.revision(),
        track_id: TrackId("native".into()),
        track_name: "Original take".into(),
        clip: original_clip(&source),
    };
    let published = store.publish_take(&publish).unwrap().document;
    let edit = update(&published, "edited");
    let edited = store.apply_edit(&edit).unwrap().document;
    let restore = intent(
        &edited,
        "restored",
        native_address(),
        EditOperation::RestoreClip { timeline: KEEP },
    );
    let restored = store.apply_edit(&restore).unwrap().document;
    let mut current = restored.clone();
    for id in ["undo-restore", "undo-edit", "undo-publication"] {
        current = store
            .apply_edit(&history(&current, id, EditOperation::Undo))
            .unwrap()
            .document;
    }
    current = store
        .apply_edit(&history(
            &current,
            "branch",
            EditOperation::Rename {
                name: "Branch without clips".into(),
            },
        ))
        .unwrap()
        .document;
    assert!(current.referenced_takes().unwrap().is_empty());
    let db = f.db("source");
    let history_rows: i64 = db
        .query_row(
            "SELECT count(*) FROM edits WHERE project_id=?1",
            [initial.id()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history_rows, 1);
    let archive = f.0.join("operation-only.omniusong-native");
    let receipt = store.export_archive(initial.id(), &archive).unwrap();
    assert_eq!(receipt.take_count, 1);
    assert_eq!(receipt.operation_rows, 7);
    let archive_bytes = fs::read(&archive).unwrap();
    let mut identity = f.store("identity");
    assert_eq!(identity.restore_archive(&archive).unwrap(), current);
    assert_eq!(identity.apply_edit(&edit).unwrap().document, edited);
    assert_eq!(identity.apply_edit(&restore).unwrap().document, restored);
    assert_eq!(identity.load(current.id()).unwrap(), current);
    assert_eq!(
        fs::read(f.object("identity", &identity)).unwrap(),
        object_bytes
    );
    let mut copy = f.store("copy");
    let inspection = copy.inspect_archive(&archive).unwrap();
    let copied = copy
        .restore_archive_as_copy(
            &archive,
            &RestoreCopyIntent {
                operation_id: OperationId("copy-native".into()),
                expected_archive_sha256: inspection.sha256,
            },
        )
        .unwrap()
        .document;
    let mut expected = current.value().clone();
    expected["id"] = copied.id().into();
    assert_eq!(copied.value(), &expected);
    let copy_db = f.db("copy");
    for old_id in ["edited", "restored"] {
        let new_id: String = copy_db.query_row("SELECT destination_operation_id FROM archive_copy_proof_operations WHERE source_operation_id=?1", [old_id], |r|r.get(0)).unwrap();
        assert_ne!(new_id, old_id);
        let request_json: String = copy_db
            .query_row(
                "SELECT request_json FROM operations WHERE operation_id=?1",
                [&new_id],
                |r| r.get(0),
            )
            .unwrap();
        let request: ProjectEditIntent = serde_json::from_str(&request_json).unwrap();
        assert_eq!(request.project_id.0, copied.id());
        assert_eq!(
            request.scope,
            EditScope::DocumentClip {
                address: native_address()
            }
        );
        assert_eq!(request.extensions["vendor"]["project_id"], current.id());
        assert_eq!(request.extensions["vendor"]["operation_id"], old_id);
        let result = copy.apply_edit(&request).unwrap();
        assert!(result.replayed);
        let original_result = if old_id == "edited" {
            &edited
        } else {
            &restored
        };
        assert_eq!(result.document.tracks(), original_result.tracks());
    }
    assert_eq!(copy.load(copied.id()).unwrap(), copied);
    assert_eq!(fs::read(f.object("copy", &copy)).unwrap(), object_bytes);
    let copied_archive = f.0.join("copied.omniusong-native");
    copy.export_archive(copied.id(), &copied_archive).unwrap();
    let mut second = f.store("copy-restored");
    assert_eq!(second.restore_archive(&copied_archive).unwrap(), copied);
    assert_eq!(
        fs::read(f.object("copy-restored", &second)).unwrap(),
        object_bytes
    );
    assert_eq!(fs::read(f.source()).unwrap(), source_bytes);
    assert_eq!(fs::read(&object).unwrap(), object_bytes);
    assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
}

#[test]
fn replay_authenticates_the_owned_source_even_after_a_later_valid_edit() {
    let f = Fixture::new();
    let source = sealed(&f, false);
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let initial = store.create(&native_document(&source)).unwrap();
    let request = update(&initial, "edited");
    let edited = store.apply_edit(&request).unwrap().document;
    let later = store
        .apply_edit(&history(
            &edited,
            "later",
            EditOperation::Rename {
                name: "Keep later revision".into(),
            },
        ))
        .unwrap()
        .document;
    let path = f.object("source", &store);
    let bytes = fs::read(&path).unwrap();
    let before = state(&f.db("source"), initial.id());
    let mut damaged = bytes.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    fs::write(&path, &damaged).unwrap();
    assert!(store.apply_edit(&request).is_err());
    assert_eq!(state(&f.db("source"), initial.id()), before);
    fs::write(&path, &bytes).unwrap();
    assert!(store.apply_edit(&request).unwrap().replayed);
    assert_eq!(store.load(initial.id()).unwrap(), later);
}

#[test]
fn rehashed_native_receipts_must_match_explicit_fields_restore_presence_and_extent() {
    let f = Fixture::new();
    let source = sealed(&f, true);
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let mut value = native_document(&source).value().clone();
    value["bars"] = 1.into();
    let initial = store
        .create(&ProjectDocument::from_value(value).unwrap())
        .unwrap();
    let mut request = update(&initial, "updated");
    if let EditOperation::UpdateClip {
        patch: ClipPatch::NativeAudio(patch),
        ..
    } = &mut request.edit
    {
        patch.position_frame = ValueChange::Set { value: 480_000 };
    }
    let edited = store.apply_edit(&request).unwrap().document;
    assert_eq!(edited.bars(), 1);
    let restored = store
        .apply_edit(&intent(
            &edited,
            "restored",
            native_address(),
            EditOperation::RestoreClip { timeline: KEEP },
        ))
        .unwrap()
        .document;
    assert!(
        restored.tracks()[2]["recording"]["clips"][0]
            .get("fades")
            .is_some()
    );
    let good_path = f.0.join("good-native.omniusong-native");
    store.export_archive(initial.id(), &good_path).unwrap();
    let good = fs::read(&good_path).unwrap();
    for fault in [
        "position",
        "window",
        "fades",
        "remove-null",
        "locked",
        "restore-position",
        "restore-presence",
        "restore-missing",
        "extent",
    ] {
        let archive_path = f.0.join(format!("{fault}.omniusong-native"));
        fs::write(&archive_path, &good).unwrap();
        let archive = Connection::open(&archive_path).unwrap();
        let operation = if fault.starts_with("restore-") {
            "restored"
        } else {
            "updated"
        };
        let request_fault = matches!(
            fault,
            "position" | "window" | "fades" | "remove-null" | "extent"
        );
        let (json_column, hash_column) = if request_fault {
            ("request_json", "request_sha256")
        } else {
            ("result_json", "result_sha256")
        };
        let original_json: String = archive
            .query_row(
                &format!("SELECT {json_column} FROM operations WHERE operation_id=?1"),
                [operation],
                |r| r.get(0),
            )
            .unwrap();
        let rewritten = if request_fault {
            let mut request: ProjectEditIntent = serde_json::from_str(&original_json).unwrap();
            let EditOperation::UpdateClip {
                patch: ClipPatch::NativeAudio(patch),
                timeline,
            } = &mut request.edit
            else {
                panic!("Expected native update")
            };
            match fault {
                "position" => patch.position_frame = ValueChange::Set { value: 480_001 },
                "window" => {
                    patch.source_window = ValueChange::Set {
                        value: AudioSourceWindow {
                            start_frame: 102,
                            frame_count: 4,
                        },
                    }
                }
                "fades" => {
                    patch.fades = FieldChange::Set {
                        value: AudioFades {
                            in_frames: 1,
                            out_frames: 1,
                            curve: FadeCurve::Linear,
                        },
                    }
                }
                "remove-null" => patch.fades = FieldChange::Remove,
                "extent" => {
                    *timeline = ClipTimelinePolicy::ExtendLoop {
                        allow_native_schema_upgrade: false,
                    }
                }
                _ => unreachable!(),
            }
            serde_json::to_string(&request).unwrap()
        } else {
            let mut result = ProjectDocument::from_json(original_json.as_bytes())
                .unwrap()
                .value()
                .clone();
            let clip = &mut result["tracks"][2]["recording"]["clips"][0];
            match fault {
                "locked" => result["tracks"][2]["locked"] = true.into(),
                "restore-position" => clip["position"]["frame"] = 0.into(),
                "restore-presence" => {
                    clip.as_object_mut().unwrap().remove("fades");
                }
                "restore-missing" => {
                    clip.as_object_mut().unwrap().remove("editSource");
                }
                _ => unreachable!(),
            }
            String::from_utf8(
                ProjectDocument::from_value(result)
                    .unwrap()
                    .to_json()
                    .unwrap(),
            )
            .unwrap()
        };
        archive
            .execute(
                &format!(
                    "UPDATE operations SET {json_column}=?1,{hash_column}=?2 WHERE operation_id=?3"
                ),
                params![rewritten, digest(rewritten.as_bytes()), operation],
            )
            .unwrap();
        drop(archive);
        let cause = match fault {
            "position" => "Archived clip placement differs from the explicit request",
            "window" => "Archived clip source window differs from the explicit request",
            "fades" | "remove-null" => "Archived clip field differs from the explicit request",
            "locked" => "Archived clip operation requires an unlocked track of the declared kind",
            "restore-position" | "restore-presence" => {
                "Archived restored clip settings differ from the retained original"
            }
            "restore-missing" => "Archived restore-clip result lost its original snapshot",
            "extent" => "Archived extended loop does not cover the clip extent",
            _ => unreachable!(),
        };
        let mut identity = f.store(&format!("identity-{fault}"));
        let failure = identity
            .restore_archive(&archive_path)
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains(cause),
            "Wrong identity-restore rejection for {fault}: {failure}"
        );
        assert!(identity.list().unwrap().is_empty());
        let mut copy = f.store(&format!("copy-{fault}"));
        let failure = copy
            .restore_archive_as_copy(
                &archive_path,
                &RestoreCopyIntent {
                    operation_id: OperationId(format!("copy-{fault}")),
                    expected_archive_sha256: digest(&fs::read(&archive_path).unwrap()),
                },
            )
            .unwrap_err()
            .to_string();
        assert!(
            failure.contains(cause),
            "Wrong copy rejection for {fault}: {failure}"
        );
        assert!(copy.list().unwrap().is_empty());
    }
    assert_eq!(fs::read(&good_path).unwrap(), good);
}

#[test]
fn rehashed_legacy_receipt_cannot_change_decimal_digits_or_remove_presence() {
    let f = Fixture::new();
    let mut store = f.store("source");
    let (initial, _, _, _) = imported_legacy(&f, &mut store);
    let edited = store
        .apply_edit(&intent(
            &initial,
            "legacy-edited",
            legacy_address(),
            EditOperation::UpdateClip {
                patch: legacy_patch(),
                timeline: KEEP,
            },
        ))
        .unwrap()
        .document;
    store
        .apply_edit(&intent(
            &edited,
            "legacy-restored",
            legacy_address(),
            EditOperation::RestoreClip { timeline: KEEP },
        ))
        .unwrap();
    let good_path = f.0.join("good-legacy.omniusong-native");
    store.export_archive(initial.id(), &good_path).unwrap();
    let good = fs::read(&good_path).unwrap();
    for fault in ["sub-f64-decimal", "restore-omission"] {
        let archive_path = f.0.join(format!("{fault}.omniusong-native"));
        fs::write(&archive_path, &good).unwrap();
        let archive = Connection::open(&archive_path).unwrap();
        let (operation, json_column, hash_column) = if fault == "sub-f64-decimal" {
            ("legacy-edited", "request_json", "request_sha256")
        } else {
            ("legacy-restored", "result_json", "result_sha256")
        };
        let raw: String = archive
            .query_row(
                &format!("SELECT {json_column} FROM operations WHERE operation_id=?1"),
                [operation],
                |r| r.get(0),
            )
            .unwrap();
        let rewritten = if fault == "sub-f64-decimal" {
            let mut request: ProjectEditIntent = serde_json::from_str(&raw).unwrap();
            let EditOperation::UpdateClip {
                patch: ClipPatch::LegacyAudio(patch),
                ..
            } = &mut request.edit
            else {
                panic!("Expected legacy update")
            };
            let replacement = decimal("0.12345678901234567890123456788");
            assert_eq!(
                replacement.as_f64(),
                decimal("0.12345678901234567890123456789").as_f64()
            );
            patch.trim_start_seconds = FieldChange::Set { value: replacement };
            serde_json::to_string(&request).unwrap()
        } else {
            let mut result = ProjectDocument::from_json(raw.as_bytes())
                .unwrap()
                .value()
                .clone();
            // Effective default is unchanged, but exact original absence matters.
            result["tracks"][0]["audio"]["startTick"] = 0.into();
            String::from_utf8(
                ProjectDocument::from_value(result)
                    .unwrap()
                    .to_json()
                    .unwrap(),
            )
            .unwrap()
        };
        archive
            .execute(
                &format!(
                    "UPDATE operations SET {json_column}=?1,{hash_column}=?2 WHERE operation_id=?3"
                ),
                params![rewritten, digest(rewritten.as_bytes()), operation],
            )
            .unwrap();
        drop(archive);
        let cause = if fault == "sub-f64-decimal" {
            "Archived clip field differs from the explicit request"
        } else {
            "Archived restored clip settings differ from the retained original"
        };
        let mut identity = f.store(&format!("identity-{fault}"));
        let error = identity
            .restore_archive(&archive_path)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(cause),
            "Wrong rejection for {fault}: {error}"
        );
        assert!(identity.list().unwrap().is_empty());
        let mut copy = f.store(&format!("copy-{fault}"));
        let error = copy
            .restore_archive_as_copy(
                &archive_path,
                &RestoreCopyIntent {
                    operation_id: OperationId(format!("copy-{fault}")),
                    expected_archive_sha256: digest(&fs::read(&archive_path).unwrap()),
                },
            )
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(cause),
            "Wrong copy rejection for {fault}: {error}"
        );
        assert!(copy.list().unwrap().is_empty());
    }
}

#[test]
fn rehashed_receipt_original_outside_the_prefix_blocks_replay_and_archive_export() {
    let f = Fixture::new();
    let source = sealed(&f, true);
    let mut store = f.store("source");
    store.ingest_take(&source).unwrap();
    let initial = store.create(&native_document(&source)).unwrap();
    let request = update(&initial, "edited");
    let edited = store.apply_edit(&request).unwrap().document;
    let db = f.db("source");
    let original_json: String = db
        .query_row(
            "SELECT result_json FROM operations WHERE operation_id='edited'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let object_bytes = fs::read(f.object("source", &store)).unwrap();
    for (start, count) in [(99, 8), (100, 9)] {
        let mut value = ProjectDocument::from_json(original_json.as_bytes())
            .unwrap()
            .value()
            .clone();
        value["tracks"][2]["recording"]["clips"][0]["editSource"]["original"]["range"] =
            json!({"unit":"audio_frames","start":start,"count":count});
        let rewritten = String::from_utf8(
            ProjectDocument::from_value(value)
                .unwrap()
                .to_json()
                .unwrap(),
        )
        .unwrap();
        db.execute(
            "UPDATE operations SET result_json=?1,result_sha256=?2 WHERE operation_id='edited'",
            params![rewritten, digest(rewritten.as_bytes())],
        )
        .unwrap();
        let replay = store.apply_edit(&request).unwrap_err().to_string();
        assert!(
            replay.contains("Take clip exceeds the proven original source window"),
            "{replay}"
        );
        let destination =
            f.0.join(format!("invalid-{start}-{count}.omniusong-native"));
        let export = store
            .export_archive(initial.id(), &destination)
            .unwrap_err()
            .to_string();
        assert!(
            export.contains("Take clip exceeds the proven original source window"),
            "{export}"
        );
        assert!(!destination.exists());
        assert_eq!(store.load(initial.id()).unwrap(), edited);
        assert_eq!(fs::read(f.object("source", &store)).unwrap(), object_bytes);
    }
    db.execute(
        "UPDATE operations SET result_json=?1,result_sha256=?2 WHERE operation_id='edited'",
        params![original_json, digest(original_json.as_bytes())],
    )
    .unwrap();
    assert!(store.apply_edit(&request).unwrap().replayed);
}

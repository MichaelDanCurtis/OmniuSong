//! Real-source consumer admission. A metadata summary is deliberately weaker
//! than authenticated load or publication; no native device or UI is exercised.
use super::*;
use omniusong_core::contracts::TrackId;
use omniusong_store::{NativeReader, TakePublication};

const LEGACY_GUARD: &str = "Performance projection ownership requires authenticated publication and archive validation on this path";

fn projection_original_path(fixture: &ArchiveFixture) -> PathBuf {
    let registry = fixture.project.performance_projections().unwrap().unwrap();
    let take = &registry.entries[0].source.take_id.0;
    let object: String = db(&fixture.store)
        .query_row(
            "SELECT object_sha256 FROM native_takes WHERE take_id=?1",
            [take],
            |row| row.get(0),
        )
        .unwrap();
    fixture
        .store
        .root()
        .join("takes")
        .join(format!("{object}.sqlite3"))
}

fn assert_summary(fixture: &ArchiveFixture, document: &ProjectDocument) {
    let rows = fixture.store.list().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, document.id());
    assert_eq!(row.name, document.name());
    assert_eq!(row.revision, document.revision());
    assert_eq!(row.track_count, document.tracks().len());
    let notes = document
        .tracks()
        .iter()
        .filter(|track| track["kind"] != "recording")
        .map(|track| track["notes"].as_array().map_or(0, Vec::len))
        .sum::<usize>();
    assert_eq!(row.note_count, notes);
    assert!(row.problem.is_none());
    assert!(row.source_authentication_required);
    fixture
        .store
        .check_project_metadata_admission(document.id())
        .unwrap();
}

#[test]
fn actual_v1_v2_projection_metadata_remains_readable_with_its_owned_original_absent() {
    for v2 in [false, true] {
        let f = ArchiveFixture::new(v2);
        let before = catalog(&f.store);
        let owned = originals(&f.store);
        let staged = f.original.bytes();
        assert_summary(&f, &f.project);
        let mut reader = NativeReader::open(f.store.root()).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        assert_eq!(
            reader
                .load_document_with_budget(f.project.id(), &governor)
                .unwrap(),
            f.project
        );
        let original = projection_original_path(&f);
        let bytes = fs::read(&original).unwrap();
        let parked = f.root.join("metadata-only-parked-original.bin");
        fs::rename(&original, &parked).unwrap();
        assert!(!original.exists());
        assert_summary(&f, &f.project);
        let (governor, _) = budget(InspectionLimits::default());
        assert_eq!(
            reader
                .read_snapshot(|snapshot| {
                    snapshot.load_document_metadata_with_budget(f.project.id(), &governor)
                })
                .unwrap(),
            f.project
        );
        assert!(
            matches!(reader.read_snapshot(|snapshot| snapshot.load_document(f.project.id())),
            Err(StoreError::Invalid(message)) if message == LEGACY_GUARD)
        );
        let (governor, _) = budget(InspectionLimits::default());
        assert!(
            matches!(primary(&reader.load_document_with_budget(f.project.id(), &governor).unwrap_err()),
            StoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound)
        );
        assert!(
            matches!(primary(&f.store.load(f.project.id()).unwrap_err()),
            StoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound)
        );
        assert_eq!(catalog(&f.store), before);
        assert_eq!(fs::read(&parked).unwrap(), bytes);
        assert!(
            !original.exists(),
            "metadata consumers must not restore or adopt an original"
        );
        assert_eq!(f.original.bytes(), staged);
        fs::rename(&parked, &original).unwrap();
        assert_eq!(originals(&f.store), owned);
        assert_eq!(f.store.load(f.project.id()).unwrap(), f.project);
        let (governor, _) = budget(InspectionLimits::default());
        assert_eq!(
            reader
                .load_document_with_budget(f.project.id(), &governor)
                .unwrap(),
            f.project
        );
        assert_eq!(catalog(&f.store), before);
    }
}

#[test]
fn coherent_false_projection_is_metadata_but_not_source_authenticated_music() {
    let f = ArchiveFixture::new(true);
    let forged = false_pitch(&f.project);
    let value = String::from_utf8(forged.to_json().unwrap()).unwrap();
    let connection = db(&f.store);
    assert_eq!(
        connection
            .execute(
                "UPDATE projects SET payload=?1 WHERE id=?2",
                params![value, f.project.id()]
            )
            .unwrap(),
        1
    );
    drop(connection);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    assert_summary(&f, &forged);
    let mut reader = NativeReader::open(f.store.root()).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    assert_eq!(
        reader
            .read_snapshot(|snapshot| {
                snapshot.load_document_metadata_with_budget(f.project.id(), &governor)
            })
            .unwrap(),
        forged
    );
    let (governor, _) = budget(InspectionLimits::default());
    assert!(
        matches!(primary(&reader.load_document_with_budget(f.project.id(), &governor).unwrap_err()),
        StoreError::Recording(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation")
    );
    assert!(
        matches!(primary(&f.store.load(f.project.id()).unwrap_err()),
        StoreError::Recording(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation")
    );
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn malformed_owned_projection_envelope_stays_a_metadata_problem_without_repair_or_file_work() {
    let f = ArchiveFixture::new(false);
    let mut malformed = f.project.value().clone();
    malformed["performanceProjections"]["version"] = 2.into();
    let expected = ProjectDocument::from_value(malformed.clone())
        .unwrap_err()
        .to_string();
    let payload = serde_json::to_string(&malformed).unwrap();
    let connection = db(&f.store);
    assert_eq!(
        connection
            .execute(
                "UPDATE projects SET payload=?1 WHERE id=?2",
                params![payload, f.project.id()]
            )
            .unwrap(),
        1
    );
    drop(connection);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let rows = f.store.list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, f.project.id());
    assert_eq!(rows[0].revision, f.project.revision());
    assert_eq!(rows[0].problem.as_deref(), Some(expected.as_str()));
    assert!(
        matches!(f.store.check_project_metadata_admission(f.project.id()),
        Err(StoreError::Document(error)) if error.to_string() == expected)
    );
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
}

struct IncomingAudio {
    path: PathBuf,
    reference: TakeReference,
}
const ORDINARY_FRAMES: u64 = 2000;
fn ordinary_audio(fixture: &ArchiveFixture) -> IncomingAudio {
    use omniusong_engine::{
        capture::CaptureJournal,
        realtime::{AudioPacket, MAX_CHANNELS, PACKET_FRAMES},
    };
    let path = fixture.root.join("incoming-ordinary.sqlite3");
    let descriptor = serde_json::from_value(json!({
        "version":1,"take_id":uuid::Uuid::new_v4().to_string(),"streams":[{
            "stream_id":"audio","segment_id":"incoming-segment","source_id":"synthetic-packets",
            "source_name":"Device-free consumer source","source_connection_generation":1,
            "host_clock":{"domain":"consumer-clock","epoch":"consumer-epoch","ticks_per_second_numerator":1000000000,"ticks_per_second_denominator":1},
            "kind":{"kind":"audio","format":{"sample_rate":8000,"channels":1,"encoding":"f32_le","layout":"interleaved"},"audio_epoch":"consumer-epoch"},
            "initial_source_position":0
        }],"provenance":{"synthetic":true,"role":"ordinary audio, no performance namespace"}
    })).unwrap();
    let mut writer = CaptureJournal::create(&path, descriptor).unwrap();
    let mut start = 0u64;
    for index in 0..8 {
        let frames = if index == 7 {
            208
        } else {
            PACKET_FRAMES as u16
        };
        writer
            .append_packet(&AudioPacket {
                start_frame: start,
                frames,
                channels: 1,
                clock_start_ns: start * 125000,
                clock_end_ns: (start + u64::from(frames)) * 125000,
                samples: [0.125; PACKET_FRAMES * MAX_CHANNELS],
            })
            .unwrap();
        start += u64::from(frames);
    }
    assert_eq!(start, ORDINARY_FRAMES);
    drop(writer);
    CaptureReader::open(&path)
        .unwrap()
        .recover("consumer-review.ordinary-recovered")
        .unwrap();
    let source = SealedJournal::open(&path).unwrap();
    assert_eq!(source.descriptor().streams.len(), 1);
    assert!(source.manifest().body.interrupted.is_some());
    let reference = TakeReference {
        take_id: source.descriptor().take_id.clone(),
        manifest_sha256: source.manifest().sha256.clone(),
        stream_id: "audio".into(),
    };
    drop(source);
    IncomingAudio { path, reference }
}
fn take_publication(
    document: &ProjectDocument,
    source: &TakeReference,
    id: &str,
    frames: u64,
) -> TakePublication {
    TakePublication {
        operation_id: OperationId(id.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        track_id: TrackId(format!("{id}-track")),
        track_name: "Another retained original".into(),
        clip: TakeClip {
            id: format!("{id}-clip"),
            source: source.clone(),
            range: TakeRange::AudioFrames {
                start: 0,
                count: frames,
            },
            position: TimelinePosition {
                frame: 321,
                sample_rate: 8000,
            },
            fades: None,
            edit_source: None,
        },
    }
}
fn assert_retained_projection(before: &ProjectDocument, after: &ProjectDocument) {
    assert_eq!(after.schema_version(), 3);
    assert_eq!(
        after.performance_projections().unwrap(),
        before.performance_projections().unwrap()
    );
    assert_eq!(
        after.value()["com.example.retained"],
        before.value()["com.example.retained"]
    );
    for original in before
        .tracks()
        .iter()
        .filter(|track| track["kind"] != "recording")
    {
        assert_eq!(
            after
                .tracks()
                .iter()
                .find(|track| track["id"] == original["id"])
                .unwrap(),
            original
        );
    }
}

#[test]
fn actual_v1_v2_normal_capture_and_take_publication_preserve_projection_through_replay_history_and_copy()
 {
    for v2 in [false, true] {
        let mut f = ArchiveFixture::new(v2);
        let registry = f.project.clone();
        let original_bytes = originals(&f.store);
        let staged = f.original.bytes();
        let incoming = ordinary_audio(&f);
        let incoming_bytes = fs::read(&incoming.path).unwrap();
        let publication = take_publication(&f.project, &incoming.reference, "attach-ordinary", 64);
        let first = f
            .store
            .attach_capture(&incoming.path, &publication)
            .unwrap();
        assert!(!first.replayed);
        assert_eq!(first.previous_revision, f.project.revision());
        assert_eq!(first.document.revision(), f.project.revision() + 1);
        assert_eq!(
            first.document.take_clips().unwrap(),
            vec![publication.clip.clone()]
        );
        assert_retained_projection(&registry, &first.document);
        assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
        let all_originals = originals(&f.store);
        assert_eq!(all_originals.len(), 2);
        for (take, bytes) in &original_bytes {
            assert_eq!(&all_originals[take], bytes);
        }
        let later = f
            .store
            .apply_edit(&edit(
                &first.document,
                "later-consumer-name",
                "Later human name",
            ))
            .unwrap()
            .document;
        let before_replay = catalog(&f.store);
        let parked = f.root.join("incoming-private-parked.bin");
        fs::rename(&incoming.path, &parked).unwrap();
        assert!(!incoming.path.exists());
        let replay = f
            .store
            .attach_capture(&incoming.path, &publication)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.document, first.document);
        assert_eq!(f.store.load(later.id()).unwrap(), later);
        assert_eq!(catalog(&f.store), before_replay);
        assert_eq!(originals(&f.store), all_originals);
        assert_eq!(fs::read(&parked).unwrap(), incoming_bytes);
        let reference = registry.performance_projections().unwrap().unwrap().entries[0]
            .source
            .clone();
        let performance_audio =
            take_publication(&later, &reference, "publish-performance-audio", 80);
        let published = f.store.publish_take(&performance_audio).unwrap();
        assert!(!published.replayed);
        assert_eq!(published.document.revision(), later.revision() + 1);
        assert_retained_projection(&registry, &published.document);
        assert_eq!(
            published.document.take_clips().unwrap(),
            vec![publication.clip.clone(), performance_audio.clip.clone()]
        );
        assert_eq!(originals(&f.store), all_originals);
        let before_replay = catalog(&f.store);
        let replay = f.store.publish_take(&performance_audio).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.document, published.document);
        assert_eq!(catalog(&f.store), before_replay);
        let undone = f
            .store
            .undo(published.document.id(), published.document.revision())
            .unwrap();
        assert_eq!(undone.take_clips().unwrap(), later.take_clips().unwrap());
        assert_retained_projection(&registry, &undone);
        let redone = f.store.redo(undone.id(), undone.revision()).unwrap();
        assert_eq!(
            redone.take_clips().unwrap(),
            published.document.take_clips().unwrap()
        );
        assert_retained_projection(&registry, &redone);
        f.project = redone.clone();
        let before_archive = catalog(&f.store);
        let (archive, receipt) = f.export("consumer-roundtrip.osar");
        let mut copy = NativeStore::open(f.root.join("consumer-copy")).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        let copied = copy
            .restore_archive_as_copy_bounded_outcome(
                &archive,
                &copy_intent(&receipt, "consumer-copy-once"),
                &governor,
            )
            .unwrap()
            .value
            .document;
        assert_copy_document(&redone, &copied);
        assert_eq!(originals(&copy), all_originals);
        assert_eq!(catalog(&f.store), before_archive);
        assert_eq!(originals(&f.store), all_originals);
        assert_eq!(f.original.bytes(), staged);
        assert_eq!(fs::read(&parked).unwrap(), incoming_bytes);
    }
}

#[test]
fn registered_incoming_windows_and_references_refuse_before_any_project_or_operation_write() {
    let mut f = ArchiveFixture::new(true);
    let incoming = ordinary_audio(&f);
    let source = SealedJournal::open(&incoming.path).unwrap();
    f.store.ingest_take(&source).unwrap();
    drop(source);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let incoming_bytes = fs::read(&incoming.path).unwrap();
    let good = take_publication(
        &f.project,
        &incoming.reference,
        "bounded-normal-publication",
        64,
    );
    let mut outside = good.clone();
    outside.clip.range = TakeRange::AudioFrames {
        start: ORDINARY_FRAMES - 1,
        count: 2,
    };
    assert!(
        matches!(primary(&f.store.publish_take(&outside).unwrap_err()), StoreError::Invalid(message)
        if message == "Take clip exceeds the proven original source window")
    );
    assert_eq!(catalog(&f.store), before);
    let mut wrong = good.clone();
    wrong.clip.source.manifest_sha256 = "f".repeat(64);
    assert!(
        matches!(primary(&f.store.publish_take(&wrong).unwrap_err()), StoreError::Invalid(message)
        if message == "Take reference does not match its registered sealed identity")
    );
    assert_eq!(catalog(&f.store), before);
    let mut wrong = good.clone();
    wrong.clip.position.sample_rate = 16000;
    assert!(
        matches!(primary(&f.store.publish_take(&wrong).unwrap_err()), StoreError::Invalid(message)
        if message == "Take source and arrangement sample rates differ; resampling requires an explicit policy")
    );
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
    assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
    let accepted = f.store.publish_take(&good).unwrap();
    assert_eq!(accepted.document.take_clips().unwrap(), vec![good.clip]);
    assert_retained_projection(&f.project, &accepted.document);
    assert_eq!(originals(&f.store), owned);
}

#[test]
fn authenticated_reader_cancellation_and_work_quota_release_its_scope_for_fresh_reuse() {
    let f = ArchiveFixture::new(true);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let mut reader = NativeReader::open(f.store.root()).unwrap();
    let (cancelled, token) = budget(InspectionLimits::default());
    token.store(true, Ordering::Release);
    assert!(matches!(
        primary(
            &reader
                .load_document_with_budget(f.project.id(), &cancelled)
                .unwrap_err()
        ),
        StoreError::Recording(RecordingError::InspectionCancelled)
    ));
    let cancelled_usage = cancelled.usage();
    let (limited, _) = budget(InspectionLimits {
        max_work_bytes: 1,
        ..InspectionLimits::default()
    });
    assert!(matches!(
        primary(
            &reader
                .load_document_with_budget(f.project.id(), &limited)
                .unwrap_err()
        ),
        StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            limit: 1,
            ..
        })
    ));
    let limited_usage = limited.usage();
    let (fresh, _) = budget(InspectionLimits::default());
    assert_eq!(
        reader
            .load_document_with_budget(f.project.id(), &fresh)
            .unwrap(),
        f.project
    );
    assert_eq!(cancelled.usage(), cancelled_usage);
    assert_eq!(limited.usage(), limited_usage);
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn normal_projection_take_publication_uses_the_callers_sticky_governor_and_reuses_the_store_after_stop()
 {
    let mut f = ArchiveFixture::new(false);
    let incoming = ordinary_audio(&f);
    let source = SealedJournal::open(&incoming.path).unwrap();
    f.store.ingest_take(&source).unwrap();
    drop(source);
    let intent = take_publication(&f.project, &incoming.reference, "governed-normal-take", 64);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let incoming_bytes = fs::read(&incoming.path).unwrap();
    let (cancelled, token) = budget(InspectionLimits::default());
    token.store(true, Ordering::Release);
    assert!(matches!(
        primary(
            &f.store
                .publish_take_with_budget(&intent, &cancelled)
                .unwrap_err()
        ),
        StoreError::Recording(RecordingError::InspectionCancelled)
    ));
    let cancelled_usage = cancelled.usage();
    let (limited, _) = budget(InspectionLimits {
        max_work_bytes: 1,
        ..InspectionLimits::default()
    });
    assert!(matches!(
        primary(
            &f.store
                .publish_take_with_budget(&intent, &limited)
                .unwrap_err()
        ),
        StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            limit: 1,
            ..
        })
    ));
    let limited_usage = limited.usage();
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), owned);
    let (fresh, _) = budget(InspectionLimits::default());
    let accepted = f.store.publish_take_with_budget(&intent, &fresh).unwrap();
    assert_eq!(accepted.document.revision(), f.project.revision() + 1);
    assert_eq!(accepted.document.take_clips().unwrap(), vec![intent.clip]);
    assert_retained_projection(&f.project, &accepted.document);
    assert_eq!(cancelled.usage(), cancelled_usage);
    assert_eq!(limited.usage(), limited_usage);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
    assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
}

#[test]
fn a_different_valid_sealed_source_at_the_registered_path_never_becomes_projection_authority() {
    let f = ArchiveFixture::new(true);
    let replacement = Fixture::record(true, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535)]);
        tap.render(writer, 80);
        tap.admit(&[off(1, 1, 2, 0)]);
        tap.render(writer, 2000);
        tap.finish(writer, 64);
    });
    replacement.recover();
    let replacement_bytes = fs::read(&replacement.path).unwrap();
    let replacement_staged = replacement.bytes();
    let (control, _) = budget(InspectionLimits::default());
    let source = SealedJournal::open_bounded(&replacement.path, &control).unwrap();
    let held = AuthenticatedPerformance::open(&source).unwrap();
    assert_ne!(held.source_pin().take_id, f.intent.request.source.take_id);
    drop(held);
    drop(source);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let original = projection_original_path(&f);
    let original_length = fs::metadata(&original).unwrap().len();
    let parked = f.root.join("original-before-substitution.bin");
    fs::rename(&original, &parked).unwrap();
    fs::write(&original, &replacement_bytes).unwrap();
    assert_summary(&f, &f.project);
    let expected = if original_length != replacement_bytes.len() as u64 {
        "Registered source length differs from its catalog object"
    } else {
        "Held source manifest or take differs from its registered object"
    };
    let mut reader = NativeReader::open(f.store.root()).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    assert!(
        matches!(primary(&reader.load_document_with_budget(f.project.id(), &governor).unwrap_err()),
        StoreError::Invalid(message) if message == expected)
    );
    assert!(
        matches!(primary(&f.store.load(f.project.id()).unwrap_err()),
        StoreError::Invalid(message) if message == expected)
    );
    assert_eq!(catalog(&f.store), before);
    assert_eq!(fs::read(&original).unwrap(), replacement_bytes);
    fs::remove_file(&original).unwrap();
    fs::rename(&parked, &original).unwrap();
    let (fresh, _) = budget(InspectionLimits::default());
    assert_eq!(
        reader
            .load_document_with_budget(f.project.id(), &fresh)
            .unwrap(),
        f.project
    );
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
    assert_eq!(replacement.bytes(), replacement_staged);
}

#[test]
fn normal_recording_publication_accepts_valid_compact_stored_payload_as_its_exact_sql_preimage() {
    let mut f = ArchiveFixture::new(true);
    let incoming = ordinary_audio(&f);
    let source = SealedJournal::open(&incoming.path).unwrap();
    f.store.ingest_take(&source).unwrap();
    drop(source);
    let pretty = serde_json::to_string(f.project.value()).unwrap();
    let canonical = String::from_utf8(f.project.to_json().unwrap()).unwrap();
    assert_ne!(pretty, canonical);
    assert_eq!(
        ProjectDocument::from_json(pretty.as_bytes()).unwrap(),
        f.project
    );
    let connection = db(&f.store);
    assert_eq!(
        connection
            .execute(
                "UPDATE projects SET payload=?1 WHERE id=?2",
                params![pretty, f.project.id()]
            )
            .unwrap(),
        1
    );
    let (raw, cursor): (String, i64) = connection
        .query_row(
            "SELECT payload,cursor FROM projects WHERE id=?1",
            [f.project.id()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        raw, pretty,
        "the stored preimage is actually noncanonical before publication"
    );
    drop(connection);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let incoming_bytes = fs::read(&incoming.path).unwrap();
    let intent = take_publication(
        &f.project,
        &incoming.reference,
        "pretty-payload-publication",
        64,
    );
    let receipt = f.store.publish_take(&intent).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.operation_id, intent.operation_id.0);
    assert_eq!(receipt.previous_revision, f.project.revision());
    assert_eq!(receipt.document.revision(), f.project.revision() + 1);
    assert_eq!(
        receipt.document.take_clips().unwrap(),
        vec![intent.clip.clone()]
    );
    assert_retained_projection(&f.project, &receipt.document);
    let after = catalog(&f.store);
    assert_eq!(after.application, before.application);
    assert_eq!(after.version, before.version);
    assert_eq!(after.schema, before.schema);
    assert_eq!(after.tables.len(), before.tables.len());
    for (table, rows) in &before.tables {
        if table != "projects" && table != "edits" && table != "operations" {
            assert_eq!(
                &after.tables[table], rows,
                "unrelated table {table} changed"
            );
        }
    }
    for table in ["edits", "operations"] {
        assert_eq!(after.tables[table].len(), before.tables[table].len() + 1);
        assert!(
            before.tables[table]
                .iter()
                .all(|row| after.tables[table].contains(row))
        );
    }
    let connection = db(&f.store);
    let (payload, revision, next_cursor): (String, i64, i64) = connection
        .query_row(
            "SELECT payload,revision,cursor FROM projects WHERE id=?1",
            [f.project.id()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let after_json = String::from_utf8(receipt.document.to_json().unwrap()).unwrap();
    assert_eq!(payload, after_json);
    assert_eq!(revision as u64, receipt.document.revision());
    assert_eq!(next_cursor, cursor + 1);
    let (history_before, history_after): (String, String) = connection
        .query_row(
            "SELECT before_json,after_json FROM edits WHERE project_id=?1 AND sequence=?2",
            params![f.project.id(), next_cursor],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(history_before, canonical);
    assert_eq!(history_after, after_json);
    let (request, request_sha, result, result_sha): (String, String, String, String) = connection.query_row(
        "SELECT request_json,request_sha256,result_json,result_sha256 FROM operations WHERE operation_id=?1",
        [&intent.operation_id.0], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&request).unwrap(),
        json!({"kind":"take_publication","version":1,"intent":intent})
    );
    assert_eq!(request_sha, sha(request.as_bytes()));
    assert_eq!(result, after_json);
    assert_eq!(result_sha, sha(result.as_bytes()));
    drop(connection);
    assert_eq!(f.store.load(f.project.id()).unwrap(), receipt.document);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
    assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
}

#[test]
fn selected_native_waveform_on_a_real_projection_song_authenticates_only_the_selected_audio() {
    use omniusong_core::{AudioSourceWindow, DocumentClipAddress};
    use omniusong_engine::{
        WaveformBin, WaveformRequest, WaveformSettings, WaveformSourceKey,
        waveform::prepare_waveform,
    };
    let mut f = ArchiveFixture::new(false);
    let incoming = ordinary_audio(&f);
    let incoming_bytes = fs::read(&incoming.path).unwrap();
    let publication = take_publication(&f.project, &incoming.reference, "selected-waveform", 64);
    let attached = f
        .store
        .attach_capture(&incoming.path, &publication)
        .unwrap()
        .document;
    assert_retained_projection(&f.project, &attached);
    let before = catalog(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let selected = &owned[&incoming.reference.take_id.0];
    let request = WaveformRequest {
        project_id: attached.id().into(),
        expected_revision: attached.revision(),
        address: DocumentClipAddress::NativeAudio {
            track_id: publication.track_id.0.clone(),
            clip_id: publication.clip.id.clone(),
        },
        source: WaveformSourceKey::NativeAudio {
            source: incoming.reference.clone(),
            sample_rate: 8000,
        },
        bins: 4,
    };
    let result = prepare_waveform(f.store.root(), &request, || false).unwrap();
    result.validate().unwrap();
    assert_eq!(result.request, request);
    assert_eq!(result.authenticated_sha256, selected.object_sha);
    assert_eq!(result.authenticated_sha256, sha(&selected.bytes));
    assert_eq!((result.sample_rate, result.channels), (8000, 1));
    assert_eq!(
        (result.source_start_frame, result.frame_count),
        (0, ORDINARY_FRAMES)
    );
    assert!(result.unknown_tail);
    assert_eq!(
        result.channel_bins,
        vec![
            (0..4)
                .map(|index| WaveformBin {
                    start_frame: index * 500,
                    frame_count: 500,
                    content_frames: 500,
                    gap_frames: 0,
                    min: Some(0.125),
                    max: Some(0.125),
                })
                .collect::<Vec<_>>()
        ]
    );
    assert_eq!(
        result.saved,
        WaveformSettings::Native {
            window: AudioSourceWindow {
                start_frame: 0,
                frame_count: 64
            },
            position_frame: 321,
            fades: None,
        }
    );
    assert!(result.original.is_none());
    assert_eq!(
        prepare_waveform(f.store.root(), &request, || true).unwrap_err(),
        "Waveform cancelled"
    );
    assert_eq!(catalog(&f.store), before);
    assert_eq!(originals(&f.store), owned);
    // A coherent false note derivation elsewhere in the document remains a
    // metadata claim. It cannot invalidate independent peaks for this actual
    // selected audio, and those peaks cannot authenticate the false music.
    let forged = false_pitch(&attached);
    let payload = String::from_utf8(forged.to_json().unwrap()).unwrap();
    let connection = db(&f.store);
    assert_eq!(
        connection
            .execute(
                "UPDATE projects SET payload=?1 WHERE id=?2",
                params![payload, attached.id()]
            )
            .unwrap(),
        1
    );
    drop(connection);
    let false_before = catalog(&f.store);
    assert_eq!(
        prepare_waveform(f.store.root(), &request, || false).unwrap(),
        result
    );
    assert!(matches!(primary(&f.store.load(attached.id()).unwrap_err()),
        StoreError::Recording(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation"));
    assert_eq!(catalog(&f.store), false_before);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
    assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
}

#[test]
fn actual_recording_publication_rolls_back_a_trigger_substituting_an_unrelated_valid_project() {
    let mut f = ArchiveFixture::new(true);
    let unrelated = f
        .store
        .create(&ProjectDocument::new("Unrelated human song").unwrap())
        .unwrap();
    let mut changed = unrelated.value().clone();
    changed["name"] = "Coherent foreign replacement".into();
    let coherent = ProjectDocument::from_value(changed).unwrap();
    assert_eq!(coherent.id(), unrelated.id());
    assert_eq!(coherent.revision(), unrelated.revision());
    let incoming = ordinary_audio(&f);
    let source = SealedJournal::open(&incoming.path).unwrap();
    f.store.ingest_take(&source).unwrap();
    drop(source);
    let publication = take_publication(&f.project, &incoming.reference, "foreign-row-trigger", 64);
    let connection = db(&f.store);
    let substituted: String = connection.query_row(
        "SELECT json_set(payload,'$.name','Coherent foreign replacement') FROM projects WHERE id=?1",
        [unrelated.id()], |row| row.get(0),
    ).unwrap();
    assert_eq!(
        ProjectDocument::from_json(substituted.as_bytes()).unwrap(),
        coherent
    );
    connection.execute_batch(&format!(
        "CREATE TRIGGER consumer_unrelated_project AFTER INSERT ON operations WHEN NEW.operation_id='{}' BEGIN UPDATE projects SET payload=json_set(payload,'$.name','Coherent foreign replacement') WHERE id='{}'; END;",
        publication.operation_id.0, unrelated.id(),
    )).unwrap();
    drop(connection);
    let before = catalog(&f.store);
    assert_eq!(
        before.tables.len(),
        13,
        "the oracle includes every persistent native catalog table"
    );
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let incoming_bytes = fs::read(&incoming.path).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    assert!(
        matches!(primary(&f.store.publish_take_with_budget(&publication, &governor).unwrap_err()),
        StoreError::Invalid(message) if message == "Final SQL rows differ from the frozen expected postimage")
    );
    let failed_usage = governor.usage();
    assert_eq!(
        catalog(&f.store),
        before,
        "publication, history, operation and unrelated row all roll back"
    );
    assert_eq!(f.store.load(unrelated.id()).unwrap(), unrelated);
    assert_eq!(f.store.load(f.project.id()).unwrap(), f.project);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
    assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
    db(&f.store)
        .execute_batch("DROP TRIGGER consumer_unrelated_project")
        .unwrap();
    let (fresh, _) = budget(InspectionLimits::default());
    let accepted = f
        .store
        .publish_take_with_budget(&publication, &fresh)
        .unwrap();
    assert!(!accepted.replayed);
    assert_eq!(accepted.document.revision(), f.project.revision() + 1);
    assert_eq!(
        accepted.document.take_clips().unwrap(),
        vec![publication.clip]
    );
    assert_retained_projection(&f.project, &accepted.document);
    assert_eq!(f.store.load(unrelated.id()).unwrap(), unrelated);
    assert_eq!(governor.usage(), failed_usage);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
    assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
}

#[test]
fn independently_frozen_operation_receipt_rejects_coherent_result_substitution_and_ignored_insert()
{
    for substitute in [true, false] {
        let mut f = ArchiveFixture::new(true);
        let incoming = ordinary_audio(&f);
        let source = SealedJournal::open(&incoming.path).unwrap();
        f.store.ingest_take(&source).unwrap();
        drop(source);
        let publication =
            take_publication(&f.project, &incoming.reference, "receipt-postimage", 64);
        let mut planned = f.project.clone();
        planned
            .append_take_clip(
                &publication.track_id.0,
                &publication.track_name,
                publication.clip.clone(),
            )
            .unwrap();
        let planned = planned.with_revision(f.project.revision() + 1).unwrap();
        let mut wrong = planned.clone();
        wrong
            .rename("Coherent but unauthorized receipt name")
            .unwrap();
        let wrong_json = String::from_utf8(wrong.to_json().unwrap()).unwrap();
        assert_eq!(wrong.id(), planned.id());
        assert_eq!(wrong.revision(), planned.revision());
        assert_eq!(wrong.take_clips().unwrap(), planned.take_clips().unwrap());
        assert_ne!(wrong, planned);
        let before_trigger = catalog(&f.store);
        let trigger = if substitute {
            format!(
                "CREATE TRIGGER consumer_receipt AFTER INSERT ON operations WHEN NEW.operation_id='{}' BEGIN UPDATE operations SET result_json='{}',result_sha256='{}' WHERE operation_id=NEW.operation_id; END;",
                publication.operation_id.0,
                wrong_json.replace('\'', "''"),
                sha(wrong_json.as_bytes())
            )
        } else {
            format!(
                "CREATE TRIGGER consumer_receipt BEFORE INSERT ON operations WHEN NEW.operation_id='{}' BEGIN SELECT RAISE(IGNORE); END;",
                publication.operation_id.0
            )
        };
        db(&f.store).execute_batch(&trigger).unwrap();
        let before = catalog(&f.store);
        assert_eq!(before.tables.len(), 13);
        let owned = originals(&f.store);
        let original = f.original.bytes();
        let incoming_bytes = fs::read(&incoming.path).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        let error = f
            .store
            .publish_take_with_budget(&publication, &governor)
            .unwrap_err();
        assert!(
            matches!(primary(&error), StoreError::Invalid(message) if message == "Final SQL rows differ from the frozen expected postimage"),
            "substitute={substitute}: {error}"
        );
        let stopped_usage = governor.usage();
        assert_eq!(catalog(&f.store), before);
        assert!(
            f.store
                .take_publication_receipt(&publication)
                .unwrap()
                .is_none()
        );
        assert_eq!(f.store.load(f.project.id()).unwrap(), f.project);
        assert_eq!(originals(&f.store), owned);
        assert_eq!(f.original.bytes(), original);
        assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
        db(&f.store)
            .execute_batch("DROP TRIGGER consumer_receipt")
            .unwrap();
        assert_eq!(catalog(&f.store), before_trigger);
        let (fresh, _) = budget(InspectionLimits::default());
        let accepted = f
            .store
            .publish_take_with_budget(&publication, &fresh)
            .unwrap();
        assert!(!accepted.replayed);
        assert_eq!(accepted.document, planned);
        assert_eq!(f.store.load(f.project.id()).unwrap(), planned);
        assert_eq!(governor.usage(), stopped_usage);
        assert_eq!(originals(&f.store), owned);
        assert_eq!(f.original.bytes(), original);
        assert_eq!(fs::read(&incoming.path).unwrap(), incoming_bytes);
    }
}

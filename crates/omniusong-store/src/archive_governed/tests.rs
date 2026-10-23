use super::*;
use crate::{RestoreCopyIntent, archive_boundary::TestEvent};
use omniusong_core::contracts::OperationId;
use omniusong_recording::{InspectionLimits, RecordingError};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/governed-archive-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn store(&self, name: &str) -> NativeStore {
        NativeStore::open(self.0.join(name)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Governed archive retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn budget() -> InspectionBudget {
    crate::projection_publication::default_budget()
}

fn ordinary_audio(fixture: &Fixture, store: &mut NativeStore) -> (ProjectDocument, PathBuf) {
    use omniusong_core::{
        contracts::{ProjectId, TakeId, TrackId},
        native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
    };
    use omniusong_recording::{
        ChunkHeader, ChunkKind, ClockDomainId, ClockEpochId, HostClock, HostTimestamp, Journal,
        PcmEncoding, PcmFormat, PcmLayout, SealedJournal, SourceRange, StreamDescriptor,
        StreamKind, TakeDescriptor,
    };
    let kind = StreamKind::Audio {
        format: PcmFormat {
            sample_rate: 48_000,
            channels: 1,
            encoding: PcmEncoding::S16Le,
            layout: PcmLayout::Interleaved,
        },
        audio_epoch: ClockEpochId("audio".into()),
    };
    let descriptor = TakeDescriptor {
        version: 1,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            source_id: "software".into(),
            source_name: "Synthetic archive boundary audio".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("boot".into()),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            kind: kind.clone(),
            initial_source_position: 0,
        }],
        provenance: Default::default(),
    };
    let clock = HostTimestamp {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("boot".into()),
        ticks: 0,
    };
    let path = fixture.0.join("ordinary-audio.sqlite3");
    let mut journal = Journal::create(&path, &descriptor).unwrap();
    journal
        .append(
            &ChunkHeader {
                version: 1,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                sequence: 0,
                range: SourceRange::AudioFrames { start: 0, count: 2 },
                interpretation: kind,
                observed_start: clock.clone(),
                observed_end: clock,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &[1, 0, 2, 0],
        )
        .unwrap();
    journal.seal("software-seal").unwrap();
    drop(journal);
    let source = SealedJournal::open(&path).unwrap();
    let object = store.ingest_take(&source).unwrap();
    let project = store
        .create(&ProjectDocument::new("Held original").unwrap())
        .unwrap();
    let receipt = store
        .publish_take(&crate::TakePublication {
            operation_id: OperationId("ordinary-audio-publication".into()),
            project_id: ProjectId(project.id().into()),
            expected_revision: 0,
            track_id: TrackId("audio-track".into()),
            track_name: "Original audio".into(),
            clip: TakeClip {
                edit_source: None,
                id: "audio-clip".into(),
                source: TakeReference {
                    take_id: object.take_id,
                    manifest_sha256: object.manifest_sha256,
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames { start: 0, count: 2 },
                position: TimelinePosition {
                    frame: 0,
                    sample_rate: 48_000,
                },
                fades: None,
            },
        })
        .unwrap();
    (
        receipt.document,
        object_path(&store.root, &object.object_sha256).unwrap(),
    )
}

#[test]
fn all_governed_routes_recheck_the_held_final_owned_path_after_the_publication_hook() {
    let fixture = Fixture::new();
    let mut source = fixture.store("source");
    let (project, owned) = ordinary_audio(&fixture, &mut source);
    let source_bytes = fs::read(&owned).unwrap();
    let input = fixture.0.join("source.osar");
    let receipt = source
        .export_archive_bounded_outcome(project.id(), &input, &budget())
        .unwrap()
        .value;
    for action in [
        ArchiveAction::Export,
        ArchiveAction::Restore,
        ArchiveAction::Copy,
    ] {
        let mut target = fixture.store(&format!("target-{action:?}"));
        let target_root = target.root.clone();
        let source_path = owned.clone();
        let parked = fixture.0.join(format!("parked-{action:?}.sqlite3"));
        let parked_for_hook = parked.clone();
        let destination = fixture.0.join("replaced-export.osar");
        let before = if action == ArchiveAction::Export {
            fs::read(source.root.join("studio.sqlite3")).unwrap()
        } else {
            fs::read(target.root.join("studio.sqlite3")).unwrap()
        };
        let failure = boundary::with_test_hook(
            Box::new(move |event, _| {
                let expected = match action {
                    ArchiveAction::Export => TestEvent::OutputLink,
                    ArchiveAction::Restore => TestEvent::RestoreCommit,
                    ArchiveAction::Copy => TestEvent::CopyCommit,
                    _ => unreachable!(),
                };
                if event == expected {
                    let path = if action == ArchiveAction::Export {
                        source_path.clone()
                    } else {
                        fs::read_dir(target_root.join("takes"))?
                            .next()
                            .unwrap()?
                            .path()
                    };
                    fs::rename(&path, &parked_for_hook)?;
                    fs::copy(&parked_for_hook, &path)?;
                }
                Ok(())
            }),
            || match action {
                ArchiveAction::Export => source
                    .export_archive_bounded_outcome(project.id(), &destination, &budget())
                    .map(|_| ()),
                ArchiveAction::Restore => target
                    .restore_archive_bounded_outcome(&input, &budget())
                    .map(|_| ()),
                ArchiveAction::Copy => target
                    .restore_archive_as_copy_bounded_outcome(
                        &input,
                        &RestoreCopyIntent {
                            operation_id: OperationId("final-path-copy".into()),
                            expected_archive_sha256: receipt.sha256.clone(),
                        },
                        &budget(),
                    )
                    .map(|_| ()),
                _ => unreachable!(),
            },
        )
        .unwrap_err();
        assert!(
            parked.is_file(),
            "publication hook was not reached: {failure:?}"
        );
        assert!(failure.publication_uncertain());
        assert_eq!(failure.catalog_sqlite_error_code(), None);
        assert_eq!(fs::read(&parked).unwrap(), source_bytes);
        if action == ArchiveAction::Export {
            assert!(!destination.exists());
            assert_eq!(
                fs::read(source.root.join("studio.sqlite3")).unwrap(),
                before
            );
            fs::remove_file(&owned).unwrap();
            fs::rename(&parked, &owned).unwrap();
        } else {
            assert!(target.connection.is_autocommit());
            assert_eq!(
                fs::read(target.root.join("studio.sqlite3")).unwrap(),
                before
            );
            assert!(target.list().unwrap().is_empty());
            assert_eq!(
                target
                    .connection
                    .query_row("SELECT count(*) FROM native_takes", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                target
                    .restore_archive_bounded_outcome(&input, &budget())
                    .unwrap()
                    .value,
                project
            );
        }
    }
    assert_eq!(fs::read(&owned).unwrap(), source_bytes);
}

#[test]
fn governed_export_rejects_valid_but_unplanned_output_rows_before_link() {
    let fixture = Fixture::new();
    let mut source = fixture.store("source");
    let project = source
        .create(&ProjectDocument::new("Frozen export").unwrap())
        .unwrap();
    let before = fs::read(source.root.join("studio.sqlite3")).unwrap();
    let destination = fixture.0.join("altered.osar");
    let failure = boundary::with_test_hook(Box::new(|event, context| {
        if event==TestEvent::ExportSchema {
            let output = Connection::open(context.staging.as_ref().unwrap().join("archive.sqlite3")).unwrap();
            output.execute_batch("CREATE TRIGGER alter_export AFTER INSERT ON projects BEGIN UPDATE projects SET payload=json_set(payload,'$.lyrics','valid but unplanned'); END;").unwrap();
        }
        Ok(())
    }), ||source.export_archive_bounded_outcome(project.id(), &destination, &budget())).unwrap_err();
    assert!(
        matches!(failure.operation_error(), StoreError::Invalid(message) if message=="Final SQL rows differ from the frozen expected postimage")
    );
    assert_eq!(failure.catalog_sqlite_error_code(), None);
    assert!(!destination.exists());
    assert_eq!(
        fs::read(source.root.join("studio.sqlite3")).unwrap(),
        before
    );
    assert!(source.connection.is_autocommit());
    assert_eq!(
        source
            .export_archive_bounded_outcome(project.id(), &destination, &budget())
            .unwrap()
            .completion,
        ArchiveCompletion::ExportDurable
    );
}
#[test]
fn bounded_ordinary_export_identity_and_two_copy_generations_are_complete() {
    let fixture = Fixture::new();
    let mut source = fixture.store("source");
    let project = source
        .create(&ProjectDocument::new("Governed ordinary original").unwrap())
        .unwrap();
    let archive = fixture.0.join("complete.osar");
    let exported = source
        .export_archive_bounded_outcome(project.id(), &archive, &budget())
        .unwrap();
    assert_eq!(exported.completion, ArchiveCompletion::ExportDurable);
    assert!(exported.cleanup.is_empty());
    let mut identity = fixture.store("identity");
    let restored = identity
        .restore_archive_bounded_outcome(&archive, &budget())
        .unwrap();
    assert_eq!(restored.value, project);
    assert_eq!(restored.completion, ArchiveCompletion::CatalogCommitted);
    assert!(restored.cleanup.is_empty());
    assert_eq!(identity.load(project.id()).unwrap(), project);
    let mut copy = fixture.store("copy");
    let intent = RestoreCopyIntent {
        operation_id: OperationId("bounded-copy-one".into()),
        expected_archive_sha256: exported.value.sha256,
    };
    let copied = copy
        .restore_archive_as_copy_bounded_outcome(&archive, &intent, &budget())
        .unwrap();
    let copied_path = fixture.0.join("copy.osar");
    let copied_export = copy
        .export_archive_bounded_outcome(copied.value.document.id(), &copied_path, &budget())
        .unwrap();
    let mut second = fixture.store("second");
    let second = second
        .restore_archive_as_copy_bounded_outcome(
            &copied_path,
            &RestoreCopyIntent {
                operation_id: OperationId("bounded-copy-two".into()),
                expected_archive_sha256: copied_export.value.sha256,
            },
            &budget(),
        )
        .unwrap();
    assert_ne!(second.value.document.id(), copied.value.document.id());
    fs::remove_file(&archive).unwrap();
    let replay = copy
        .restore_archive_as_copy_bounded_outcome(&archive, &intent, &budget())
        .unwrap();
    assert_eq!(replay.completion, ArchiveCompletion::RecordedReplay);
    assert_eq!(replay.value.document, copied.value.document);
}
#[test]
fn bounded_export_initial_stop_keeps_catalog_and_destination_unchanged() {
    let fixture = Fixture::new();
    let mut source = fixture.store("source");
    let project = source
        .create(&ProjectDocument::new("Retained").unwrap())
        .unwrap();
    let before = fs::read(source.root.join("studio.sqlite3")).unwrap();
    let token = Arc::new(AtomicBool::new(true));
    let budget = InspectionBudget::new(InspectionLimits::default(), token).unwrap();
    let destination = fixture.0.join("absent.osar");
    let failure = source
        .export_archive_bounded_outcome(project.id(), &destination, &budget)
        .unwrap_err();
    assert!(matches!(
        failure.governor_stop(),
        Some(RecordingError::InspectionCancelled)
    ));
    assert!(!destination.exists());
    assert_eq!(
        fs::read(source.root.join("studio.sqlite3")).unwrap(),
        before
    );
    assert!(source.connection.is_autocommit());
}
#[test]
fn late_export_cancel_after_link_sync_does_not_erase_durable_completion() {
    let fixture = Fixture::new();
    let mut source = fixture.store("source");
    let project = source
        .create(&ProjectDocument::new("Durable").unwrap())
        .unwrap();
    let token = Arc::new(AtomicBool::new(false));
    let budget = InspectionBudget::new(InspectionLimits::default(), Arc::clone(&token)).unwrap();
    let destination = fixture.0.join("durable.osar");
    let outcome = boundary::with_test_hook(
        Box::new(move |event, _| {
            if event == TestEvent::ExportSync {
                token.store(true, Ordering::Release);
            }
            Ok(())
        }),
        || source.export_archive_bounded_outcome(project.id(), &destination, &budget),
    )
    .unwrap();
    assert_eq!(outcome.completion, ArchiveCompletion::ExportDurable);
    assert!(destination.is_file());
    assert!(budget.checkpoint().is_err());
}

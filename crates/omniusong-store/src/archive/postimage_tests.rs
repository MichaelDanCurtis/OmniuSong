use super::*;
use omniusong_core::{
    contracts::{OperationId, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use rusqlite::types::Value;
use std::io;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/archive-postimage-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn store(&self, name: &str) -> NativeStore {
        NativeStore::open(self.0.join(name)).unwrap()
    }
    fn source(&self) -> (NativeStore, ProjectDocument, PathBuf) {
        let mut source = self.store("source");
        let project = source
            .create(&ProjectDocument::new("Immutable input").unwrap())
            .unwrap();
        let path = self.0.join("input.osar");
        source.export_archive(project.id(), &path).unwrap();
        (source, project, path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Archive postimage fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

// Include every persistent table and schema object. A same-count replacement
// or an unrelated local row must not disappear behind project-only checks.
fn snapshot(connection: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
    let mut output = Vec::new();
    let schema = "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name";
    let mut statement = connection.prepare(schema).unwrap();
    let schema_rows = statement
        .query_map([], |row| (0..4).map(|index| row.get(index)).collect())
        .unwrap()
        .collect::<rusqlite::Result<Vec<Vec<Value>>>>()
        .unwrap();
    output.push(("schema".into(), schema_rows));
    let names = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for name in names {
        let mut statement = connection
            .prepare(&format!(
                "SELECT * FROM \"{name}\" NOT INDEXED ORDER BY rowid"
            ))
            .unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
            .unwrap()
            .collect::<rusqlite::Result<Vec<Vec<Value>>>>()
            .unwrap();
        output.push((name, rows));
    }
    output
}

fn expect_postimage_failure(failure: &ArchiveFailure) {
    assert!(
        failure
            .operation_error()
            .to_string()
            .to_ascii_lowercase()
            .contains("postimage"),
        "{failure:?}"
    );
    assert!(!failure.publication_uncertain());
    assert!(!failure.catalog_connection_unusable());
}

#[test]
fn identity_postimage_refuses_semantically_valid_project_replacement_and_retries() {
    let fixture = Fixture::new();
    let (source, project, path) = fixture.source();
    let original = fs::read(&path).unwrap();
    let mut target = fixture.store("target");
    target.connection.execute_batch("CREATE TRIGGER valid_replacement AFTER INSERT ON projects BEGIN UPDATE projects SET payload=json_set(payload,'$.name','Trigger replacement') WHERE id=NEW.id; END;").unwrap();
    let before = snapshot(&target.connection);
    let failure = target.restore_archive_outcome(&path).unwrap_err();
    expect_postimage_failure(&failure);
    assert_eq!(snapshot(&target.connection), before);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(source.load(project.id()).unwrap(), project);
    assert!(target.connection.is_autocommit());
    target
        .connection
        .execute_batch("DROP TRIGGER valid_replacement")
        .unwrap();
    assert_eq!(target.restore_archive(&path).unwrap(), project);
}

#[test]
fn identity_postimage_refuses_same_count_unrelated_row_swap() {
    let fixture = Fixture::new();
    let (_source, project, path) = fixture.source();
    let mut target = fixture.store("target");
    let first = target
        .create(&ProjectDocument::new("First unrelated").unwrap())
        .unwrap();
    let second = target
        .create(&ProjectDocument::new("Second unrelated").unwrap())
        .unwrap();
    let first_id = first.id().replace('\'', "''");
    let second_id = second.id().replace('\'', "''");
    target.connection.execute_batch(&format!("CREATE TRIGGER unrelated_swap AFTER INSERT ON projects WHEN NEW.id='{}' BEGIN UPDATE projects SET payload=json_set(payload,'$.name',CASE id WHEN '{first_id}' THEN 'Second unrelated' ELSE 'First unrelated' END) WHERE id IN ('{first_id}','{second_id}'); END;", project.id())).unwrap();
    let before = snapshot(&target.connection);
    expect_postimage_failure(&target.restore_archive_outcome(&path).unwrap_err());
    assert_eq!(snapshot(&target.connection), before);
    assert_eq!(target.load(first.id()).unwrap(), first);
    assert_eq!(target.load(second.id()).unwrap(), second);
}

#[test]
fn identity_postimage_refuses_unplanned_local_intent() {
    let fixture = Fixture::new();
    let (_source, _project, path) = fixture.source();
    let mut target = fixture.store("target");
    target.connection.execute_batch("CREATE TRIGGER unexpected_intent AFTER INSERT ON projects BEGIN INSERT INTO recording_intents VALUES('postimage-intent',1,x'7b7d','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'); END;").unwrap();
    let before = snapshot(&target.connection);
    expect_postimage_failure(&target.restore_archive_outcome(&path).unwrap_err());
    assert_eq!(snapshot(&target.connection), before);
}

#[test]
fn identity_restore_consumes_the_acquired_archive_when_original_path_is_replaced() {
    let fixture = Fixture::new();
    let (_source, project, path) = fixture.source();
    let original = fs::read(&path).unwrap();
    let mut other = fixture.store("other");
    let other_project = other
        .create(&ProjectDocument::new("Replacement input").unwrap())
        .unwrap();
    let replacement = fixture.0.join("replacement.osar");
    other
        .export_archive(other_project.id(), &replacement)
        .unwrap();
    let replacement_bytes = fs::read(&replacement).unwrap();
    let changed_path = path.clone();
    let moved_replacement = replacement.clone();
    let mut target = fixture.store("target");
    let restored = boundary::with_test_hook(
        Box::new(move |event, context| {
            if event == boundary::TestEvent::RestoreAcquired {
                assert_eq!(
                    context.archive_sha256.as_deref(),
                    Some(checksum(&original).as_str())
                );
                fs::rename(&moved_replacement, &changed_path)?;
            }
            Ok(())
        }),
        || target.restore_archive_outcome(&path),
    )
    .unwrap();
    assert_eq!(restored.value, project);
    assert_eq!(target.load(project.id()).unwrap(), project);
    assert_eq!(count(&target.connection, "projects").unwrap(), 1);
    assert_eq!(fs::read(&path).unwrap(), replacement_bytes);
}

#[test]
fn export_postimage_refuses_coherent_payload_and_hash_replacement_before_publication() {
    let fixture = Fixture::new();
    let (source, project, _input) = fixture.source();
    let mut tampered = project.value().clone();
    tampered["name"] = "Valid but independently unplanned".into();
    let payload = serde_json::to_string(&tampered).unwrap();
    ProjectDocument::from_value(tampered).unwrap();
    let sha256 = checksum(payload.as_bytes());
    let sql = format!(
        "CREATE TRIGGER coherent_replacement AFTER INSERT ON projects BEGIN UPDATE projects SET payload='{}',payload_sha256='{sha256}' WHERE id=NEW.id; END;",
        payload.replace('\'', "''")
    );
    let destination = fixture.0.join("never-published.osar");
    let failure = boundary::with_test_hook(
        Box::new(move |event, context| {
            if event == boundary::TestEvent::ExportSchema {
                let connection =
                    Connection::open(context.staging.as_ref().unwrap().join("archive.sqlite3"))
                        .map_err(io::Error::other)?;
                connection.execute_batch(&sql).map_err(io::Error::other)?;
            }
            Ok(())
        }),
        || source.export_archive_outcome(project.id(), &destination),
    )
    .unwrap_err();
    expect_postimage_failure(&failure);
    assert!(!destination.exists());
    assert!(
        failure
            .context()
            .staging
            .as_ref()
            .unwrap()
            .join("archive.sqlite3")
            .is_file()
    );
    assert_eq!(source.load(project.id()).unwrap(), project);
}

#[test]
fn unsupported_persistent_catalog_table_refuses_restore_before_placement() {
    let fixture = Fixture::new();
    let (_source, _project, path) = fixture.source();
    let mut target = fixture.store("target");
    target.connection.execute_batch("CREATE TABLE opaque_vendor_rows(id TEXT PRIMARY KEY,payload BLOB); INSERT INTO opaque_vendor_rows VALUES('keep',x'000102');").unwrap();
    let before = snapshot(&target.connection);
    let failure = target.restore_archive_outcome(&path).unwrap_err();
    expect_postimage_failure(&failure);
    assert!(
        failure
            .operation_error()
            .to_string()
            .contains("unknown persistent")
    );
    assert_eq!(failure.context().published_objects, 0);
    assert_eq!(snapshot(&target.connection), before);
}

#[test]
fn export_postimage_refuses_coherently_rehashed_native_chunks() {
    let fixture = Fixture::new();
    let descriptor = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("postimage-native-original".into()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            source_id: "synthetic".into(),
            source_name: "Device-free source".into(),
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
                    channels: 2,
                    encoding: PcmEncoding::S16Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: ClockEpochId("audio".into()),
            },
            initial_source_position: 0,
        }],
        provenance: Default::default(),
    };
    let journal_path = fixture.0.join("original.sqlite3");
    let mut journal = Journal::create(&journal_path, &descriptor).unwrap();
    let stamp = HostTimestamp {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("boot".into()),
        ticks: 1,
    };
    journal
        .append(
            &ChunkHeader {
                version: CHUNK_VERSION,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                sequence: 0,
                range: SourceRange::AudioFrames { start: 0, count: 1 },
                interpretation: descriptor.streams[0].kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &[1, 2, 3, 4],
        )
        .unwrap();
    drop(journal);
    let mut journal = Journal::open(&journal_path).unwrap();
    journal.seal_recovered("postimage-recovery").unwrap();
    drop(journal);
    let original = SealedJournal::open(&journal_path).unwrap();
    let original_bytes = fs::read(&journal_path).unwrap();
    let mut source = fixture.store("source");
    let project = source
        .create(&ProjectDocument::new("Original registration").unwrap())
        .unwrap();
    source.ingest_take(&original).unwrap();
    let project = source
        .publish_take(&crate::TakePublication {
            operation_id: OperationId("postimage-publish".into()),
            project_id: ProjectId(project.id().into()),
            expected_revision: 0,
            track_id: TrackId("original-audio".into()),
            track_name: "Original".into(),
            clip: TakeClip {
                edit_source: None,
                id: "original-clip".into(),
                source: TakeReference {
                    take_id: descriptor.take_id.clone(),
                    manifest_sha256: original.manifest().sha256.clone(),
                    stream_id: "audio".into(),
                },
                range: TakeRange::AudioFrames { start: 0, count: 1 },
                position: TimelinePosition {
                    frame: 0,
                    sample_rate: 48_000,
                },
                fades: None,
            },
        })
        .unwrap()
        .document;
    let success = fixture.0.join("control.osar");
    let receipt = source.export_archive(project.id(), &success).unwrap();
    assert_eq!(receipt.take_count, 1);
    let (object, _) = take_metadata(&source.connection, &descriptor.take_id.0).unwrap();
    assert!(object.byte_length <= ARCHIVE_CHUNK_BYTES as u64);
    let replacement_sha = checksum(&vec![0; object.byte_length as usize]);
    let replacement_sql = format!(
        "CREATE TRIGGER coherent_chunk_replacement AFTER INSERT ON take_chunks BEGIN UPDATE take_chunks SET payload=zeroblob(byte_length),sha256='{replacement_sha}' WHERE object_sha256=NEW.object_sha256 AND sequence=NEW.sequence; END;"
    );
    let target = fixture.0.join("unplanned-chunks.osar");
    let failure = boundary::with_test_hook(
        Box::new(move |event, context| {
            if event == boundary::TestEvent::ExportSchema {
                let output =
                    Connection::open(context.staging.as_ref().unwrap().join("archive.sqlite3"))
                        .map_err(io::Error::other)?;
                output
                    .execute_batch(&replacement_sql)
                    .map_err(io::Error::other)?;
            }
            Ok(())
        }),
        || source.export_archive_outcome(project.id(), &target),
    )
    .unwrap_err();
    expect_postimage_failure(&failure);
    assert!(!target.exists());
    assert_eq!(fs::read(&journal_path).unwrap(), original_bytes);
    assert_eq!(source.load(project.id()).unwrap(), project);
}

#[test]
fn export_refuses_a_harmless_extra_trigger_even_when_every_postimage_cell_matches() {
    let fixture = Fixture::new();
    let (source, project, _) = fixture.source();
    let target = fixture.0.join("extra-schema.osar");
    let failure = boundary::with_test_hook(
        Box::new(|event, context| {
            if event == boundary::TestEvent::ExportSchema {
                let output =
                    Connection::open(context.staging.as_ref().unwrap().join("archive.sqlite3"))
                        .map_err(io::Error::other)?;
                output
                    .execute_batch(
                        "CREATE TRIGGER harmless AFTER INSERT ON projects BEGIN SELECT 1; END;",
                    )
                    .map_err(io::Error::other)?;
            }
            Ok(())
        }),
        || source.export_archive_outcome(project.id(), &target),
    )
    .unwrap_err();
    assert!(
        matches!(failure.operation_error(),StoreError::Invalid(message) if message=="Archive table definition is not the supported format"),
        "{failure:?}"
    );
    assert!(!failure.publication_uncertain());
    assert!(!target.exists());
    assert_eq!(source.load(project.id()).unwrap(), project);
}

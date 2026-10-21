//! Independent read-only waveform source access tests. No devices or live stores.
use crate::{NativeReader, NativeStore, StoreError};
use omniusong_core::{
    ProjectDocument,
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use rusqlite::{Connection, ErrorCode, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .join("omniusong-reader-independent")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn store_root(&self) -> PathBuf {
        self.0.join("store")
    }

    fn create_store(&self) -> NativeStore {
        NativeStore::open(self.store_root()).unwrap()
    }

    fn database(&self) -> Connection {
        Connection::open(self.store_root().join("studio.sqlite3")).unwrap()
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, String> {
        fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, String>) {
            let metadata = fs::symlink_metadata(path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            if metadata.file_type().is_symlink() {
                result.insert(relative, format!("link:{:?}", fs::read_link(path).unwrap()));
            } else if metadata.is_dir() {
                result.insert(relative, "directory".into());
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), result);
                }
            } else {
                let mut file = fs::File::open(path).unwrap();
                let mut hash = Sha256::new();
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    hash.update(&buffer[..read]);
                }
                result.insert(
                    relative,
                    format!("{}:{:x}", metadata.len(), hash.finalize()),
                );
            }
        }
        let mut result = BTreeMap::new();
        visit(&self.0, &self.0, &mut result);
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Reader fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn assert_invalid(error: StoreError, fragment: &str) {
    assert!(
        matches!(&error, StoreError::Invalid(message) if message.contains(fragment)),
        "Expected Invalid containing {fragment:?}, got {error}"
    );
}

struct AudioFixture {
    files: Fixture,
    document: ProjectDocument,
    reference: TakeReference,
    object: PathBuf,
    object_bytes: u64,
    good_asset: String,
    other_asset: String,
    good_bytes: Vec<u8>,
}
impl AudioFixture {
    fn new() -> Self {
        let files = Fixture::new();
        let descriptor = TakeDescriptor {
            version: JOURNAL_VERSION,
            take_id: TakeId("independent-reader-take".into()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                source_id: "synthetic".into(),
                source_name: "Read-only authentication fixture".into(),
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
        let stamp = HostTimestamp {
            domain: ClockDomainId("host".into()),
            epoch: ClockEpochId("boot".into()),
            ticks: 1_000,
        };
        let mut journal = Journal::create(files.0.join("original.sqlite3"), &descriptor).unwrap();
        journal
            .append(
                &ChunkHeader {
                    version: CHUNK_VERSION,
                    take_id: descriptor.take_id.clone(),
                    stream_id: "audio".into(),
                    segment_id: "segment".into(),
                    sequence: 0,
                    range: SourceRange::AudioFrames {
                        start: 100,
                        count: 70_000,
                    },
                    interpretation: descriptor.streams[0].kind.clone(),
                    observed_start: stamp.clone(),
                    observed_end: stamp,
                    clock_anchors: vec![],
                    kind: ChunkKind::Content,
                },
                &vec![0_u8; 140_000],
            )
            .unwrap();
        journal.seal("reader-seal").unwrap();
        drop(journal);
        let sealed = SealedJournal::open(files.0.join("original.sqlite3")).unwrap();
        let mut store = files.create_store();
        let registered = store.ingest_take(&sealed).unwrap();
        let reference = TakeReference {
            take_id: descriptor.take_id,
            manifest_sha256: registered.manifest_sha256.clone(),
            stream_id: "audio".into(),
        };
        let object = files
            .store_root()
            .join("takes")
            .join(format!("{}.sqlite3", registered.object_sha256));
        let mut document = ProjectDocument::new("Reader preservation")
            .unwrap()
            .upgrade_native()
            .unwrap();
        document
            .append_take_clip(
                "native-track",
                "Synthetic PCM",
                TakeClip {
                    id: "native-clip".into(),
                    source: reference.clone(),
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
        // These opaque asset bytes test store authentication, not WAV decoding.
        let good_bytes = b"selected immutable asset bytes".to_vec();
        let other_bytes = b"unrelated immutable asset bytes".to_vec();
        let good_asset = format!("{:x}.wav", Sha256::digest(&good_bytes));
        let other_asset = format!("{:x}.wav", Sha256::digest(&other_bytes));
        let mut value = document.value().clone();
        value["vendor:exact"] = serde_json::from_str(
            r#"{"integer":184467440737095516160000001,"decimal":0.1234567890123456789012345678901,"null":null,"id":"do-not-remap"}"#,
        )
        .unwrap();
        for (id, asset, bytes) in [
            ("selected", &good_asset, &good_bytes),
            ("other", &other_asset, &other_bytes),
        ] {
            store
                .connection
                .execute(
                    "INSERT INTO assets(id,payload) VALUES(?1,?2)",
                    params![asset, bytes],
                )
                .unwrap();
            value["tracks"].as_array_mut().unwrap().push(json!({
                "id":id,"name":id,"kind":"audio","instrument":"keys","gain":1.0,
                "muted":false,"locked":false,"notes":[],"assetId":asset,
                "audio":{"origin":"import","mimeType":"audio/wav","durationSeconds":1.0,"byteLength":bytes.len()}
            }));
        }
        let document = store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap();
        drop(store);
        drop(sealed);
        Self {
            files,
            document,
            reference,
            object,
            object_bytes: registered.byte_length,
            good_asset,
            other_asset,
            good_bytes,
        }
    }
}

#[test]
fn reader_missing_root_catalog_and_empty_file_never_create_or_initialize() {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    assert!(NativeReader::open(fixture.store_root()).is_err());
    assert_eq!(fixture.snapshot(), before);
    fs::create_dir(fixture.store_root()).unwrap();
    let before = fixture.snapshot();
    assert!(NativeReader::open(fixture.store_root()).is_err());
    assert_eq!(fixture.snapshot(), before);
    fs::write(fixture.store_root().join("studio.sqlite3"), []).unwrap();
    let before = fixture.snapshot();
    assert!(NativeReader::open(fixture.store_root()).is_err());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn reader_refuses_old_and_future_schema_without_migration_or_sidecars() {
    for version in [
        0_u32,
        1,
        crate::STORE_SCHEMA_VERSION - 1,
        crate::STORE_SCHEMA_VERSION + 1,
    ] {
        let fixture = Fixture::new();
        drop(fixture.create_store());
        let database = fixture.database();
        database
            .pragma_update(None, "user_version", version)
            .unwrap();
        drop(database);
        let before = fixture.snapshot();
        assert!(NativeReader::open(fixture.store_root()).is_err());
        assert_eq!(fixture.snapshot(), before, "schema {version} changed");
    }
}

#[test]
fn reader_rejects_wal_header_before_creating_shared_memory() {
    let fixture = Fixture::new();
    drop(fixture.create_store());
    let database = fixture.database();
    let mode: String = database
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    drop(database);
    let bytes = fs::read(fixture.store_root().join("studio.sqlite3")).unwrap();
    assert_eq!(&bytes[18..20], &[2, 2]);
    assert!(!fixture.store_root().join("studio.sqlite3-shm").exists());
    let before = fixture.snapshot();
    assert!(NativeReader::open(fixture.store_root()).is_err());
    assert_eq!(fixture.snapshot(), before);
}

#[cfg(unix)]
#[test]
fn reader_rejects_catalog_aliases_and_redirected_root_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    drop(fixture.create_store());
    let root_alias = fixture.0.join("root-link");
    symlink(fixture.store_root(), &root_alias).unwrap();
    let catalog = fixture.store_root().join("studio.sqlite3");
    let extra_link = fixture.0.join("catalog-link");
    fs::hard_link(&catalog, &extra_link).unwrap();
    let before = fixture.snapshot();
    assert!(NativeReader::open(&root_alias).is_err());
    assert!(NativeReader::open(fixture.store_root()).is_err());
    assert_eq!(fixture.snapshot(), before);
    fs::remove_file(&extra_link).unwrap();
    let actual = fixture.0.join("saved-catalog.sqlite3");
    fs::rename(&catalog, &actual).unwrap();
    symlink(&actual, &catalog).unwrap();
    let before = fixture.snapshot();
    assert!(NativeReader::open(fixture.store_root()).is_err());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn reader_exact_metadata_and_selected_asset_ignore_unrelated_damaged_sources() {
    let fixture = AudioFixture::new();
    let database = fixture.files.database();
    database
        .execute(
            "UPDATE assets SET payload=?1 WHERE id=?2",
            params![b"damage".as_slice(), fixture.other_asset],
        )
        .unwrap();
    drop(database);
    fs::remove_file(&fixture.object).unwrap();
    let before = fixture.files.snapshot();
    let mut reader = NativeReader::open(fixture.files.store_root()).unwrap();
    let (document, asset, metadata) = reader
        .read_snapshot(|view| {
            Ok((
                view.load_document(fixture.document.id())?,
                view.asset(&fixture.good_asset)?,
                view.take_metadata(&fixture.reference.take_id)?,
            ))
        })
        .unwrap();
    assert_eq!(
        document.to_json().unwrap(),
        fixture.document.to_json().unwrap()
    );
    assert_eq!(asset, fixture.good_bytes);
    assert_eq!(metadata.take_id, fixture.reference.take_id);
    assert_eq!(metadata.manifest_sha256, fixture.reference.manifest_sha256);
    assert!(reader.asset(&fixture.other_asset).is_err());
    assert!(
        reader
            .open_take_bounded(&fixture.reference, fixture.object_bytes, &|| false)
            .is_err()
    );
    drop(reader);
    assert_eq!(fixture.files.snapshot(), before);
}

#[test]
fn reader_asset_checks_hash_and_bound_before_returning_bytes() {
    let fixture = AudioFixture::new();
    let database = fixture.files.database();
    database
        .execute(
            "UPDATE assets SET payload=zeroblob(?1) WHERE id=?2",
            params![30_i64 * 1024 * 1024 + 1, fixture.other_asset],
        )
        .unwrap();
    drop(database);
    let before = fixture.files.snapshot();
    let reader = NativeReader::open(fixture.files.store_root()).unwrap();
    assert_eq!(
        reader.asset(&fixture.good_asset).unwrap(),
        fixture.good_bytes
    );
    assert_invalid(
        reader.asset(&fixture.other_asset).unwrap_err(),
        "size limit",
    );
    assert!(reader.asset("../../outside.wav").is_err());
    drop(reader);
    assert_eq!(fixture.files.snapshot(), before);
}

#[test]
fn reader_snapshot_releases_shared_lock_on_success_and_explicit_error() {
    let fixture = Fixture::new();
    let mut store = fixture.create_store();
    let mut saved = store
        .create(&ProjectDocument::new("Before snapshot").unwrap())
        .unwrap();
    drop(store);
    let mut reader = NativeReader::open(fixture.store_root()).unwrap();
    let writer = fixture.database();
    writer.busy_timeout(Duration::ZERO).unwrap();
    for fail in [false, true] {
        let mut next = saved.clone();
        next.rename(if fail {
            "After error release"
        } else {
            "After success release"
        })
        .unwrap();
        let result = reader.read_snapshot(|view| -> crate::Result<()> {
            assert_eq!(view.load_document(saved.id())?, saved);
            writer.execute_batch("BEGIN IMMEDIATE").unwrap();
            writer
                .execute(
                    "UPDATE projects SET payload=?1 WHERE id=?2",
                    params![
                        String::from_utf8(next.to_json().unwrap()).unwrap(),
                        next.id()
                    ],
                )
                .unwrap();
            let blocked = writer.execute_batch("COMMIT").unwrap_err();
            assert_eq!(blocked.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
            assert_eq!(view.load_document(saved.id())?, saved);
            if fail {
                Err(StoreError::Invalid("deliberate snapshot rejection".into()))
            } else {
                Ok(())
            }
        });
        assert_eq!(result.is_err(), fail);
        writer.execute_batch("COMMIT").unwrap();
        assert_eq!(reader.load_document(next.id()).unwrap(), next);
        saved = next;
    }
}

#[test]
fn reader_snapshot_panic_rolls_back_and_reader_can_be_reused() {
    let fixture = Fixture::new();
    let mut store = fixture.create_store();
    let saved = store
        .create(&ProjectDocument::new("Panic release").unwrap())
        .unwrap();
    drop(store);
    let before = fixture.snapshot();
    let mut reader = NativeReader::open(fixture.store_root()).unwrap();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: crate::Result<()> = reader.read_snapshot(|view| {
            assert_eq!(view.load_document(saved.id())?, saved);
            panic!("deliberate fixture panic");
        });
    }));
    assert!(caught.is_err());
    assert_eq!(
        reader
            .read_snapshot(|view| view.load_document(saved.id()))
            .unwrap(),
        saved
    );
    let writer = fixture.database();
    writer.busy_timeout(Duration::ZERO).unwrap();
    writer.execute_batch("BEGIN EXCLUSIVE; ROLLBACK;").unwrap();
    drop(writer);
    drop(reader);
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn reader_budget_rejection_precedes_corrupt_journal_scan() {
    let fixture = AudioFixture::new();
    let mut damaged = fs::read(&fixture.object).unwrap();
    damaged[..16].fill(0x41);
    fs::write(&fixture.object, &damaged).unwrap();
    let before = fixture.files.snapshot();
    let reader = NativeReader::open(fixture.files.store_root()).unwrap();
    assert_invalid(
        reader
            .open_take_bounded(&fixture.reference, fixture.object_bytes - 1, &|| false)
            .err()
            .unwrap(),
        "inspection byte limit",
    );
    assert!(
        reader
            .open_take_bounded(&fixture.reference, fixture.object_bytes, &|| false)
            .is_err()
    );
    drop(reader);
    assert_eq!(fixture.files.snapshot(), before);
    // Registered length fits this ceiling; a larger physical replacement must
    // still fail the budget check before SQLite sees its corrupt header.
    damaged.push(0);
    fs::write(&fixture.object, &damaged).unwrap();
    let before = fixture.files.snapshot();
    let reader = NativeReader::open(fixture.files.store_root()).unwrap();
    assert_invalid(
        reader
            .open_take_bounded(&fixture.reference, fixture.object_bytes, &|| false)
            .err()
            .unwrap(),
        "inspection byte limit",
    );
    drop(reader);
    assert_eq!(fixture.files.snapshot(), before);
}

#[test]
fn reader_cancellation_is_checked_before_open_and_during_multi_block_hashing() {
    let fixture = AudioFixture::new();
    assert!(fixture.object_bytes > 2 * 64 * 1024);
    let before = fixture.files.snapshot();
    let reader = NativeReader::open(fixture.files.store_root()).unwrap();
    assert_invalid(
        reader
            .open_take_bounded(&fixture.reference, fixture.object_bytes, &|| true)
            .err()
            .unwrap(),
        "cancelled",
    );
    let calls = Cell::new(0_usize);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() == 6
    };
    assert_invalid(
        reader
            .open_take_bounded(&fixture.reference, fixture.object_bytes, &cancel)
            .err()
            .unwrap(),
        "cancelled",
    );
    assert_eq!(calls.get(), 6);
    let checks = Cell::new(0_u64);
    let sealed = reader
        .open_take_bounded(&fixture.reference, fixture.object_bytes, &|| {
            checks.set(checks.get() + 1);
            false
        })
        .unwrap();
    assert!(checks.get() >= fixture.object_bytes.div_ceil(64 * 1024) + 4);
    assert_eq!(sealed.manifest().sha256, fixture.reference.manifest_sha256);
    assert_eq!(
        sealed.read_chunk("audio", 0).unwrap().payload.len(),
        140_000
    );
    drop(sealed);
    drop(reader);
    assert_eq!(fixture.files.snapshot(), before);
}

#[test]
fn reader_hash_rejects_growth_after_preflight_without_reading_the_appended_tail() {
    let fixture = AudioFixture::new();
    let original = fs::read(&fixture.object).unwrap();
    let before = fixture.files.snapshot();
    let reader = NativeReader::open(fixture.files.store_root()).unwrap();
    let checks = Cell::new(0_u64);
    let grow_at_hash_start = || {
        checks.set(checks.get() + 1);
        // Calls 1–3 precede path open, sealed scan and manifest checks. Call 4
        // is the first hash-read cancellation check, after both size preflights.
        if checks.get() == 4 {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&fixture.object)
                .unwrap();
            file.write_all(&vec![0x5a_u8; 1024 * 1024]).unwrap();
        }
        false
    };
    assert_invalid(
        reader
            .open_take_bounded(&fixture.reference, u64::MAX, &grow_at_hash_start)
            .err()
            .unwrap(),
        "grew beyond its authenticated inspection byte limit",
    );
    assert_eq!(
        checks.get(),
        3 + (fixture.object_bytes + 1).div_ceil(64 * 1024),
        "Hashing must stop after at most registered length plus one byte, even with an unlimited caller ceiling"
    );
    drop(reader);
    let after_bytes = fs::read(&fixture.object).unwrap();
    assert_eq!(&after_bytes[..original.len()], &original);
    assert_eq!(after_bytes.len(), original.len() + 1024 * 1024);
    let mut after = fixture.files.snapshot();
    let mut before = before;
    let injected = fixture.object.strip_prefix(&fixture.files.0).unwrap();
    after.remove(injected);
    before.remove(injected);
    assert_eq!(
        after, before,
        "Only the fixture's explicit injected tail may change"
    );
}

#[test]
fn reader_authenticates_physical_file_even_when_sealed_rows_still_validate() {
    let fixture = AudioFixture::new();
    let mut altered = fs::read(&fixture.object).unwrap();
    // application_id is outside the authenticated logical take rows, but the
    // registered full-file SHA must still reject its alteration.
    altered[68] ^= 1;
    fs::write(&fixture.object, &altered).unwrap();
    assert!(SealedJournal::open(&fixture.object).is_ok());
    let before = fixture.files.snapshot();
    let reader = NativeReader::open(fixture.files.store_root()).unwrap();
    assert_invalid(
        reader
            .open_take_bounded(&fixture.reference, fixture.object_bytes, &|| false)
            .err()
            .unwrap(),
        "file hash is damaged",
    );
    let mut wrong_manifest = fixture.reference.clone();
    wrong_manifest.manifest_sha256 = "0".repeat(64);
    assert_invalid(
        reader
            .open_take_bounded(&wrong_manifest, fixture.object_bytes, &|| false)
            .err()
            .unwrap(),
        "registered sealed identity",
    );
    let mut wrong_stream = fixture.reference.clone();
    wrong_stream.stream_id = "absent".into();
    assert_invalid(
        reader
            .open_take_bounded(&wrong_stream, fixture.object_bytes, &|| false)
            .err()
            .unwrap(),
        "unknown stream",
    );
    drop(reader);
    assert_eq!(fixture.files.snapshot(), before);
}

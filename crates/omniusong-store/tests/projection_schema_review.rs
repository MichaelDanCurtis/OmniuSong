//! Independent schema-3 admission tests. Every database and source is synthetic.
//! Parsing a projection claim is deliberately weaker than admitting its authority.
use omniusong_core::{
    ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, RestoreCopyIntent, TakePublication};
use rusqlite::{Connection, params, types::ValueRef};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/projection-schema-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn root(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn store(&self, name: &str) -> NativeStore {
        NativeStore::open(self.root(name)).unwrap()
    }
    fn db(&self, name: &str) -> Connection {
        Connection::open(self.root(name).join("studio.sqlite3")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Projection-schema review retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
#[derive(Debug, PartialEq, Eq)]
struct Catalog {
    application: i64,
    version: i64,
    schema: Vec<(String, String, String, Option<String>)>,
    tables: Vec<(String, Vec<Vec<Cell>>)>,
}
fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
fn catalog(db: &Connection) -> Catalog {
    let schema = db
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let tables = names
        .into_iter()
        .map(|name| {
            let mut statement = db
                .prepare(&format!("SELECT * FROM {}", quote(&name)))
                .unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            let mut values = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                let cells = (0..columns)
                    .map(|i| match row.get_ref(i).unwrap() {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(v) => Cell::Integer(v),
                        ValueRef::Real(v) => Cell::Real(v.to_bits()),
                        ValueRef::Text(v) => Cell::Text(v.to_vec()),
                        ValueRef::Blob(v) => Cell::Blob(v.to_vec()),
                    })
                    .collect();
                values.push(cells);
            }
            values.sort();
            (name, values)
        })
        .collect();
    Catalog {
        application: db
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .unwrap(),
        version: db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap(),
        schema,
        tables,
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, path: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !path.exists() {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, found);
            } else {
                found.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut found = BTreeMap::new();
    visit(root, root, &mut found);
    found
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn edit(document: &ProjectDocument, operation: &str, action: EditOperation) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        scope: EditScope::Project,
        edit: action,
        extensions: Default::default(),
    }
}
fn empty(name: &str) -> ProjectDocument {
    ProjectDocument::new(name)
        .unwrap()
        .upgrade_performance_projection_schema()
        .unwrap()
}
fn sealed(f: &Fixture) -> (PathBuf, SealedJournal) {
    let path = f.0.join("synthetic-audio.sqlite3");
    let descriptor: TakeDescriptor = serde_json::from_value(json!({
        "version":1,"take_id":"schema-review-take","streams":[{
            "stream_id":"audio","segment_id":"segment","source_id":"synthetic",
            "source_name":"Device-free schema review","source_connection_generation":1,
            "host_clock":{"domain":"host","epoch":"epoch","ticks_per_second_numerator":8000,"ticks_per_second_denominator":1},
            "kind":{"kind":"audio","format":{"sample_rate":8000,"channels":1,"encoding":"f32_le","layout":"interleaved"},"audio_epoch":"epoch"},
            "initial_source_position":0
        }],"provenance":{"synthetic":true}
    })).unwrap();
    let mut journal = Journal::create(&path, &descriptor).unwrap();
    let stamp = HostTimestamp {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("epoch".into()),
        ticks: 0,
    };
    journal
        .append(
            &ChunkHeader {
                version: 1,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                sequence: 0,
                range: SourceRange::AudioFrames { start: 0, count: 4 },
                interpretation: descriptor.streams[0].kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &[0; 16],
        )
        .unwrap();
    journal.seal("synthetic-schema-review").unwrap();
    drop(journal);
    let reader = SealedJournal::open(&path).unwrap();
    (path, reader)
}
fn publication(document: &ProjectDocument, source: &SealedJournal) -> TakePublication {
    TakePublication {
        operation_id: OperationId("publish-native-audio".into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: document.revision(),
        track_id: TrackId("native-audio".into()),
        track_name: "Original PCM".into(),
        clip: TakeClip {
            id: "native-clip".into(),
            source: TakeReference {
                take_id: source.descriptor().take_id.clone(),
                manifest_sha256: source.manifest().sha256.clone(),
                stream_id: "audio".into(),
            },
            range: TakeRange::AudioFrames { start: 0, count: 4 },
            position: TimelinePosition {
                frame: 0,
                sample_rate: 8000,
            },
            fades: None,
            edit_source: None,
        },
    }
}

// This is intentionally NOT a valid shared-kernel review. It is sufficient for
// the core's documented structural envelope, including matching source/digest
// claims. Refusal must therefore come from storage's authority boundary.
fn forged(document: &ProjectDocument, source: &TakeReference) -> ProjectDocument {
    let digest = "a".repeat(64);
    let review = json!({"version":1,"review_sha256":digest,"request":{"source":{
        "take_id":source.take_id.0,"manifest_sha256":source.manifest_sha256
    }}})
    .to_string();
    let mut value = document.value().clone();
    value["schemaVersion"] = 3.into();
    value["performanceProjections"] = json!({"version":1,"entries":[{
        "id":"plausible-but-unverified","version":1,"source":source,
        "review":{"version":1,"canonicalJson":review,"reviewSha256":digest},
        "lanes":[{"source":1,"trackId":document.tracks()[0]["id"],"instrument":"keys",
            "originalNotes":[{"noteId":"original-projection-note","identitySha256":"b".repeat(64),
                "values":{"pitch":60,"start_tick":0,"duration_ticks":1,"velocity16":65535}}]}]
    }]});
    ProjectDocument::from_value(value)
        .expect("Core structure must not be confused with authenticated publication")
}
fn refused<T: std::fmt::Debug>(result: std::result::Result<T, omniusong_store::StoreError>) {
    let error = result.expect_err("Unverified projection authority was admitted");
    assert!(
        error.to_string().to_lowercase().contains("projection"),
        "Wrong refusal layer: {error}"
    );
}
fn remove_table_triggers(db: &Connection, table: &str) {
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1")
        .unwrap()
        .query_map([table], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for name in names {
        db.execute_batch(&format!("DROP TRIGGER {}", quote(&name)))
            .unwrap();
    }
}

#[test]
fn recorded_opaque_promotion_pair_cannot_bypass_save_through_redo_or_undo() {
    for redo in [true, false] {
        let f = Fixture::new();
        let mut store = f.store("history");
        let mut value = ProjectDocument::new("Opaque history source")
            .unwrap()
            .value()
            .clone();
        value["performanceProjections"] = Value::Null;
        let before = store
            .create(&ProjectDocument::from_value(value.clone()).unwrap())
            .unwrap();
        value["schemaVersion"] = 3.into();
        value["performanceProjections"] = json!({"version":1,"entries":[]});
        let after = ProjectDocument::from_value(value)
            .unwrap()
            .with_revision(1)
            .unwrap();
        let db = f.db("history");
        db.execute(
            "INSERT INTO edits(project_id,sequence,before_json,after_json) VALUES(?1,1,?2,?3)",
            params![
                before.id(),
                String::from_utf8(before.to_json().unwrap()).unwrap(),
                String::from_utf8(after.to_json().unwrap()).unwrap()
            ],
        )
        .unwrap();
        if !redo {
            db.execute(
                "UPDATE projects SET payload=?1,revision=1,cursor=1 WHERE id=?2",
                params![
                    String::from_utf8(after.to_json().unwrap()).unwrap(),
                    before.id()
                ],
            )
            .unwrap();
        }
        let retained = catalog(&db);
        let result = if redo {
            store.redo(before.id(), 0)
        } else {
            store.undo(before.id(), 1)
        };
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("opaque performanceProjections")
        );
        assert_eq!(catalog(&db), retained);
    }
}

#[test]
fn undo_of_a_valid_empty_registry_downgrade_uses_the_stored_forward_pair() {
    let f = Fixture::new();
    let mut store = f.store("reverse");
    let source = store
        .create(&empty("Explicit empty projection format"))
        .unwrap();
    let mut value = source.value().clone();
    value["schemaVersion"] = 2.into();
    let old = ProjectDocument::from_value(value).unwrap();
    let saved = store.save(&old, 0).unwrap();
    assert_eq!(saved.schema_version(), 2);
    assert_eq!(
        saved.value()["performanceProjections"],
        source.value()["performanceProjections"]
    );
    let undone = store.undo(source.id(), 1).unwrap();
    assert_eq!(undone.schema_version(), 3);
    assert_eq!(
        undone.value()["performanceProjections"],
        source.value()["performanceProjections"]
    );
    let redone = store.redo(source.id(), 2).unwrap();
    assert_eq!(redone.schema_version(), 2);
    let candidate = ProjectDocument::from_value({
        let mut v = redone.value().clone();
        v["schemaVersion"] = 3.into();
        v
    })
    .unwrap();
    let db = f.db("reverse");
    let retained = catalog(&db);
    assert!(store.save(&candidate, 3).is_err());
    assert_eq!(catalog(&db), retained);
}

#[test]
fn unsupported_current_registry_refuses_normal_capture_before_any_ingestion() {
    let f = Fixture::new();
    let (path, source) = sealed(&f);
    let source_bytes = fs::read(&path).unwrap();
    let mut store = f.store("unsupported-target");
    let document = store.create(&empty("Unsupported target")).unwrap();
    let request = publication(&document, &source);
    let claim = forged(&document, &request.clip.source);
    let db = f.db("unsupported-target");
    db.execute(
        "UPDATE projects SET payload=?1 WHERE id=?2",
        params![
            String::from_utf8(claim.to_json().unwrap()).unwrap(),
            document.id()
        ],
    )
    .unwrap();
    let before = catalog(&db);
    let original_files = files(&f.root("unsupported-target"));
    refused(store.attach_capture(&path, &request));
    assert_eq!(catalog(&db), before);
    assert_eq!(files(&f.root("unsupported-target")), original_files);
    assert_eq!(fs::read(path).unwrap(), source_bytes);
    assert!(
        !store
            .has_registered_take(&request.clip.source.take_id)
            .unwrap()
    );
}

#[test]
fn plausible_nonempty_registry_cannot_enter_create_or_save_even_with_an_owned_matching_take() {
    let f = Fixture::new();
    let (source_path, source) = sealed(&f);
    let source_bytes = fs::read(source_path).unwrap();
    let mut store = f.store("store");
    store.ingest_take(&source).unwrap();
    let document = store.create(&empty("Active clean document")).unwrap();
    let claim = forged(&document, &publication(&document, &source).clip.source);
    assert_eq!(
        claim
            .performance_projections()
            .unwrap()
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_eq!(
        claim.referenced_takes().unwrap().len(),
        1,
        "Registry-only sources must be enumerated even without current notes or recording clips"
    );
    let before = catalog(&f.db("store"));
    let owned_before = files(&f.root("store").join("takes"));
    refused(store.create(&claim));
    refused(store.save(&claim, document.revision()));
    assert_eq!(store.load(document.id()).unwrap(), document);
    assert_eq!(catalog(&f.db("store")), before);
    assert_eq!(files(&f.root("store").join("takes")), owned_before);
    drop(source);
    assert_eq!(
        fs::read(f.0.join("synthetic-audio.sqlite3")).unwrap(),
        source_bytes
    );
}

#[test]
fn retained_nonempty_current_cannot_load_downgrade_save_edit_or_navigate() {
    let f = Fixture::new();
    let (_, source) = sealed(&f);
    let mut store = f.store("store");
    store.ingest_take(&source).unwrap();
    let initial = store
        .create(&empty("Clean before direct catalog injection"))
        .unwrap();
    let mut next = initial.clone();
    next.rename("Clean history").unwrap();
    let current = store.save(&next, 0).unwrap();
    let claim = forged(&current, &publication(&current, &source).clip.source);
    let db = f.db("store");
    db.execute(
        "UPDATE projects SET payload=?1 WHERE id=?2",
        params![
            String::from_utf8(claim.to_json().unwrap()).unwrap(),
            current.id()
        ],
    )
    .unwrap();
    let before = catalog(&db);
    let sources = files(&f.root("store").join("takes"));
    refused(store.load(current.id()));
    let mut downgrade = current.value().clone();
    downgrade["schemaVersion"] = 2.into();
    downgrade
        .as_object_mut()
        .unwrap()
        .remove("performanceProjections");
    let downgrade = ProjectDocument::from_value(downgrade).unwrap();
    refused(store.save(&downgrade, current.revision()));
    refused(store.apply_edit(&edit(
        &current,
        "no-rename",
        EditOperation::Rename {
            name: "Do not erase the retained claim".into(),
        },
    )));
    refused(store.undo(current.id(), current.revision()));
    refused(store.redo(current.id(), current.revision()));
    assert_eq!(catalog(&db), before);
    assert_eq!(files(&f.root("store").join("takes")), sources);
    drop(store);
    refused(
        NativeStore::open_existing(f.root("store"))
            .unwrap()
            .load(current.id()),
    );
    assert_eq!(catalog(&db), before);
}

#[test]
fn poisoned_undo_and_redo_targets_refuse_before_any_history_or_operation_changes() {
    for redo in [false, true] {
        let f = Fixture::new();
        let (_, source) = sealed(&f);
        let mut store = f.store("store");
        store.ingest_take(&source).unwrap();
        let initial = store.create(&empty("History origin")).unwrap();
        let mut changed = initial.clone();
        changed.rename("History after").unwrap();
        let saved = store.save(&changed, 0).unwrap();
        let current = if redo {
            store.undo(saved.id(), saved.revision()).unwrap()
        } else {
            saved.clone()
        };
        let target = if redo { &saved } else { &initial };
        let claim = forged(target, &publication(target, &source).clip.source);
        let db = f.db("store");
        remove_table_triggers(&db, "edits");
        let field = if redo { "after_json" } else { "before_json" };
        db.execute(
            &format!("UPDATE edits SET {field}=?1 WHERE project_id=?2 AND sequence=1"),
            params![
                String::from_utf8(claim.to_json().unwrap()).unwrap(),
                initial.id()
            ],
        )
        .unwrap();
        let before = catalog(&db);
        let owned = files(&f.root("store").join("takes"));
        let action = if redo {
            EditOperation::Redo
        } else {
            EditOperation::Undo
        };
        refused(store.apply_edit(&edit(&current, "refuse-poisoned-navigation", action)));
        assert_eq!(store.load(current.id()).unwrap(), current);
        assert_eq!(catalog(&db), before);
        assert_eq!(files(&f.root("store").join("takes")), owned);
    }
}

#[test]
fn nonempty_claim_hidden_in_history_blocks_export_despite_a_clean_schema_two_head() {
    let f = Fixture::new();
    let (_, source) = sealed(&f);
    let mut store = f.store("store");
    store.ingest_take(&source).unwrap();
    let initial = store
        .create(
            &ProjectDocument::new("Schema two head")
                .unwrap()
                .upgrade_native()
                .unwrap(),
        )
        .unwrap();
    let mut next = initial.clone();
    next.rename("Clean latest head").unwrap();
    let current = store.save(&next, 0).unwrap();
    let claim = forged(&initial, &publication(&initial, &source).clip.source);
    let db = f.db("store");
    remove_table_triggers(&db, "edits");
    db.execute(
        "UPDATE edits SET before_json=?1 WHERE project_id=?2",
        params![
            String::from_utf8(claim.to_json().unwrap()).unwrap(),
            initial.id()
        ],
    )
    .unwrap();
    let before = catalog(&db);
    let owned = files(&f.root("store").join("takes"));
    assert_eq!(store.load(current.id()).unwrap(), current);
    let destination = f.0.join("must-not-publish.omniusong");
    refused(store.export_archive(current.id(), &destination));
    assert!(!destination.exists());
    assert_eq!(catalog(&db), before);
    assert_eq!(files(&f.root("store").join("takes")), owned);
}

#[test]
fn coherently_rehashed_operation_result_cannot_replay_a_nonempty_claim() {
    let f = Fixture::new();
    let (_, source) = sealed(&f);
    let mut store = f.store("store");
    store.ingest_take(&source).unwrap();
    let initial = store.create(&empty("Replay origin")).unwrap();
    let intent = edit(
        &initial,
        "same-rename",
        EditOperation::Rename {
            name: "Real completed result".into(),
        },
    );
    let completed = store.apply_edit(&intent).unwrap().document;
    let claim = forged(&completed, &publication(&completed, &source).clip.source);
    let bytes = claim.to_json().unwrap();
    let db = f.db("store");
    remove_table_triggers(&db, "operations");
    db.execute(
        "UPDATE operations SET result_json=?1,result_sha256=?2 WHERE operation_id=?3",
        params![
            String::from_utf8(bytes.clone()).unwrap(),
            hash(&bytes),
            intent.operation_id.0
        ],
    )
    .unwrap();
    let before = catalog(&db);
    refused(store.apply_edit(&intent));
    assert_eq!(store.load(completed.id()).unwrap(), completed);
    assert_eq!(catalog(&db), before);
}

fn index_record(index: &mut Sha256, tag: &[u8], fields: &[&[u8]]) {
    index.update((tag.len() as u64).to_le_bytes());
    index.update(tag);
    index.update((fields.len() as u64).to_le_bytes());
    for field in fields {
        index.update((field.len() as u64).to_le_bytes());
        index.update(field);
    }
}
// Independent format oracle for this deliberately minimal archive: exactly one
// project and one original take, with no edits/imports/operations/assets/lineage.
fn single_source_archive_index(db: &Connection) -> (String, usize) {
    for table in ["edits", "operations", "imports", "assets"] {
        let count: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let (id, payload, revision, cursor): (String, String, i64, i64) = db
        .query_row("SELECT id,payload,revision,cursor FROM projects", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    assert_eq!((revision, cursor), (0, 0));
    let mut index = Sha256::new();
    index.update(b"OmniuSong.project-archive.v1\0");
    index_record(
        &mut index,
        b"project",
        &[
            id.as_bytes(),
            payload.as_bytes(),
            &revision.to_le_bytes(),
            &cursor.to_le_bytes(),
        ],
    );
    let (take,manifest,object,length,json):(String,String,String,i64,String) = db.query_row("SELECT take_id,manifest_sha256,object_sha256,byte_length,manifest_json FROM native_takes", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    let length = u64::try_from(length).unwrap();
    index_record(
        &mut index,
        b"take",
        &[
            take.as_bytes(),
            manifest.as_bytes(),
            object.as_bytes(),
            &length.to_le_bytes(),
            json.as_bytes(),
        ],
    );
    let chunks: i64 = db
        .query_row("SELECT count(*) FROM take_chunks", [], |r| r.get(0))
        .unwrap();
    (
        format!("{:x}", index.finalize()),
        payload.len()
            + id.len()
            + 16
            + json.len()
            + take.len()
            + 136
            + usize::try_from(chunks).unwrap() * 144,
    )
}

#[test]
fn fully_reindexed_archive_claim_refuses_identity_and_copy_restore_before_catalog_or_object_install()
 {
    let f = Fixture::new();
    let (source_path, source) = sealed(&f);
    let source_bytes = fs::read(&source_path).unwrap();
    let mut source_store = f.store("source");
    source_store.ingest_take(&source).unwrap();
    let mut document = empty("One source, no operation receipts");
    let request = publication(&document, &source);
    document
        .append_take_clip(
            &request.track_id.0,
            &request.track_name,
            request.clip.clone(),
        )
        .unwrap();
    let document = source_store.create(&document).unwrap();
    let archive_path = f.0.join("reindexed-claim.omniusong");
    source_store
        .export_archive(document.id(), &archive_path)
        .unwrap();
    let archive_db = Connection::open(&archive_path).unwrap();
    let (index, metadata) = single_source_archive_index(&archive_db);
    let recorded: (String, i64) = archive_db
        .query_row(
            "SELECT index_sha256,metadata_bytes FROM archive_meta",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        recorded,
        (index, i64::try_from(metadata).unwrap()),
        "Oracle must reproduce the untouched valid archive first"
    );
    let claim = forged(&document, &request.clip.source);
    let bytes = claim.to_json().unwrap();
    archive_db
        .execute(
            "UPDATE projects SET payload=?1,payload_sha256=?2",
            params![String::from_utf8(bytes.clone()).unwrap(), hash(&bytes)],
        )
        .unwrap();
    let (index, metadata) = single_source_archive_index(&archive_db);
    archive_db
        .execute(
            "UPDATE archive_meta SET index_sha256=?1,metadata_bytes=?2",
            params![index, metadata as i64],
        )
        .unwrap();
    archive_db.close().unwrap();
    let archive_bytes = fs::read(&archive_path).unwrap();
    let source_before = catalog(&f.db("source"));
    for copy in [false, true] {
        let name = if copy { "copy" } else { "identity" };
        let mut destination = f.store(name);
        let before = catalog(&f.db(name));
        let objects = files(&f.root(name).join("takes"));
        if copy {
            refused(destination.restore_archive_as_copy(
                &archive_path,
                &RestoreCopyIntent {
                    operation_id: OperationId("refuse-forged-copy".into()),
                    expected_archive_sha256: hash(&archive_bytes),
                },
            ));
        } else {
            refused(destination.restore_archive(&archive_path));
        }
        assert_eq!(catalog(&f.db(name)), before);
        assert_eq!(files(&f.root(name).join("takes")), objects);
        assert_eq!(fs::read(&archive_path).unwrap(), archive_bytes);
    }
    assert_eq!(catalog(&f.db("source")), source_before);
    drop(source);
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
}

#[test]
fn legacy_same_named_registry_is_opaque_and_cannot_be_reinterpreted_by_generic_save() {
    for schema in [1, 2] {
        for opaque in [
            Value::Null,
            json!({"version":"vendor","entries":{"not":"owned"},"exact":18446744073709551615u64}),
        ] {
            let f = Fixture::new();
            let mut store = f.store("store");
            let mut value = ProjectDocument::new("Old registry namespace")
                .unwrap()
                .value()
                .clone();
            value["schemaVersion"] = schema.into();
            value["performanceProjections"] = opaque.clone();
            value["tracks"][0]["recording"] = json!({"unrecognized":"legacy extension"});
            let legacy = ProjectDocument::from_value(value).unwrap();
            assert!(legacy.performance_projections().unwrap().is_none());
            assert!(legacy.referenced_takes().unwrap().is_empty());
            assert!(legacy.upgrade_performance_projection_schema().is_err());
            let original = store.create(&legacy).unwrap();
            let mut rename = original.clone();
            rename.rename("An ordinary legacy edit").unwrap();
            let saved = store.save(&rename, original.revision()).unwrap();
            assert_eq!(saved.value()["performanceProjections"], opaque);
            let undone = store.undo(saved.id(), saved.revision()).unwrap();
            let current = store.redo(undone.id(), undone.revision()).unwrap();
            assert_eq!(current.value()["performanceProjections"], opaque);
            let before = catalog(&f.db("store"));
            // Bypass the explicit pure helper with an otherwise valid empty-3
            // candidate. Generic Save must still preserve the older namespace.
            let mut promoted = current.value().clone();
            promoted["schemaVersion"] = 3.into();
            promoted["performanceProjections"] = json!({"version":1,"entries":[]});
            let promoted = ProjectDocument::from_value(promoted).unwrap();
            refused(store.save(&promoted, current.revision()));
            assert_eq!(catalog(&f.db("store")), before);
            assert_eq!(store.load(current.id()).unwrap(), current);
            let archive_path = f.0.join("opaque-legacy.omniusong");
            let archive = store.export_archive(current.id(), &archive_path).unwrap();
            let mut copy = f.store("copy");
            let copied = copy
                .restore_archive_as_copy(
                    &archive_path,
                    &RestoreCopyIntent {
                        operation_id: OperationId("copy-opaque".into()),
                        expected_archive_sha256: archive.sha256,
                    },
                )
                .unwrap();
            assert_eq!(copied.document.schema_version(), schema);
            assert_eq!(copied.document.value()["performanceProjections"], opaque);
            assert_eq!(
                copied.document.tracks()[0]["recording"],
                current.tracks()[0]["recording"]
            );
        }
    }
}

#[test]
fn malformed_schema_three_ownership_refuses_without_touching_existing_sql_or_source_bytes() {
    let f = Fixture::new();
    let (source_path, source) = sealed(&f);
    let source_bytes = fs::read(&source_path).unwrap();
    let mut store = f.store("store");
    store.ingest_take(&source).unwrap();
    let initial = store
        .create(&empty("Untouched while parsing malformed claims"))
        .unwrap();
    let valid_shape = forged(&initial, &publication(&initial, &source).clip.source);
    let before = catalog(&f.db("store"));
    let mut malformed = Vec::new();
    let mut missing = valid_shape.value().clone();
    missing
        .as_object_mut()
        .unwrap()
        .remove("performanceProjections");
    malformed.push(missing);
    for (pointer, replacement) in [
        ("/performanceProjections/version", json!(2)),
        (
            "/performanceProjections/entries/0/source/streamId",
            json!("events"),
        ),
        (
            "/performanceProjections/entries/0/review/reviewSha256",
            json!("c".repeat(64)),
        ),
        ("/performanceProjections/entries/0/lanes/0/source", json!(0)),
        (
            "/performanceProjections/entries/0/lanes/0/originalNotes/0/values/velocity16",
            json!(65536),
        ),
    ] {
        let mut value = valid_shape.value().clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        malformed.push(value);
    }
    let mut unknown = valid_shape.value().clone();
    unknown["performanceProjections"]["extra"] = true.into();
    malformed.push(unknown);
    for value in malformed {
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(ProjectDocument::from_json(&bytes).is_err());
        assert_eq!(catalog(&f.db("store")), before);
    }
    drop(source);
    assert_eq!(fs::read(source_path).unwrap(), source_bytes);
}

#[test]
fn empty_schema_three_retains_real_native_sources_through_history_reopen_and_both_archive_modes() {
    let f = Fixture::new();
    let (source_path, source) = sealed(&f);
    let original_bytes = fs::read(&source_path).unwrap();
    let mut store = f.store("source");
    let initial = store
        .create(&empty("Empty registry, real recording"))
        .unwrap();
    assert_eq!(initial.schema_version(), 3);
    assert!(initial.supports_native_takes());
    assert!(
        initial
            .performance_projections()
            .unwrap()
            .unwrap()
            .entries
            .is_empty()
    );
    let request = publication(&initial, &source);
    let published = store
        .attach_capture(&source_path, &request)
        .unwrap()
        .document;
    assert_eq!(published.schema_version(), 3);
    assert_eq!(published.take_clips().unwrap(), vec![request.clip.clone()]);
    assert_eq!(
        published.referenced_takes().unwrap(),
        [request.clip.source.clone()].into()
    );
    let owned_before = files(&f.root("source").join("takes"));
    let mut renamed = published.clone();
    renamed
        .rename("Saved without projection authority")
        .unwrap();
    let saved = store.save(&renamed, published.revision()).unwrap();
    let undone = store.undo(saved.id(), saved.revision()).unwrap();
    assert_eq!(undone.name(), published.name());
    assert_eq!(
        undone.take_clips().unwrap(),
        published.take_clips().unwrap()
    );
    let redone = store.redo(undone.id(), undone.revision()).unwrap();
    assert_eq!(redone.name(), saved.name());
    assert_eq!(redone.schema_version(), 3);
    assert_eq!(fs::read(&source_path).unwrap(), original_bytes);
    drop(source);
    fs::rename(&source_path, f.0.join("original-parked.sqlite3")).unwrap();
    drop(store);
    let store = NativeStore::open_existing(f.root("source")).unwrap();
    assert_eq!(store.load(redone.id()).unwrap(), redone);
    let archive_path = f.0.join("empty-three.omniusong");
    let archive = store.export_archive(redone.id(), &archive_path).unwrap();
    assert_eq!(archive.take_count, 1);
    assert_eq!(files(&f.root("source").join("takes")), owned_before);
    let archive_bytes = fs::read(&archive_path).unwrap();
    let mut identity = f.store("identity");
    assert_eq!(identity.restore_archive(&archive_path).unwrap(), redone);
    assert_eq!(
        identity.load(redone.id()).unwrap().take_clips().unwrap(),
        vec![request.clip.clone()]
    );
    let mut copy = f.store("copy");
    let intent = RestoreCopyIntent {
        operation_id: OperationId("copy-empty-three".into()),
        expected_archive_sha256: archive.sha256,
    };
    let copied = copy
        .restore_archive_as_copy(&archive_path, &intent)
        .unwrap();
    assert!(!copied.replayed);
    assert_ne!(copied.document.id(), redone.id());
    assert_eq!(copied.document.schema_version(), 3);
    assert_eq!(copied.document.take_clips().unwrap(), vec![request.clip]);
    assert!(
        copied
            .document
            .performance_projections()
            .unwrap()
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(fs::read(&archive_path).unwrap(), archive_bytes);
    fs::rename(&archive_path, f.0.join("archive-parked.omniusong")).unwrap();
    let before_replay = catalog(&f.db("copy"));
    let replay = copy
        .restore_archive_as_copy(&archive_path, &intent)
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.document, copied.document);
    assert_eq!(catalog(&f.db("copy")), before_replay);
}

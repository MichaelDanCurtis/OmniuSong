//! Independent regression checks for the initial native persistence review.
//! Every fixture is synthetic and isolated from the legacy data directory.

use omniusong_core::ProjectDocument;
use omniusong_store::{NativeStore, StoreError};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const ABC_ASSET: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.wav";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/review-regressions")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("source-assets")).unwrap();
        Self(root)
    }

    fn store(&self) -> NativeStore {
        NativeStore::open(self.0.join("native")).unwrap()
    }

    fn db(&self) -> Connection {
        Connection::open(self.0.join("native/studio.sqlite3")).unwrap()
    }

    fn write_asset(&self) {
        fs::write(self.0.join("source-assets").join(ABC_ASSET), b"abc").unwrap();
    }

    fn write_json(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Failed review fixture retained at {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[derive(Debug, PartialEq)]
struct JournalState {
    project: (String, i64, i64),
    edits: Vec<(i64, String, String)>,
}

fn journal_state(db: &Connection, id: &str) -> JournalState {
    let project = db
        .query_row(
            "SELECT payload,revision,cursor FROM projects WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let mut statement = db
        .prepare("SELECT sequence,before_json,after_json FROM edits WHERE project_id=?1 ORDER BY sequence")
        .unwrap();
    let edits = statement
        .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    JournalState { project, edits }
}

fn table_counts(db: &Connection) -> Vec<i64> {
    ["projects", "assets", "imports", "edits"]
        .iter()
        .map(|table| {
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        })
        .collect()
}

fn rejects_foreign_history(redo: bool) {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let original = store
        .create(&ProjectDocument::new("Intended project").unwrap())
        .unwrap();
    let mut edit = original.clone();
    edit.rename("Committed edit").unwrap();
    let saved = store.save(&edit, 0).unwrap();
    let current = if redo {
        store.undo(original.id(), 1).unwrap()
    } else {
        saved
    };
    drop(store);

    let foreign = ProjectDocument::new("Foreign journal payload").unwrap();
    let db = fixture.db();
    let column = if redo { "after_json" } else { "before_json" };
    db.execute(
        &format!("UPDATE edits SET {column}=?1 WHERE project_id=?2 AND sequence=1"),
        params![
            String::from_utf8(foreign.to_json().unwrap()).unwrap(),
            original.id()
        ],
    )
    .unwrap();
    let before = journal_state(&db, original.id());
    drop(db);

    let mut store = fixture.store();
    let result = if redo {
        store.redo(original.id(), current.revision())
    } else {
        store.undo(original.id(), current.revision())
    };
    assert!(matches!(result, Err(StoreError::Invalid(_))));
    assert_eq!(store.load(original.id()).unwrap(), current);
    drop(store);
    assert_eq!(journal_state(&fixture.db(), original.id()), before);
    let reopened = fixture.store();
    assert_eq!(reopened.load(original.id()).unwrap(), current);
    assert_eq!(journal_state(&fixture.db(), original.id()), before);
}

#[test]
fn foreign_undo_snapshot_cannot_change_payload_revision_cursor_or_journal() {
    rejects_foreign_history(false);
}

#[test]
fn foreign_redo_snapshot_cannot_change_payload_revision_cursor_or_journal() {
    rejects_foreign_history(true);
}

fn assert_opaque_values(document: &ProjectDocument) {
    let metadata = &document.value()["vendor:opaque"];
    assert_eq!(metadata["integer"].to_string(), "18446744073709551617");
    assert_eq!(
        metadata["decimal"].to_string(),
        "0.12345678901234567890123456789"
    );
    assert_eq!(metadata["assetId"], "remote-provider-42");
    assert_eq!(metadata["sourceAssetId"], 17);
    assert_eq!(metadata["looksLocal"], ABC_ASSET);
    assert_eq!(
        document.value()["nativeImport"]["previousImport"]["foreign"],
        "preserve this earlier import"
    );
}

#[test]
fn literal_import_precision_and_prior_lineage_survive_save_undo_redo_and_reopen() {
    let fixture = Fixture::new();
    let mut value = ProjectDocument::new("Literal source")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["notes"] = json!([{
        "id":"preserved-note", "pitch":60, "startTick":9, "durationTicks":960,
        "velocity16":65535, "expression":[{"type":"pitch","points":[
            {"tick":0,"value32":2147483648u32}, {"tick":960,"value32":4294967295u32}
        ]}], "vendor:estimate":{"confidence":0.75}
    }]);
    let base = serde_json::to_string(&value).unwrap();
    // The high-precision numbers enter as literal JSON bytes, never an f64.
    let raw = format!(
        "{},\"vendor:opaque\":{{\"integer\":18446744073709551617,\"decimal\":0.12345678901234567890123456789,\"assetId\":\"remote-provider-42\",\"sourceAssetId\":17,\"looksLocal\":\"{ABC_ASSET}\"}},\"nativeImport\":{{\"foreign\":\"preserve this earlier import\"}}}}\n",
        base.strip_suffix('}').unwrap()
    );
    let source = fixture.0.join("literal-source.json");
    fs::write(&source, &raw).unwrap();
    let mut store = fixture.store();
    let imported = store
        .import_legacy(&source, &fixture.0.join("source-assets"))
        .unwrap();
    assert_opaque_values(&imported);
    assert!(imported.referenced_assets().unwrap().is_empty());
    assert_eq!(
        store.imported_original(imported.id()).unwrap().unwrap(),
        raw.as_bytes()
    );
    let mut edited = imported.clone();
    edited.rename("Renamed native copy").unwrap();
    let saved = store.save(&edited, 0).unwrap();
    assert_opaque_values(&saved);
    assert_eq!(saved.tracks(), imported.tracks());
    drop(store);
    let mut store = fixture.store();
    assert_opaque_values(&store.load(imported.id()).unwrap());
    let undone = store.undo(imported.id(), 1).unwrap();
    assert_eq!(undone.name(), "Literal source");
    assert_opaque_values(&undone);
    assert_eq!(undone.tracks(), imported.tracks());
    drop(store);
    let mut store = fixture.store();
    assert_opaque_values(&store.load(imported.id()).unwrap());
    let redone = store.redo(imported.id(), 2).unwrap();
    assert_eq!(redone.name(), "Renamed native copy");
    assert_opaque_values(&redone);
    assert_eq!(redone.tracks(), imported.tracks());
    drop(store);
    let store = fixture.store();
    assert_eq!(store.load(imported.id()).unwrap(), redone);
    assert_opaque_values(&store.load(imported.id()).unwrap());
    assert_eq!(
        store.imported_original(imported.id()).unwrap().unwrap(),
        raw.as_bytes()
    );
    assert_eq!(fs::read(&source).unwrap(), raw.as_bytes());
    assert_eq!(table_counts(&fixture.db()), [1, 0, 1, 1]);

    // A further import keeps the previous lineage nested rather than replacing it.
    let exported = fixture.0.join("native-copy.json");
    let second_raw = redone.to_json().unwrap();
    fs::write(&exported, &second_raw).unwrap();
    drop(store);
    let mut store = fixture.store();
    let copied = store
        .import_legacy(&exported, &fixture.0.join("source-assets"))
        .unwrap();
    assert_eq!(
        copied.value()["vendor:opaque"],
        redone.value()["vendor:opaque"]
    );
    assert_eq!(
        copied.value()["nativeImport"]["previousImport"],
        redone.value()["nativeImport"]
    );
    assert_eq!(
        store.imported_original(copied.id()).unwrap().unwrap(),
        second_raw
    );
    drop(store);
    assert_eq!(fixture.store().load(copied.id()).unwrap(), copied);
}

fn valid_audio_source() -> Value {
    let mut value = ProjectDocument::new("Audio fixture")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["kind"] = "audio".into();
    value["tracks"][0]["instrument"] = "voice".into();
    value["tracks"][0]["assetId"] = ABC_ASSET.into();
    value["tracks"][0]["audio"] = json!({
        "origin":"import", "mimeType":"audio/wav", "durationSeconds":2.0,
        "byteLength":3, "startTick":3840, "trimStartSeconds":0.25,
        "trimEndSeconds":1.75, "fadeInSeconds":0.25, "fadeOutSeconds":0.5,
        "editSource":{"version":1,"original":{"startTick":0}}
    });
    value
}

#[test]
fn malformed_clip_transport_and_timing_imports_leave_no_partial_rows() {
    let fixture = Fixture::new();
    fixture.write_asset();
    let valid = valid_audio_source();
    assert!(ProjectDocument::from_value(valid.clone()).is_ok());
    let mut cases = Vec::new();
    for (field, bad) in [
        ("trimStartSeconds", json!(-100)),
        ("trimEndSeconds", json!(500)),
        ("fadeInSeconds", json!(-1)),
        ("fadeOutSeconds", json!(2)),
    ] {
        let mut value = valid.clone();
        value["tracks"][0]["audio"][field] = bad;
        cases.push((field, value));
    }
    let mut value = valid.clone();
    value["tracks"][0]["audio"]["editSource"]["version"] = 77.into();
    cases.push(("original-version", value));
    let mut value = valid.clone();
    value["tracks"][0]["audio"]["editSource"]["original"]["trimEndSeconds"] = 8.into();
    cases.push(("invalid-original-window", value));
    let mut value = valid.clone();
    value["transport"] = json!({"version":1,"metronome":true,"countInBars":3,"clickLevel":0.5});
    cases.push(("transport", value));
    let mut value = valid.clone();
    value["tracks"][0]["audio"]["origin"] = "microphone".into();
    value["tracks"][0]["audio"]["recordingTiming"] = json!({
        "version":1,"method":"browser-recorder-start-call","calibration":"unmeasured",
        "captureAudioTimeSeconds":8.0,"songStartAudioTimeSeconds":9.0,
        "tempo":120,"ppq":960,"loopTicks":15360,"status":"calibrated"
    });
    cases.push(("recording-timing-status", value));
    let mut store = fixture.store();
    for (name, value) in cases {
        let source = fixture.write_json(&format!("bad-{name}.json"), &value);
        let bytes = fs::read(&source).unwrap();
        assert!(
            store
                .import_legacy(&source, &fixture.0.join("source-assets"))
                .is_err(),
            "accepted {name}"
        );
        assert!(store.list().unwrap().is_empty(), "published {name}");
        assert_eq!(
            table_counts(&fixture.db()),
            [0, 0, 0, 0],
            "partial rows after {name}"
        );
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
    let source = fixture.write_json("valid-audio.json", &valid);
    let imported = store
        .import_legacy(&source, &fixture.0.join("source-assets"))
        .unwrap();
    assert_eq!(imported.tracks()[0]["audio"], valid["tracks"][0]["audio"]);
    assert_eq!(store.asset(ABC_ASSET).unwrap(), b"abc");
    drop(store);
    assert_eq!(fixture.store().load(imported.id()).unwrap(), imported);
    assert_eq!(
        fs::read(fixture.0.join("source-assets").join(ABC_ASSET)).unwrap(),
        b"abc"
    );
}

#[test]
fn damaged_listing_rows_are_visible_without_hiding_or_rewriting_healthy_data() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let good = store
        .create(&ProjectDocument::new("Healthy").unwrap())
        .unwrap();
    let malformed = store
        .create(&ProjectDocument::new("Bad JSON").unwrap())
        .unwrap();
    let bad_id = store
        .create(&ProjectDocument::new("Bad identity").unwrap())
        .unwrap();
    let bad_revision = store
        .create(&ProjectDocument::new("Bad revision").unwrap())
        .unwrap();
    drop(store);
    let db = fixture.db();
    db.execute(
        "UPDATE projects SET payload='broken' WHERE id=?1",
        [malformed.id()],
    )
    .unwrap();
    let unrelated = ProjectDocument::new("Other identity").unwrap();
    let wrong_id_payload = String::from_utf8(unrelated.to_json().unwrap()).unwrap();
    db.execute(
        "UPDATE projects SET payload=?1 WHERE id=?2",
        params![wrong_id_payload, bad_id.id()],
    )
    .unwrap();
    db.execute(
        "UPDATE projects SET revision=7 WHERE id=?1",
        [bad_revision.id()],
    )
    .unwrap();
    let originals: Vec<_> = [&good, &malformed, &bad_id, &bad_revision]
        .iter()
        .map(|p| (p.id().to_owned(), journal_state(&db, p.id())))
        .collect();
    drop(db);
    let store = fixture.store();
    let summaries = store.list().unwrap();
    assert_eq!(summaries.len(), 4);
    let summary = summaries.iter().find(|p| p.id == good.id()).unwrap();
    assert_eq!(summary.name, "Healthy");
    assert!(summary.problem.is_none());
    assert_eq!(store.load(good.id()).unwrap(), good);
    for damaged in [&malformed, &bad_id, &bad_revision] {
        assert!(
            summaries
                .iter()
                .find(|p| p.id == damaged.id())
                .unwrap()
                .problem
                .is_some()
        );
        assert!(store.load(damaged.id()).is_err());
    }
    drop(store);
    let db = fixture.db();
    for (id, before) in originals {
        assert_eq!(journal_state(&db, &id), before, "listing rewrote {id}");
    }
}

#[test]
fn final_import_receipt_failure_rolls_back_assets_and_project_together() {
    let fixture = Fixture::new();
    fixture.write_asset();
    let source = fixture.write_json("source.json", &valid_audio_source());
    let original_bytes = fs::read(&source).unwrap();
    let mut store = fixture.store();
    let healthy = store
        .create(&ProjectDocument::new("Earlier commit").unwrap())
        .unwrap();
    drop(store);
    let db = fixture.db();
    db.execute_batch("CREATE TRIGGER fail_review_receipt BEFORE INSERT ON imports BEGIN SELECT RAISE(ABORT, 'review receipt fault'); END;").unwrap();
    drop(db);
    let mut store = fixture.store();
    assert!(matches!(
        store.import_legacy(&source, &fixture.0.join("source-assets")),
        Err(StoreError::Database(_))
    ));
    assert_eq!(store.load(healthy.id()).unwrap(), healthy);
    assert_eq!(table_counts(&fixture.db()), [1, 0, 0, 0]);
    drop(store);
    assert_eq!(fixture.store().load(healthy.id()).unwrap(), healthy);
    assert_eq!(table_counts(&fixture.db()), [1, 0, 0, 0]);
    assert_eq!(fs::read(&source).unwrap(), original_bytes);
    assert_eq!(
        fs::read(fixture.0.join("source-assets").join(ABC_ASSET)).unwrap(),
        b"abc"
    );
}

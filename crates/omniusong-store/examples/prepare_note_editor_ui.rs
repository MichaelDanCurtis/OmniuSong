//! Append ONE synthetic native note-editor session; never edit existing sessions.
//!
//! Usage: prepare_note_editor_ui LIVE_STORE_ROOT FRESH_RECEIPT_JSON
//! Both directories must exist. The receipt must be outside the native root.
//! A fresh sibling `note-editor-ui-<uuid>` directory retains the exact canonical
//! r0 JSON and complete source archive. The receipt is reserved with create_new
//! before opening the existing-only store. This current-schema fixture never migrates.
//!
//! ONLY the coordinator should run this against live storage after review.
//! No device, model, human performance, MIDI delivery, listening or UI acceptance
//! is implied. The expressive/out-of-loop track is muted deliberately. Failures
//! retain artifacts and any created synthetic project; there is no deletion or
//! rollback across receipt/archive/filesystem failures. A pending-create receipt
//! means inspect its known project ID before deciding whether to run again.
use omniusong_core::ProjectDocument;
use omniusong_store::NativeStore;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

type Outcome<T> = Result<T, Box<dyn Error>>;
const MAX_FIXTURE_BYTES: usize = 256 * 1024;
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_sha(path: &Path) -> Outcome<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn directory(path: &Path) -> Outcome<PathBuf> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(format!(
            "{} must be an existing ordinary non-symlink directory",
            path.display()
        )
        .into());
    }
    Ok(fs::canonicalize(path)?)
}
fn catalog(path: &Path) -> Outcome<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() == 0 {
        return Err("Expected an existing ordinary nonempty native catalog".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err("Hard-linked catalogs are not accepted by this fixture".into());
        }
    }
    // Avoid an implicit migration in NativeStore::open_existing. No CREATE flag,
    // schema writes, integrity repair, source import or catalog replacement.
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(2))?;
    let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != omniusong_store::STORE_SCHEMA_VERSION {
        return Err(
            "This fixture requires the current current-schema native catalog; it will not migrate it"
                .into(),
        );
    }
    Ok(())
}
fn fresh_directory(path: &Path) -> Outcome<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Outcome<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn write_receipt(file: &mut File, value: &Value) -> Outcome<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() > MAX_FIXTURE_BYTES {
        return Err("Fixture receipt exceeds its bounded size".into());
    }
    // This is the original, exclusively reserved new receipt inode, never an
    // existing user receipt. A failed update does not roll back a committed row.
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
fn note(id: &str, pitch: u8, start: u64, duration: u64, velocity: u16) -> Value {
    json!({"id":id,"pitch":pitch,"startTick":start,"durationTicks":duration,"velocity16":velocity,"expression":[]})
}
fn source_project() -> Outcome<ProjectDocument> {
    let session_uuid = uuid::Uuid::new_v4().to_string();
    let mut value = ProjectDocument::new(&format!(
        "SYNTHETIC note editor walkthrough {}",
        &session_uuid[..8]
    ))?
    .value()
    .clone();
    value["tempo"] = json!(119.25);
    value["bars"] = 4.into();
    let playable_id = uuid::Uuid::new_v4().to_string();
    let legacy_id = "synthetic:legacy:🎹\0\n";
    let playable = json!({"id":playable_id,"name":"SYNTHETIC · playable keys (no expression)","kind":"notes","instrument":"keys","gain":0.25,"muted":false,"locked":false,"armed":false,"notes":[
        note(&uuid::Uuid::new_v4().to_string(),60,123,987,51313),
        note(&uuid::Uuid::new_v4().to_string(),60,123,701,0),
        note(&uuid::Uuid::new_v4().to_string(),60,123,433,u16::MAX),
        note(&uuid::Uuid::new_v4().to_string(),67,2357,997,19001)
    ]});
    let opaque: Value = serde_json::from_str(
        r#"{"largeInteger":184467440737095516160000001,"longDecimal":0.123456789012345678901234567890123456789,"uuidLooking":"00000000-0000-4000-8000-000000000001","namespace":"synthetic vendor data; keep exact"}"#,
    )?;
    let expression = json!([
        {"type":"pitch","points":[{"tick":0,"value32":2147483648u32},{"tick":699,"value32":2271560481u32},{"tick":987,"value32":4294967295u32}],"vendor":"Synthetic pitch fixture"},
        {"type":"pressure","points":[{"tick":123,"value32":305419896u32},{"tick":877,"value32":4000000001u32}]}
    ]);
    let input = json!({"sourceId":"synthetic:ump-fixture","sourceName":"Synthetic UMP fixture · no device opened","protocol":"midi2","group":3,"channel":2,"receivedTimeMs":12.345,"keyDurationTicks":877,"releaseVelocity16":37155,"endReason":"synthetic-fixture-release","fixtureOnly":true,"human":false});
    let mut expressive = note("旧ノート\0\n", 62, 321, 1201, 51313);
    expressive["expression"] = expression.clone();
    expressive["input"] = input;
    expressive["vendor:opaque"] = opaque.clone();
    expressive["provenance"] = json!({"synthetic":true,"human":false,"deviceOpened":false,"inference":false,"scenario":"Pre-edited source state authored by fixture; not evidence of an earlier physical performance or UI Save"});
    let source = json!({"version":1,"pitch":60,"startTick":123,"durationTicks":987,"velocity16":49123,"expression":expression,"vendor:original":opaque});
    expressive["editSource"] = source;
    let mut out_of_loop = note(
        &uuid::Uuid::new_v4().to_string(),
        71,
        128 * 3840 + 123,
        777,
        u16::MAX,
    );
    // Exercise a genuinely absent expression field separately from [].
    out_of_loop
        .as_object_mut()
        .expect("fixture note object")
        .remove("expression");
    out_of_loop["fixturePurpose"] =
        "SYNTHETIC out-of-loop note; explicit ExtendLoop requires 129 bars and schema-2 consent"
            .into();
    let expressive_track = json!({"id":legacy_id,"name":"SYNTHETIC · expression and legacy IDs (muted)","kind":"notes","instrument":"keys","gain":0.25,"muted":true,"locked":false,"armed":false,"notes":[expressive,out_of_loop],"provenance":{"synthetic":true,"human":false,"deviceOpened":false}});
    value["tracks"] = json!([playable, expressive_track]);
    let source_name = "Synthetic raw UMP fixture · not a delivered controller event";
    value["inputCapture"] = json!({"version":1,"droppedEvents":0,"fixtureOnly":true,"actualDeviceInput":false,"timing":"Synthetic uncalibrated observations, not native host measurements","events":[
        {"trackId":legacy_id,"tick":123,"sourceId":"synthetic:ump-fixture","sourceName":source_name,"receivedTimeMs":12.345,"hostTime":"9007199254740993","format":"ump","words":[0x43923c03u32,(49123u32<<16)|0xabcdu32]},
        {"trackId":legacy_id,"tick":1110,"sourceId":"synthetic:ump-fixture","sourceName":source_name,"receivedTimeMs":145.678,"hostTime":"9007199254741993","format":"ump","words":[0x43823c03u32,(37155u32<<16)|0x1234u32]}
    ]});
    value["syntheticNoteEditorFixture"] = json!({"version":1,"synthetic":true,"human":false,"deviceOpened":false,"inference":false,"rawPacketsAreFixtureOnly":true,"sourceTimingCalibrated":false,"description":"One isolated native note editor walkthrough; original sessions are not edited"});
    let document = ProjectDocument::from_value(value)?;
    if document.to_json()?.len() > MAX_FIXTURE_BYTES {
        return Err("Synthetic fixture exceeds its size bound".into());
    }
    Ok(document)
}
fn track_receipts(document: &ProjectDocument) -> Outcome<Value> {
    let mut tracks = Vec::new();
    for track in document.tracks() {
        let mut notes = Vec::new();
        for note in track["notes"]
            .as_array()
            .ok_or("Fixture notes are invalid")?
        {
            let raw = serde_json::to_string(note)?;
            let expression_presence = match note.get("expression") {
                None => "absent",
                Some(Value::Null) => "null",
                Some(Value::Array(points)) if points.is_empty() => "empty_array",
                Some(_) => "present",
            };
            notes.push(json!({"noteId":note["id"],"pitch":note["pitch"],"startTick":note["startTick"],"durationTicks":note["durationTicks"],"velocity16":note["velocity16"],"expressionPresence":expression_presence,"canonicalNoteJson":raw,"canonicalNoteSha256":sha(raw.as_bytes()),"editSource":note.get("editSource")}));
        }
        tracks.push(json!({"trackId":track["id"],"name":track["name"],"muted":track["muted"],"notes":notes}));
    }
    Ok(Value::Array(tracks))
}
fn main() -> Outcome<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [root_arg, receipt_arg] = args.as_slice() else {
        return Err("usage: prepare_note_editor_ui LIVE_STORE_ROOT FRESH_RECEIPT_JSON".into());
    };
    let root = directory(root_arg)?;
    let catalog_path = root.join("studio.sqlite3");
    let receipt_name = receipt_arg.file_name().ok_or("Receipt needs a filename")?;
    let parent = directory(
        receipt_arg
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let receipt_path = parent.join(receipt_name);
    if receipt_path.starts_with(&root) {
        return Err("Receipt and task artifacts must be outside the native store".into());
    }
    if root.to_str().is_none() || receipt_path.to_str().is_none() {
        return Err("This fixture requires UTF-8 filesystem paths".into());
    }
    // Existing receipt/dangling symlink fails BEFORE any store open or writes.
    let mut receipt_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)?;
    let artifact_dir = parent.join(format!("note-editor-ui-{}", uuid::Uuid::new_v4()));
    let source_path = artifact_dir.join("source-project.json");
    let archive_path = artifact_dir.join("source.omniusong-native");
    let source = source_project()?;
    let bytes = source.to_json()?;
    let raw_capture = serde_json::to_string(&source.value()["inputCapture"])?;
    let mut receipt = json!({"format":"omniusong.note-editor-ui-fixture","version":1,"phase":"prepared_not_published","projectId":source.id(),"projectName":source.name(),"sourceRevision":0,"liveStoreRoot":root,"catalogPath":catalog_path,"artifactDirectory":artifact_dir,"receiptPath":receipt_path});
    let provenance = json!({"synthetic":true,"human":false,"deviceOpened":false,"nativeInputOpened":false,"inference":false,"uiOperated":false,"audioPlayed":false,"rawPacketsAreFixtureOnly":true,"existingOnlyStoreOpen":true,"existingDocumentsEdited":false,"exampleSourceSha256":sha(include_bytes!("prepare_note_editor_ui.rs"))});
    let evidence = json!({"sourceProjectJson":{"path":source_path,"sha256":sha(&bytes),"byteLength":bytes.len()},"tracks":track_receipts(&source)?,"inputCaptureCanonicalJson":raw_capture,"inputCaptureSha256":sha(raw_capture.as_bytes()),"outOfLoop":{"trackId":source.tracks()[1]["id"],"noteId":source.tracks()[1]["notes"][1]["id"],"startTick":128*3840+123,"durationTicks":777,"requiredBars":129,"initialBars":4,"initialSchemaVersion":1}});
    let limits = json!({"limitations":["All notes, expressive values, input metadata, UMP words and clock observations are synthetic fixtures; no human performance or controller delivery is asserted.","The muted source/expression track includes an out-of-loop 129-bar note. It is excluded by saved mute state; expression and long playback are not newly supported.","The unmuted procedural keys track is suitable for device-free comparisons within existing renderer limits; this CLI does not render or play it.","Only one new project is created. Receipt/archive failure does not roll back that project: retain this receipt, source JSON and known ID, then inspect saved state. No project is deleted.","A complete source archive is retained only after phase complete. No OS crash, power-loss, hardware MIDI, inference, listening, latency, actual UI or Windows qualification follows from this fixture."]});
    for section in [provenance, evidence, limits] {
        receipt
            .as_object_mut()
            .ok_or("Receipt object missing")?
            .extend(
                section
                    .as_object()
                    .ok_or("Receipt section missing")?
                    .clone(),
            );
    }
    write_receipt(&mut receipt_file, &receipt)?;
    catalog(&catalog_path)?;
    fresh_directory(&artifact_dir)?;
    write_new(&source_path, &bytes)?;
    #[cfg(unix)]
    File::open(&artifact_dir)?.sync_all()?;
    // Repeat the existing-root/header guard after artifact work. No mkdir -p.
    if directory(&root)? != root {
        return Err("Native root changed during fixture preflight".into());
    }
    catalog(&catalog_path)?;
    receipt["phase"] = "create_pending_inspect_known_project_id_if_interrupted".into();
    write_receipt(&mut receipt_file, &receipt)?;
    eprintln!(
        "Synthetic-only project create attempt {} · source retained at {}",
        source.id(),
        source_path.display()
    );
    let mut store = NativeStore::open_existing(&root)?;
    let saved = match store.create(&source) {
        Ok(saved) => saved,
        Err(error) => {
            receipt["phase"] = "create_failed_or_uncertain_inspect_known_project_id".into();
            receipt["error"] = error.to_string().into();
            let _ = write_receipt(&mut receipt_file, &receipt);
            return Err(error.into());
        }
    };
    eprintln!(
        "CREATED synthetic project {}. It remains saved if a later receipt/archive write fails.",
        saved.id()
    );
    receipt["phase"] = "project_created_archive_pending".into();
    write_receipt(&mut receipt_file, &receipt)?;
    if saved.to_json()? != bytes {
        return Err("Saved synthetic source differs from retained canonical original; keep project/artifacts for inspection".into());
    }
    let archive = match store.export_archive(saved.id(), &archive_path) {
        Ok(archive) => archive,
        Err(error) => {
            receipt["phase"] = "project_created_archive_failed".into();
            receipt["error"] = error.to_string().into();
            let _ = write_receipt(&mut receipt_file, &receipt);
            return Err(error.into());
        }
    };
    if archive.history_rows != 0
        || archive.operation_rows != 0
        || archive.asset_count != 0
        || archive.take_count != 0
    {
        return Err(
            "Unexpected source archive closure; synthetic project/artifacts retained".into(),
        );
    }
    if file_sha(&source_path)? != sha(&bytes) || file_sha(&archive_path)? != archive.sha256 {
        return Err("Retained synthetic evidence hash mismatch; do not delete the project".into());
    }
    receipt["sourceArchive"] = json!({"path":archive.path,"sha256":archive.sha256,"byteLength":archive.byte_length,"historyRows":archive.history_rows,"operationRows":archive.operation_rows,"assetCount":archive.asset_count,"takeCount":archive.take_count});
    receipt["phase"] = "complete".into();
    receipt["savedRevision"] = saved.revision().into();
    write_receipt(&mut receipt_file, &receipt)?;
    #[cfg(unix)]
    {
        File::open(&artifact_dir)?.sync_all()?;
        File::open(&parent)?.sync_all()?;
    }
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}

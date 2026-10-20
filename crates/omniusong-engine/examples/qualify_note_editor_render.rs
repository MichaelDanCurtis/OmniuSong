//! Device-free agreement between two caller-supplied validated project snapshots.
//!
//! Usage: qualify_note_editor_render EXISTING_STORE ORIGINAL_JSON EDITED_JSON
//!                                  FRESH_OUTPUT_DIR NEW_RECEIPT
//! Uses production prepare -> RenderCursor at 48 kHz stereo; opens no device,
//! plays/captures nothing and calls no store mutation API. The store must already
//! be schema 4 in clean rollback-journal mode, preventing opening-time migration
//! or hot-journal recovery. NativeStore still uses its existing READ_WRITE handle;
//! this is not an OS-enforced read-only capability. Catalog/source hashes must be
//! unchanged at success. Use stable, coordinator-owned paths, not a hostile
//! same-user namespace. Failed runs retain only their own new partial outputs.
//!
//! PCM equality/difference is reported honestly: edits confined to muted tracks
//! can leave audio unchanged. This does not prove a JSON file came from a UI
//! commit; the coordinator binds snapshot hashes to the actual saved revision.
use omniusong_core::{MAX_NATIVE_PROJECT_BYTES, ProjectDocument};
use omniusong_engine::{realtime::RenderCursor, render};
use omniusong_store::NativeStore;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

type Outcome<T> = Result<T, Box<dyn Error>>;
const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const CHUNK_SAMPLES: usize = 8192;
const WAV_HEADER: u64 = 44;
const MAX_DETAILS: usize = 32;
const MAX_RECEIPT: usize = 256 * 1024;
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_hash(path: &Path) -> Outcome<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn ordinary_file(path: &Path) -> Outcome<PathBuf> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(format!("{} must be an existing ordinary file", path.display()).into());
    }
    Ok(fs::canonicalize(path)?)
}
fn directory(path: &Path) -> Outcome<PathBuf> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(format!("{} must be an existing ordinary directory", path.display()).into());
    }
    Ok(fs::canonicalize(path)?)
}
fn new_path(path: &Path) -> Outcome<PathBuf> {
    let name = path.file_name().ok_or("New path needs a leaf name")?;
    let parent = directory(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let resolved = parent.join(name);
    match fs::symlink_metadata(&resolved) {
        Ok(_) => Err(format!("{} already exists", resolved.display()).into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(resolved),
        Err(e) => Err(e.into()),
    }
}
fn clean_catalog(root: &Path) -> Outcome<PathBuf> {
    let path = ordinary_file(&root.join("studio.sqlite3"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(&path)?.nlink() != 1 {
            return Err("Hard-linked resolver catalogs are not accepted".into());
        }
    }
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = root.join(format!("studio.sqlite3{suffix}"));
        match fs::symlink_metadata(&sidecar) {
            Ok(_) => return Err(
                "Resolver catalog has a journal/WAL sidecar; close writers before qualification"
                    .into(),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let mut header = [0u8; 100];
    File::open(&path)?.read_exact(&mut header)?;
    if &header[..16] != b"SQLite format 3\0"
        || header[18] != 1
        || header[19] != 1
        || u32::from_be_bytes(header[60..64].try_into()?) != 4
    {
        return Err("Resolver catalog must already be schema 4, rollback-journal SQLite; no migrations/recovery requested".into());
    }
    Ok(path)
}
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn value_hash(value: &Value) -> Outcome<String> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(format!("{:x}", writer.0.finalize()))
}
struct Loaded {
    document: ProjectDocument,
    evidence: Value,
    path: PathBuf,
    raw_sha: String,
}
fn load(path: &Path) -> Outcome<Loaded> {
    let path = ordinary_file(path)?;
    let mut bytes = Vec::new();
    File::open(&path)?
        .take(MAX_NATIVE_PROJECT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_NATIVE_PROJECT_BYTES {
        return Err("Project JSON exceeds the native metadata cap".into());
    }
    let raw_sha = hash(&bytes);
    let document = ProjectDocument::from_json(&bytes)?;
    let canonical = document.to_json()?;
    let evidence = json!({"path":path,"rawByteLength":bytes.len(),"rawSha256":raw_sha,"canonicalSha256":hash(&canonical),"id":document.id(),"revision":document.revision(),"schemaVersion":document.schema_version()});
    Ok(Loaded {
        document,
        evidence,
        path,
        raw_sha,
    })
}
fn equal_except(before: &Value, after: &Value, except: &[&str]) -> bool {
    let (Some(a), Some(b)) = (before.as_object(), after.as_object()) else {
        return before == after;
    };
    a.iter()
        .filter(|(key, _)| !except.contains(&key.as_str()))
        .all(|(key, value)| b.get(key) == Some(value))
        && b.keys()
            .filter(|key| !except.contains(&key.as_str()))
            .all(|key| a.contains_key(key))
}
fn note_values(note: &Value) -> Value {
    json!({"pitch":note["pitch"],"startTick":note["startTick"],"durationTicks":note["durationTicks"],"velocity16":note["velocity16"]})
}
fn note_facts(
    track_id: &str,
    id: &str,
    before: Option<&Value>,
    after: Option<&Value>,
) -> Outcome<Value> {
    let mut fact = json!({"trackId":track_id,"noteId":id,"change":match(before,after){(None,_)=>"added",(_,None)=>"removed",_=>"modified"},"before":before.map(note_values),"after":after.map(note_values)});
    if let (Some(before), Some(after)) = (before, after) {
        fact["expressionOptionEqual"] = json!(before.get("expression") == after.get("expression"));
        fact["editSourceOptionEqual"] = json!(before.get("editSource") == after.get("editSource"));
        fact["otherFieldsExceptFourValuesAndEditSourceEqual"] = json!(equal_except(
            before,
            after,
            &[
                "pitch",
                "startTick",
                "durationTicks",
                "velocity16",
                "editSource"
            ]
        ));
        fact["firstSnapshotMatchesPriorValuesAndExpression"] = if before.get("editSource").is_none()
        {
            match after.get("editSource") {
                Some(source) => json!(
                    source["version"] == 1
                        && ["pitch", "startTick", "durationTicks", "velocity16"]
                            .iter()
                            .all(|field| source.get(*field) == before.get(*field))
                        && source.get("expression") == before.get("expression")
                ),
                None => Value::Null,
            }
        } else {
            Value::Null
        };
    }
    fact["beforeCanonicalNoteSha256"] = match before {
        Some(note) => json!(value_hash(note)?),
        None => Value::Null,
    };
    fact["afterCanonicalNoteSha256"] = match after {
        Some(note) => json!(value_hash(note)?),
        None => Value::Null,
    };
    Ok(fact)
}
fn document_facts(original: &ProjectDocument, edited: &ProjectDocument) -> Outcome<Value> {
    let before = original.value();
    let after = edited.value();
    let mut details = Vec::new();
    let mut changed_notes = 0u64;
    let mut tracks = Vec::new();
    let mut uninterpreted = 0u64;
    let original_tracks: BTreeMap<&str, &Value> = original
        .tracks()
        .iter()
        .map(|t| (t["id"].as_str().expect("validated track ID"), t))
        .collect();
    let edited_tracks: BTreeMap<&str, &Value> = edited
        .tracks()
        .iter()
        .map(|t| (t["id"].as_str().expect("validated track ID"), t))
        .collect();
    for (id, track) in &original_tracks {
        let Some(next) = edited_tracks.get(id) else {
            if tracks.len() < MAX_DETAILS {
                tracks.push(json!({"trackId":id,"change":"removed"}));
            }
            uninterpreted += 1;
            continue;
        };
        if *track == *next {
            continue;
        }
        if tracks.len() < MAX_DETAILS {
            tracks.push(json!({"trackId":id,"change":"modified","fieldsExceptNotesEqual":equal_except(track,next,&["notes"])}));
        }
        if track["kind"] != "notes" || next["kind"] != "notes" {
            uninterpreted += 1;
            continue;
        }
        let a: BTreeMap<&str, &Value> = track["notes"]
            .as_array()
            .ok_or("Invalid original note array")?
            .iter()
            .map(|n| (n["id"].as_str().expect("validated note ID"), n))
            .collect();
        let b: BTreeMap<&str, &Value> = next["notes"]
            .as_array()
            .ok_or("Invalid edited note array")?
            .iter()
            .map(|n| (n["id"].as_str().expect("validated note ID"), n))
            .collect();
        for (note_id, note) in &a {
            let right = b.get(note_id).copied();
            if right == Some(*note) {
                continue;
            }
            changed_notes += 1;
            if details.len() < MAX_DETAILS {
                details.push(note_facts(id, note_id, Some(note), right)?);
            }
        }
        for (note_id, note) in &b {
            if a.contains_key(note_id) {
                continue;
            }
            changed_notes += 1;
            if details.len() < MAX_DETAILS {
                details.push(note_facts(id, note_id, None, Some(note))?);
            }
        }
    }
    for id in edited_tracks.keys() {
        if !original_tracks.contains_key(id) {
            uninterpreted += 1;
            if tracks.len() < MAX_DETAILS {
                tracks.push(json!({"trackId":id,"change":"added"}));
            }
        }
    }
    Ok(
        json!({"rootFieldsExceptRevisionAndTracksEqual":equal_except(before,after,&["revision","tracks"]),"trackOrderEqual":original.tracks().iter().map(|t|&t["id"]).eq(edited.tracks().iter().map(|t|&t["id"])),"inputCaptureOptionEqual":before.get("inputCapture")==after.get("inputCapture"),"changedNoteCountOnPairedNoteTracks":changed_notes,"noteDetails":details,"noteDetailsOmitted":changed_notes.saturating_sub(MAX_DETAILS as u64),"changedTrackDetails":tracks,"uninterpretedAddedRemovedOrNonNoteTrackChanges":uninterpreted,"noteFactScope":"Exact Value comparisons on paired kind:notes tracks. Added/removed/non-note tracks are not reinterpreted as note events."}),
    )
}
#[derive(Serialize)]
struct AudioReceipt {
    path: PathBuf,
    frames: u64,
    samples: u64,
    pcm_bytes: u64,
    peak: f32,
    pcm_sha256: String,
    file_sha256: String,
    file_bytes: u64,
}
#[derive(Default, Serialize)]
struct Difference {
    overlap_changed_samples: u64,
    overlap_changed_frames: u64,
    first_changed_frame: Option<u64>,
    original_only_frames: u64,
    edited_only_frames: u64,
}
fn wav_header(file: &mut File, bytes: u32) -> Outcome<()> {
    file.write_all(b"RIFF")?;
    file.write_all(
        &bytes
            .checked_add(36)
            .ok_or("WAV length overflow")?
            .to_le_bytes(),
    )?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?;
    file.write_all(&CHANNELS.to_le_bytes())?;
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * u32::from(CHANNELS) * 4).to_le_bytes())?;
    file.write_all(&(CHANNELS * 4).to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    Ok(())
}
fn render_one(
    store: &NativeStore,
    document: &ProjectDocument,
    path: &Path,
    previous: Option<&AudioReceipt>,
) -> Outcome<(AudioReceipt, Difference)> {
    let prepared = render::prepare(store, document, RATE, CHANNELS)?;
    let frames = prepared.len() / usize::from(CHANNELS);
    let byte_len = prepared.len().checked_mul(4).ok_or("PCM size overflow")?;
    if !prepared.len().is_multiple_of(usize::from(CHANNELS))
        || byte_len > render::PREPARE_BUDGET
        || frames as u64 > render::MAX_SECONDS * u64::from(RATE)
    {
        return Err("Prepared render exceeds channel/byte/time bounds".into());
    }
    let (mut cursor, _meters) = RenderCursor::new(prepared, CHANNELS)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    wav_header(&mut file, u32::try_from(byte_len)?)?;
    let mut reference = match previous {
        Some(previous) => {
            let mut file = File::open(&previous.path)?;
            file.seek(SeekFrom::Start(WAV_HEADER))?;
            Some(file)
        }
        None => None,
    };
    let mut difference = Difference::default();
    let overlap = previous.map_or(0, |p| p.frames.min(frames as u64));
    if let Some(previous) = previous {
        difference.original_only_frames = previous.frames.saturating_sub(frames as u64);
        difference.edited_only_frames = (frames as u64).saturating_sub(previous.frames);
    }
    let mut samples = [0.0f32; CHUNK_SAMPLES];
    let mut bytes = [0u8; CHUNK_SAMPLES * 4];
    let mut old = [0u8; CHUNK_SAMPLES * 4];
    let mut next = 0u64;
    let mut digest = Sha256::new();
    let mut peak = 0.0f32;
    while next < frames as u64 {
        let count =
            (frames as u64 - next).min((CHUNK_SAMPLES / usize::from(CHANNELS)) as u64) as usize;
        let used = count * usize::from(CHANNELS);
        cursor.render_into(&mut samples[..used]);
        for (index, sample) in samples[..used].iter().enumerate() {
            if !sample.is_finite() {
                return Err("Nonfinite production render sample".into());
            }
            peak = peak.max(sample.abs());
            bytes[index * 4..index * 4 + 4].copy_from_slice(&sample.to_le_bytes());
        }
        let compare_frames = overlap.saturating_sub(next).min(count as u64) as usize;
        if let Some(reference) = reference.as_mut() {
            let compare_bytes = compare_frames * usize::from(CHANNELS) * 4;
            reference.read_exact(&mut old[..compare_bytes])?;
            for frame in 0..compare_frames {
                let mut changed = false;
                for channel in 0..usize::from(CHANNELS) {
                    let at = (frame * usize::from(CHANNELS) + channel) * 4;
                    if bytes[at..at + 4] != old[at..at + 4] {
                        difference.overlap_changed_samples += 1;
                        changed = true;
                    }
                }
                if changed {
                    difference.overlap_changed_frames += 1;
                    difference
                        .first_changed_frame
                        .get_or_insert(next + frame as u64);
                }
            }
        }
        digest.update(&bytes[..used * 4]);
        file.write_all(&bytes[..used * 4])?;
        next += count as u64;
    }
    file.sync_all()?;
    let receipt = AudioReceipt {
        path: path.to_path_buf(),
        frames: frames as u64,
        samples: frames as u64 * u64::from(CHANNELS),
        pcm_bytes: byte_len as u64,
        peak,
        pcm_sha256: format!("{:x}", digest.finalize()),
        file_sha256: file_hash(path)?,
        file_bytes: fs::metadata(path)?.len(),
    };
    Ok((receipt, difference))
}
fn main() -> Outcome<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [root_arg, original_arg, edited_arg, output_arg, receipt_arg] = args.as_slice() else {
        return Err("usage: qualify_note_editor_render EXISTING_STORE ORIGINAL_JSON EDITED_JSON FRESH_OUTPUT_DIR NEW_RECEIPT".into());
    };
    let root = directory(root_arg)?;
    let output = new_path(output_arg)?;
    let receipt_path = new_path(receipt_arg)?;
    if output.starts_with(&root)
        || receipt_path.starts_with(&root)
        || receipt_path.starts_with(&output)
    {
        return Err("Output and receipt must be fresh paths outside the resolver store; receipt parent must already exist".into());
    }
    let mut receipt_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)?;
    let original = load(original_arg)?;
    let edited = load(edited_arg)?;
    if original.document.id() != edited.document.id() {
        return Err(
            "Project identities differ; this qualification will not infer a copy mapping".into(),
        );
    }
    let facts = document_facts(&original.document, &edited.document)?;
    let catalog = clean_catalog(&root)?;
    let catalog_before = file_hash(&catalog)?;
    let store = NativeStore::open_existing(&root)?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&output)?;
    // Each cursor/prepared buffer is dropped before preparing the next source.
    // WAV writing/hashing/comparison use fixed stack blocks, not a second PCM Vec.
    let (original_audio, _) = render_one(
        &store,
        &original.document,
        &output.join("original.wav"),
        None,
    )?;
    let (edited_audio, difference) = render_one(
        &store,
        &edited.document,
        &output.join("edited.wav"),
        Some(&original_audio),
    )?;
    drop(store);
    let original_after = file_hash(&original.path)?;
    let edited_after = file_hash(&edited.path)?;
    let catalog_after = file_hash(&catalog)?;
    if original_after != original.raw_sha
        || edited_after != edited.raw_sha
        || catalog_before != catalog_after
    {
        return Err("Source JSON or resolver catalog bytes changed during qualification; retained partial outputs are not accepted evidence".into());
    }
    clean_catalog(&root)?;
    let pcm_different = original_audio.pcm_sha256 != edited_audio.pcm_sha256
        || original_audio.frames != edited_audio.frames;
    let mut receipt = json!({"format":"omniusong.note-editor-render-agreement","version":1,"pipeline":"production render::prepare -> RenderCursor output limiting -> IEEE float32 WAV","sampleRate":RATE,"channels":CHANNELS,"outputDirectory":output,"originalProject":original.evidence,"editedProject":edited.evidence,"documentFacts":facts});
    let audio = json!({"originalAudio":original_audio,"editedAudio":edited_audio,"difference":difference,"pcmDifferent":pcm_different,"sampleEquality":"Exact f32 little-endian byte equality; no tolerance, normalization or loudness rescaling"});
    let invariant = json!({"originalJsonUnchanged":true,"editedJsonUnchanged":true,"resolverCatalogUnchanged":true,"resolverCatalogSha256":catalog_before,"resolverStoreRoot":root,"storeMutationApiCalled":false,"catalogOpen":"existing-only READ_WRITE handle; schema4 and clean rollback-journal preflight; byte-invariance checked","projectCommitVerified":false,"deviceOpened":false,"audioPlayed":false,"captured":false,"inference":false,"humanPerformance":false,"exampleSourceSha256":hash(include_bytes!("qualify_note_editor_render.rs"))});
    let limits = json!({"limits":{"jsonBytesPerInput":MAX_NATIVE_PROJECT_BYTES,"renderPrepareBytes":render::PREPARE_BUDGET,"seconds":render::MAX_SECONDS,"detailedNoteChanges":MAX_DETAILS},"limitations":["Caller-supplied project hashes/revisions must be bound to actual UI/store commits by separate coordinator evidence.","A changed muted track may leave PCM identical. No track is unmuted or altered by this tool; supported production rendering decides what is audible.","Expression playback, physical MIDI/input, model inference, listening quality, measured latency, driver timing and Windows are not qualified.","This is offline deterministic sample generation, not realtime delivery, device playback or a power-loss durability test."]});
    for section in [audio, invariant, limits] {
        let Value::Object(fields) = section else {
            return Err("Receipt section is not an object".into());
        };
        receipt
            .as_object_mut()
            .ok_or("Receipt is not an object")?
            .extend(fields);
    }
    let bytes = serde_json::to_vec_pretty(&receipt)?;
    if bytes.len() > MAX_RECEIPT {
        return Err("Receipt detail size exceeds bounded output".into());
    }
    receipt_file.write_all(&bytes)?;
    receipt_file.write_all(b"\n")?;
    receipt_file.sync_all()?;
    #[cfg(unix)]
    {
        File::open(&output)?.sync_all()?;
        File::open(output.parent().ok_or("Output parent missing")?)?.sync_all()?;
        File::open(receipt_path.parent().ok_or("Receipt parent missing")?)?.sync_all()?;
    }
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}

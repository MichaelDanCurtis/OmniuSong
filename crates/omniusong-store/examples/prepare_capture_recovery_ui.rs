//! Prepare ONE synthetic retained stage for an actual native UI walkthrough.
//!
//! Usage: prepare_capture_recovery_ui LIVE_STORE_ROOT FRESH_RECEIPT_JSON
//! The store and catalog must already exist. The catalog is only stat'ed, never
//! opened; no project is created or changed. The receipt must be outside captures.
//! This leaves Recording with two raw chunks and a deliberately invalid second
//! payload. It never calls discovery/inspection/recovery/seal/attachment APIs.
//! No devices, elapsed recording, human performance, listening, latency or crash
//! durability are qualified. On failure, new partial artifacts are retained.
use omniusong_recording::{
    CHUNK_VERSION, ChunkHeader, ChunkKind, ClockDomainId, ClockEpochId, HostClock, HostTimestamp,
    JOURNAL_VERSION, Journal, PcmEncoding, PcmFormat, PcmLayout, SourceRange, StreamDescriptor,
    StreamKind, TakeDescriptor, TakeId,
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Outcome<T> = std::result::Result<T, Box<dyn Error>>;
const FRAMES: u64 = 12_000;
const RATE: u32 = 48_000;

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_sha(path: &Path) -> Outcome<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn ordinary_file(path: &Path) -> Outcome<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} must be an existing ordinary non-symlink file",
            path.display()
        )
        .into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("Hard-linked files are not accepted for this fixture".into());
        }
    }
    Ok(metadata)
}
fn ordinary_directory(path: &Path) -> Outcome<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} must be an existing ordinary non-symlink directory",
            path.display()
        )
        .into());
    }
    Ok(fs::canonicalize(path)?)
}
fn private_directory(path: &Path) -> Outcome<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
fn raw_fingerprints(connection: &Connection) -> Outcome<Value> {
    let mut statement = connection.prepare(
        "SELECT ordinal,stream_id,sequence,header,payload,sha256 FROM chunks ORDER BY ordinal",
    )?;
    let mut cursor = statement.query([])?;
    let mut rows = Vec::new();
    let mut aggregate = Sha256::new();
    aggregate.update(b"OmniuSong.capture-ui-fixture-raw-rows.v1\0");
    while let Some(row) = cursor.next()? {
        if rows.len() == 2 {
            return Err("Unexpected extra raw chunk in the new fixture".into());
        }
        let ordinal: i64 = row.get(0)?;
        let stream: String = row.get(1)?;
        let sequence: i64 = row.get(2)?;
        let header: Vec<u8> = row.get(3)?;
        let payload: Vec<u8> = row.get(4)?;
        let stored: String = row.get(5)?;
        if ordinal != rows.len() as i64 + 1
            || sequence != rows.len() as i64
            || stream != "audio"
            || header.len() > 64 * 1024
            || payload.len() != FRAMES as usize * 4
        {
            return Err("Unexpected raw row shape in the new fixture".into());
        }
        let decoded: ChunkHeader = serde_json::from_slice(&header)?;
        let computed = decoded.sha256(&payload)?;
        if (sequence == 0 && computed != stored) || (sequence == 1 && computed == stored) {
            return Err("Fixture did not retain exactly one valid and one invalid payload".into());
        }
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.capture-ui-fixture-raw-row.v1\0");
        hash.update(ordinal.to_le_bytes());
        field(&mut hash, stream.as_bytes());
        hash.update(sequence.to_le_bytes());
        field(&mut hash, &header);
        field(&mut hash, &payload);
        field(&mut hash, stored.as_bytes());
        let row_sha = format!("{:x}", hash.finalize());
        field(&mut aggregate, row_sha.as_bytes());
        rows.push(json!({"ordinal":ordinal,"streamId":stream,"sequence":sequence,"headerByteLength":header.len(),"headerBytesSha256":sha(&header),"payloadByteLength":payload.len(),"payloadBytesSha256":sha(&payload),"storedChunkSha256":stored,"computedChunkSha256":computed,"rawRowSha256":row_sha}));
    }
    if rows.len() != 2 {
        return Err("New fixture does not contain exactly two raw chunks".into());
    }
    aggregate.update((rows.len() as u64).to_le_bytes());
    let (state, descriptor, descriptor_stored): (String, Vec<u8>, String) = connection.query_row(
        "SELECT state,descriptor,descriptor_sha256 FROM take_meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let (interruptions, manifests): (i64, i64) = connection.query_row(
        "SELECT (SELECT count(*) FROM interruptions),(SELECT count(*) FROM sealed_manifests)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if state != "recording" || interruptions != 0 || manifests != 0 {
        return Err("New fixture was unexpectedly interrupted or finalized".into());
    }
    let prefix:(String,i64,i64,i64,i64,i64)=connection.query_row("SELECT stream_id,next_sequence,source_end,recorded_units,gap_units,gap_chunks FROM stream_prefix",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    Ok(
        json!({"algorithm":"SHA256; domain + little-endian ordinal/sequence + length-prefixed complete raw stream/header/payload/stored-hash bytes","rawRowsSha256":format!("{:x}",aggregate.finalize()),"chunks":rows,"takeMeta":{"state":state,"descriptorBytesSha256":sha(&descriptor),"storedDescriptorSha256":descriptor_stored},"streamPrefix":{"streamId":prefix.0,"nextSequence":prefix.1,"sourceEnd":prefix.2,"recordedUnits":prefix.3,"gapUnits":prefix.4,"gapChunks":prefix.5},"interruptions":interruptions,"sealedManifests":manifests}),
    )
}
fn main() -> Outcome<()> {
    let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [root_argument, receipt_argument] = arguments.as_slice() else {
        return Err("usage: prepare_capture_recovery_ui LIVE_STORE_ROOT FRESH_RECEIPT_JSON".into());
    };
    let root = ordinary_directory(root_argument)?;
    let catalog = root.join("studio.sqlite3");
    let catalog_before = ordinary_file(&catalog)?;
    if catalog_before.len() == 0 {
        return Err("Existing catalog is empty; no replacement will be created".into());
    }
    let captures = root.join("captures");
    match fs::symlink_metadata(&captures) {
        Ok(_) => {
            if ordinary_directory(&captures)? != captures {
                return Err("Capture directory redirects outside the live root".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let receipt_name = receipt_argument
        .file_name()
        .ok_or("Receipt needs a filename")?;
    let receipt_parent = ordinary_directory(
        receipt_argument
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let receipt_path = receipt_parent.join(receipt_name);
    if receipt_path.starts_with(&captures) {
        return Err("Receipt must be outside captures so existing stages cannot be changed".into());
    }
    if root.to_str().is_none() || receipt_path.to_str().is_none() {
        return Err("Fixture receipt requires UTF-8 filesystem paths".into());
    }
    // Reserve this new receipt first; collisions/dangling symlinks cause failure
    // before creating a stage. Never replace somebody else's receipt.
    let mut receipt_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)?;
    // Recheck the live root/catalog without opening the catalog. No mkdir -p.
    if ordinary_directory(&root)? != root {
        return Err("Live root changed during fixture preflight".into());
    }
    ordinary_file(&catalog)?;
    if !captures.exists() {
        private_directory(&captures)?;
    }
    if ordinary_directory(&captures)? != captures {
        return Err("Capture directory changed during fixture preflight".into());
    }
    let stage_id = uuid::Uuid::new_v4().to_string();
    let stage_directory = captures.join(&stage_id);
    private_directory(&stage_directory)?; // Exclusive; a UUID collision is not reused.
    let journal_path = stage_directory.join("take.sqlite3");
    let take_id = uuid::Uuid::new_v4().to_string();
    eprintln!(
        "Preparing synthetic-only stage {stage_id}: {}",
        journal_path.display()
    );
    let epoch = ClockEpochId(uuid::Uuid::new_v4().to_string());
    let descriptor = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId(take_id.clone()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "software-segment".into(),
            source_id: "synthetic-recovery-ui-fixture".into(),
            source_name: "Synthetic 440 Hz retained stage; no device or human".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("synthetic-frame-clock".into()),
                epoch: epoch.clone(),
                ticks_per_second_numerator: RATE as u64,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: RATE,
                    channels: 1,
                    encoding: PcmEncoding::F32Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: epoch,
            },
            initial_source_position: 0,
        }],
        provenance: BTreeMap::from([(
            "omniusong:uiRecoveryFixture".into(),
            json!({"synthetic":true,"human":false,"deviceOpened":false,"nativeInputOpened":false,"elapsedRecording":false,"frequencyHz":440,"amplitude":0.1,"purpose":"Actual UI inspect/recover/attach walkthrough only"}),
        )]),
    };
    let pcm: Vec<u8> = (0..FRAMES)
        .flat_map(|frame| {
            let t = frame as f32 / RATE as f32;
            let envelope =
                (frame as f32 / 480.0).min(1.0) * ((FRAMES - frame) as f32 / 480.0).min(1.0);
            (0.1 * envelope * (std::f32::consts::TAU * 440.0 * t).sin()).to_le_bytes()
        })
        .collect();
    // Journal::create creates parent directories. Anchor its relative parent at
    // the already-exclusive stage CWD, preventing it from recreating a lost live
    // store/mount hierarchy. This is an isolated, single-purpose CLI process.
    std::env::set_current_dir(&stage_directory)?;
    let mut journal = Journal::create("take.sqlite3", &descriptor)?;
    if journal.path() != journal_path.as_path() {
        return Err("Created journal escaped its exclusive stage".into());
    }
    for sequence in 0..2 {
        let start = sequence * FRAMES;
        let stamp = |ticks| HostTimestamp {
            domain: descriptor.streams[0].host_clock.domain.clone(),
            epoch: descriptor.streams[0].host_clock.epoch.clone(),
            ticks,
        };
        journal.append(
            &ChunkHeader {
                version: CHUNK_VERSION,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "software-segment".into(),
                sequence,
                range: SourceRange::AudioFrames {
                    start,
                    count: FRAMES,
                },
                interpretation: descriptor.streams[0].kind.clone(),
                observed_start: stamp(start),
                observed_end: stamp(start + FRAMES),
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &pcm,
        )?;
    }
    drop(journal);
    let mut connection = Connection::open_with_flags(
        &journal_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("DROP TRIGGER immutable_chunk_update;")?;
    let changed=tx.execute("UPDATE chunks SET payload=zeroblob(length(payload)) WHERE stream_id='audio' AND sequence=1",[])?;
    if changed != 1 {
        return Err("Synthetic tail corruption did not affect exactly one new row".into());
    }
    tx.commit()?;
    let fingerprints = raw_fingerprints(&connection)?;
    drop(connection);
    ordinary_file(&journal_path)?;
    let original_file_sha256 = file_sha(&journal_path)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&journal_path)?
        .sync_all()?;
    #[cfg(unix)]
    {
        File::open(&stage_directory)?.sync_all()?;
        File::open(&captures)?.sync_all()?;
    }
    let catalog_after = ordinary_file(&catalog)?;
    let expected_inspection = json!({
        "state": "recording", "streamId": "audio", "startFrame": 0,
        "frames": FRAMES, "endFrame": FRAMES, "recordedFrames": FRAMES,
        "gapFrames": 0, "verifiedChunks": 1, "retainedRawChunks": 2,
        "unknownTail": true, "hasProblems": true, "manifestSha256": null,
        "unverifiedTail": {"startFrame": FRAMES, "frames": FRAMES}
    });
    let mut receipt = json!({
        "format": "omniusong.capture-recovery-ui-fixture", "version": 1,
        "syntheticOnly": true, "human": false, "deviceOpened": false,
        "nativeInputOpened": false, "physicalMicrophoneOpened": false,
        "liveStoreRoot": root, "catalogPath": catalog, "catalogOpened": false,
        "catalogWriteRequested": false, "catalogContentsVerified": false,
        "catalogByteLengthBefore": catalog_before.len(),
        "catalogByteLengthAfter": catalog_after.len()
    });
    let stage_receipt = json!({
        "stageId": stage_id, "takeId": take_id, "journalPath": journal_path,
        "state": "recording", "originalFileSha256": original_file_sha256,
        "originalByteLength": fs::metadata(&journal_path)?.len(),
        "descriptorSha256": descriptor.sha256()?, "pcmSha256": sha(&pcm),
        "pcmByteLength": pcm.len(), "sampleRate": RATE, "channels": 1,
        "encoding": "f32_le", "layout": "interleaved", "frequencyHz": 440,
        "rawFingerprints": fingerprints, "expectedInspection": expected_inspection
    });
    let action_receipt = json!({
        "scanPerformed": false, "inspectionPerformed": false,
        "recoveryPerformed": false, "sealed": false, "attached": false,
        "limitations": [
            "Synthetic procedural PCM only; no human performance, device, elapsed recording, listening, latency or crash-durability acceptance.",
            "Catalog content was not opened or validated; compare current sessions/catalog independently before and after this CLI.",
            "The raw cache still describes both originally committed chunks; only the first chunk is expected to verify after deliberate tail corruption.",
            "No stage is included in a project archive until separately recovered and explicitly attached; failed runs retain their new partial stage/receipt."
        ]
    });
    // Keep each macro invocation bounded without changing the flat receipt schema.
    let receipt_object = receipt.as_object_mut().ok_or("Receipt must be an object")?;
    for section in [stage_receipt, action_receipt] {
        let Value::Object(fields) = section else {
            return Err("Receipt section must be an object".into());
        };
        receipt_object.extend(fields);
    }
    let bytes = serde_json::to_vec_pretty(&receipt)?;
    receipt_file.write_all(&bytes)?;
    receipt_file.write_all(b"\n")?;
    receipt_file.sync_all()?;
    #[cfg(unix)]
    File::open(&receipt_parent)?.sync_all()?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}

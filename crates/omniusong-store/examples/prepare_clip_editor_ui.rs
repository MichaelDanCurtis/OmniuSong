//! Append ONE clearly synthetic clip-editor fixture to an existing current-schema store.
//!
//! Usage: prepare_clip_editor_ui EXISTING_STORE NEW_RECEIPT
//! Reserve the receipt with create_new before store writes; retain all artifacts
//! in a fresh sibling directory outside the store. Never edit old projects,
//! delete failed work, migrate storage, open devices, render or operate the UI.
//! Failures can retain a newly imported project/owned take. Inspect phase, import
//! source SHA and known destination ID before any manual retry. The importer
//! allocates its destination ID internally, so uncertain import admission is
//! identified by exact source JSON SHA until the returned ID is recorded.
use omniusong_core::{
    ProjectDocument,
    contracts::{
        CaptureTime, CapturedEvent, ClockDomainId, ClockEpochId, EndpointId, Envelope, EventId,
        HostTimestamp, MessageId, MusicalEvent, OperationId, ProjectId, RawPacket, SessionId,
        SourceAddress, TakeId, TrackId,
    },
    native::{AudioFades, FadeCurve, TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::{
    ChunkHeader, ChunkKind, HostClock, Journal, PcmEncoding, PcmFormat, PcmLayout, RawEventFormat,
    SealedJournal, SourceRange, StreamDescriptor, StreamKind, TakeDescriptor, encode_event_chunk,
};
use omniusong_store::{NativeStore, TakePublication};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

type Outcome<T> = Result<T, Box<dyn Error>>;
const MAX_OBJECT: usize = 1024 * 1024;
const MAX_JSON: usize = 256 * 1024;
const FRAMES: u64 = 24_000;
const RATE: u32 = 48_000;
const LEGACY_TRACK: &str = "synthetic:clip:音源\0\n";
const AUDIO_TRACK: &str = "synthetic-native-audio";
const EVENT_TRACK: &str = "synthetic-raw-events";

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn directory(path: &Path) -> Outcome<PathBuf> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Expected an existing ordinary non-symlink directory".into());
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
            return Err("Hard-linked catalogs are not accepted".into());
        }
    }
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_secs(2))?;
    let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != omniusong_store::STORE_SCHEMA_VERSION {
        return Err(
            "This fixture requires the current schema and will not migrate the native catalog"
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
    if bytes.len() > MAX_OBJECT {
        return Err("Synthetic artifact exceeds the 1MiB object bound".into());
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn file_sha(path: &Path) -> Outcome<String> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() > MAX_OBJECT as u64 {
        return Err("Source/retained artifact exceeds the 1MiB object bound".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count).ok_or("File length overflow")?;
        if total > MAX_OBJECT {
            return Err("Artifact grew beyond its bounded size".into());
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn receipt_write(file: &mut File, value: &Value) -> Outcome<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() > MAX_JSON {
        return Err("Receipt exceeds bounded JSON size".into());
    }
    // Only this invocation's exclusively reserved receipt inode is updated.
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
fn checkpoint(file: &mut File, receipt: &mut Value, phase: &str) -> Outcome<()> {
    receipt["phase"] = phase.into();
    receipt_write(file, receipt)
}
fn extend(receipt: &mut Value, section: Value) -> Outcome<()> {
    receipt
        .as_object_mut()
        .ok_or("Receipt object missing")?
        .extend(
            section
                .as_object()
                .ok_or("Receipt section missing")?
                .clone(),
        );
    Ok(())
}
fn pcm_f32() -> Vec<u8> {
    (0..FRAMES)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(RATE);
            let ramp = (frame + 1) as f64 / FRAMES as f64;
            let tail = ((FRAMES - frame) as f64 / 256.0).min(1.0);
            ((0.04 * ramp * tail * (std::f64::consts::TAU * 440.0 * time).sin()) as f32)
                .to_le_bytes()
        })
        .collect()
}
fn wav(pcm: &[u8]) -> Outcome<Vec<u8>> {
    let length = u32::try_from(pcm.len())?;
    let mut bytes = Vec::with_capacity(44 + pcm.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    for value in [3u16, 1u16] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(pcm);
    Ok(bytes)
}
fn source_project(
    asset_id: &str,
    byte_length: usize,
    fixture_id: &str,
) -> Outcome<ProjectDocument> {
    let mut value = ProjectDocument::new(&format!(
        "SYNTHETIC clip editor walkthrough {}",
        &fixture_id[..8]
    ))?
    .value()
    .clone();
    value["tempo"] = json!(119.25);
    value["bars"] = 4.into();
    let exact: Value = serde_json::from_str(
        r#"{"trimStartSeconds":0.125000000000000000000000000001,"trimEndSeconds":0.375000000000000000000000000001,"fadeInSeconds":0.0125000,"fadeOutSeconds":0.0250000}"#,
    )?;
    let mut audio = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":0.5,"byteLength":byte_length,"startTick":123});
    audio
        .as_object_mut()
        .ok_or("Audio object missing")?
        .extend(exact.as_object().ok_or("Exact settings missing")?.clone());
    audio["provenance"] = json!({"synthetic":true,"human":false,"deviceOpened":false,"inference":false,"unmeasuredTiming":{"domain":"synthetic-frame-clock","ticksPerSecond":48_000,"calibrated":false,"nativeHostClockObserved":false}});
    audio["vendor:opaque"] = serde_json::from_str(
        r#"{"hugeInteger":1844674407370955161600000001,"longDecimal":0.12345678901234567890123456789012345,"uuidLooking":"00000000-0000-4000-8000-000000000001"}"#,
    )?;
    value["tracks"] = json!([{"id":LEGACY_TRACK,"name":"SYNTHETIC legacy audio · exact seconds","kind":"audio","instrument":"keys","gain":0.25,"muted":false,"locked":false,"armed":false,"notes":[],"assetId":asset_id,"sourceAssetId":asset_id,"audio":audio}]);
    value["syntheticClipEditorFixture"] = json!({"version":1,"fixtureId":fixture_id,"synthetic":true,"human":false,"deviceOpened":false,"nativeInputOpened":false,"inference":false,"uiOperated":false,"timingCalibrated":false,"rawPacketsAreFixtureOnly":true});
    let document = ProjectDocument::from_value(value)?;
    if document.to_json()?.len() > MAX_JSON {
        return Err("Fixture source metadata exceeds bounded size".into());
    }
    Ok(document)
}
fn descriptor(fixture_id: &str) -> TakeDescriptor {
    let clock = HostClock {
        domain: ClockDomainId("synthetic-clip-clock".into()),
        epoch: ClockEpochId("synthetic-clip-epoch".into()),
        ticks_per_second_numerator: u64::from(RATE),
        ticks_per_second_denominator: 1,
    };
    let stream = |id: &str, initial, kind| StreamDescriptor {
        stream_id: id.into(),
        segment_id: format!("{id}-synthetic-segment"),
        source_id: format!("synthetic-{id}"),
        source_name: format!("SYNTHETIC {id} · no device opened"),
        source_connection_generation: 1,
        host_clock: clock.clone(),
        kind,
        initial_source_position: initial,
    };
    TakeDescriptor {
        version: 1,
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        streams: vec![
            stream(
                "audio",
                100,
                StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: RATE,
                        channels: 1,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("synthetic-audio-epoch".into()),
                },
            ),
            stream(
                "events",
                0,
                StreamKind::RawEvents {
                    format: RawEventFormat::LengthPrefixedCaptureV1,
                },
            ),
        ],
        provenance: BTreeMap::from([(
            "omniusong:synthetic-fixture".into(),
            json!({"fixtureId":fixture_id,"synthetic":true,"human":false,"deviceOpened":false,"inference":false,"clock":"Authored frame ticks, not native host observations or a calibration"}),
        )]),
    }
}
fn header(
    take: &TakeDescriptor,
    index: usize,
    start: u64,
    count: u64,
    observed_start: u64,
    observed_end: u64,
) -> ChunkHeader {
    let stream = &take.streams[index];
    let stamp = |ticks| HostTimestamp {
        domain: stream.host_clock.domain.clone(),
        epoch: stream.host_clock.epoch.clone(),
        ticks,
    };
    ChunkHeader {
        version: 1,
        take_id: take.take_id.clone(),
        stream_id: stream.stream_id.clone(),
        segment_id: stream.segment_id.clone(),
        sequence: 0,
        range: if index == 0 {
            SourceRange::AudioFrames { start, count }
        } else {
            SourceRange::Events { start, count }
        },
        interpretation: stream.kind.clone(),
        observed_start: stamp(observed_start),
        observed_end: stamp(observed_end),
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}
fn events(take: &TakeDescriptor) -> Outcome<Vec<Vec<u8>>> {
    let stream = &take.streams[1];
    let words = [
        (0x40903c00u32, 51313u32 << 16 | 0xabcd),
        (0x40803c00, 37155u32 << 16 | 0x1234),
        (0x40904300, 65535u32 << 16),
        (0x40804300, 19001u32 << 16),
    ];
    words
        .into_iter()
        .enumerate()
        .map(|(index, (first, second))| {
            Ok(Envelope::new(
                SessionId("synthetic-clip-fixture".into()),
                MessageId(format!("fixture-packet-{index}")),
                index as u64,
                CapturedEvent {
                    event_id: EventId(format!("fixture-event-{index}")),
                    source: SourceAddress {
                        endpoint: EndpointId(stream.source_id.clone()),
                        connection_generation: 1,
                        group: 0,
                        channel: Some(0),
                        extensions: Default::default(),
                    },
                    time: CaptureTime {
                        observed: HostTimestamp {
                            domain: stream.host_clock.domain.clone(),
                            epoch: stream.host_clock.epoch.clone(),
                            ticks: index as u64 * 6000,
                        },
                        alignment: None,
                        extensions: Default::default(),
                    },
                    raw: RawPacket::Ump {
                        words: vec![first, second],
                    },
                    musical: MusicalEvent::Uninterpreted,
                    extensions: BTreeMap::from([(
                        "omniusong:synthetic".into(),
                        json!({"human":false,"deviceOpened":false,"fixtureOnly":true}),
                    )]),
                },
            )
            .encode()?)
        })
        .collect()
}
fn main() -> Outcome<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [root_arg, receipt_arg] = args.as_slice() else {
        return Err("usage: prepare_clip_editor_ui EXISTING_STORE NEW_RECEIPT".into());
    };
    let root = directory(root_arg)?;
    let catalog_path = root.join("studio.sqlite3");
    let receipt_name = receipt_arg
        .file_name()
        .ok_or("Receipt requires a filename")?;
    let parent = directory(
        receipt_arg
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let receipt_path = parent.join(receipt_name);
    if receipt_path.starts_with(&root) || root.to_str().is_none() || receipt_path.to_str().is_none()
    {
        return Err(
            "Receipt/artifacts must use UTF-8 paths outside the existing native root".into(),
        );
    }
    let mut receipt_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)?;
    let fixture_id = uuid::Uuid::new_v4().to_string();
    let artifacts = parent.join(format!("clip-editor-ui-{fixture_id}"));
    let assets = artifacts.join("assets");
    let journal_path = artifacts.join("synthetic-original.sqlite3");
    let baseline_path = artifacts.join("baseline-project.json");
    let archive_path = artifacts.join("baseline.omniusong-native");
    let import_path = artifacts.join("exact-legacy-import.json");
    let pcm = pcm_f32();
    let wav = wav(&pcm)?;
    let wav_sha = sha(&wav);
    let asset_id = format!("{wav_sha}.wav");
    let wav_path = assets.join(&asset_id);
    let source = source_project(&asset_id, wav.len(), &fixture_id)?;
    let source_bytes = source.to_json()?;
    let mut receipt = json!({"format":"omniusong.clip-editor-ui-fixture","version":1,"phase":"prepared_not_imported","fixtureId":fixture_id,"sourceProjectId":source.id(),"sourceImportSha256":sha(&source_bytes),"existingStore":root,"catalogPath":catalog_path,"artifactDirectory":artifacts,"receiptPath":receipt_path});
    extend(
        &mut receipt,
        json!({"synthetic":true,"human":false,"deviceOpened":false,"nativeInputOpened":false,"inference":false,"uiOperated":false,"audioPlayed":false,"rawPacketsAreFixtureOnly":true,"existingDocumentsEdited":false,"existingOnlySchema4":true,"exampleSourceSha256":sha(include_bytes!("prepare_clip_editor_ui.rs"))}),
    )?;
    extend(
        &mut receipt,
        json!({"limitations":["All PCM, UMP and timestamps are synthetic; no input/human/controller, inference, physical output/listening, latency, power-loss, UI or Windows acceptance is asserted.","Native descriptor/prefix audio bounds100..24100 intentionally exceed the selected first view6100..18100. Current saved windows are not full source bounds.","Raw events are retained on a separate muted recording track for read-only inspection; ordinals are not audio duration.","Public direct save authors one fades:null representation and the raw-event mute flag as fixture metadata; it is not a claimed clip-editor action.","Import/publication/archive/receipt failures do not roll back or delete the fresh project or owned original. Inspect phase and known IDs/source SHA before manual retry."]}),
    )?;
    checkpoint(&mut receipt_file, &mut receipt, "prepared_not_imported")?;
    catalog(&catalog_path)?;
    fresh_directory(&artifacts)?;
    fresh_directory(&assets)?;
    write_new(&wav_path, &wav)?;
    write_new(&import_path, &source_bytes)?;
    let take = descriptor(&fixture_id);
    let packets = events(&take)?;
    let refs: Vec<_> = packets.iter().map(Vec::as_slice).collect();
    let event_payload = encode_event_chunk(&refs)?;
    write_new(&artifacts.join("synthetic-pcm.f32le"), &pcm)?;
    write_new(
        &artifacts.join("take-descriptor.json"),
        &serde_json::to_vec_pretty(&take)?,
    )?;
    for (index, packet) in packets.iter().enumerate() {
        write_new(
            &artifacts.join(format!("raw-ump-event-{index}.json")),
            packet,
        )?;
    }
    let mut writer = Journal::create(&journal_path, &take)?;
    writer.append(&header(&take, 0, 100, FRAMES, 0, FRAMES), &pcm)?;
    writer.append(&header(&take, 1, 0, 4, 0, FRAMES), &event_payload)?;
    let manifest = writer.seal(&format!("fixture-seal-{fixture_id}"))?;
    drop(writer);
    let source_journal_sha = file_sha(&journal_path)?;
    let sealed = SealedJournal::open(&journal_path)?;
    let mut chunk_fingerprints = Vec::new();
    sealed.visit_chunks(|ordinal,chunk|{chunk_fingerprints.push(json!({"ordinal":ordinal,"streamId":chunk.header.stream_id,"sequence":chunk.header.sequence,"sourceRange":chunk.header.range,"chunkSha256":chunk.sha256,"payloadSha256":sha(&chunk.payload),"payloadBytes":chunk.payload.len()}));Ok(())})?;
    extend(
        &mut receipt,
        json!({"legacyImport":{"path":import_path,"sha256":sha(&source_bytes),"byteLength":source_bytes.len()},"wavOriginal":{"assetId":asset_id,"path":wav_path,"sha256":wav_sha,"byteLength":wav.len(),"pcmSha256":sha(&pcm),"frames":FRAMES,"sampleRate":RATE,"channels":1,"encoding":"f32le"},"sealedOriginal":{"path":journal_path,"fileSha256":source_journal_sha,"byteLength":fs::metadata(&journal_path)?.len(),"takeId":take.take_id.0,"manifestSha256":manifest.sha256,"audioStartFrame":100,"audioEndFrame":24100,"audioRecordedFrames":FRAMES,"rawEventStart":0,"rawEventCount":4,"gapUnits":0,"unknownTail":false,"chunks":chunk_fingerprints}}),
    )?;
    if directory(&root)? != root {
        return Err("Native root changed during artifact preparation".into());
    }
    catalog(&catalog_path)?;
    checkpoint(
        &mut receipt_file,
        &mut receipt,
        "import_pending_destination_id_allocated_inside_importer_inspect_exact_source_sha_if_interrupted",
    )?;
    eprintln!(
        "Synthetic import source {} · SHA {} · any later failures retain the fresh project/artifacts",
        source.id(),
        sha(&source_bytes)
    );
    let mut store = NativeStore::open_existing(&root)?;
    let mut current = match store.import_legacy(&import_path, &assets) {
        Ok(project) => project,
        Err(error) => {
            receipt["error"] = error.to_string().into();
            let _ = checkpoint(
                &mut receipt_file,
                &mut receipt,
                "import_failed_or_uncertain_inspect_exact_source_sha",
            );
            return Err(error.into());
        }
    };
    receipt["projectId"] = current.id().into();
    receipt["projectName"] = current.name().into();
    receipt["revision"] = current.revision().into();
    checkpoint(
        &mut receipt_file,
        &mut receipt,
        "project_imported_schema_upgrade_pending",
    )?;
    eprintln!(
        "CREATED synthetic project {}. It remains if a later operation fails.",
        current.id()
    );
    if store.imported_original(current.id())?.as_deref() != Some(source_bytes.as_slice())
        || store.asset(&asset_id)? != wav
    {
        return Err(
            "Imported exact original evidence differs; retain project/artifacts for inspection"
                .into(),
        );
    }
    current = store.save(&current.upgrade_native()?, current.revision())?;
    receipt["revision"] = current.revision().into();
    checkpoint(
        &mut receipt_file,
        &mut receipt,
        "schema2_saved_take_ingestion_pending",
    )?;
    let object = store.ingest_take(&sealed)?;
    let owned_path = root
        .join("takes")
        .join(format!("{}.sqlite3", object.object_sha256));
    if file_sha(&owned_path)? != object.object_sha256 || object.byte_length > MAX_OBJECT as u64 {
        return Err("Owned synthetic original failed its bounded fingerprint".into());
    }
    receipt["ownedOriginal"] = json!({"path":owned_path,"sha256":object.object_sha256,"byteLength":object.byte_length,"takeId":object.take_id.0,"manifestSha256":object.manifest_sha256});
    let definitions = [
        (
            AUDIO_TRACK,
            "SYNTHETIC native audio · two clips",
            "native-audio-first",
            "audio",
            6100,
            12000,
            0,
        ),
        (
            AUDIO_TRACK,
            "SYNTHETIC native audio · two clips",
            "native-audio-second",
            "audio",
            100,
            12000,
            12000,
        ),
        (
            EVENT_TRACK,
            "SYNTHETIC raw UMP · muted/read-only",
            "native-raw-events",
            "events",
            0,
            4,
            0,
        ),
    ];
    let mut publications = Vec::new();
    for (track_id, track_name, clip_id, stream, start, count, position) in definitions {
        let clip = TakeClip {
            id: clip_id.into(),
            source: TakeReference {
                take_id: take.take_id.clone(),
                manifest_sha256: manifest.sha256.clone(),
                stream_id: stream.into(),
            },
            range: if stream == "audio" {
                TakeRange::AudioFrames { start, count }
            } else {
                TakeRange::Events { start, count }
            },
            position: TimelinePosition {
                frame: position,
                sample_rate: RATE,
            },
            fades: if clip_id == "native-audio-second" {
                Some(AudioFades {
                    in_frames: 100,
                    out_frames: 200,
                    curve: FadeCurve::EqualPower,
                })
            } else {
                None
            },
            edit_source: None,
        };
        let publication = TakePublication {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            project_id: ProjectId(current.id().into()),
            expected_revision: current.revision(),
            track_id: TrackId(track_id.into()),
            track_name: track_name.into(),
            clip,
        };
        receipt["pendingPublication"] = serde_json::to_value(&publication)?;
        receipt["revision"] = current.revision().into();
        checkpoint(
            &mut receipt_file,
            &mut receipt,
            "canonical_take_publication_pending_inspect_operation_id_if_interrupted",
        )?;
        current = store.publish_take(&publication)?.document;
        publications.push(serde_json::to_value(publication)?);
        receipt["publications"] = Value::Array(publications.clone());
        receipt["revision"] = current.revision().into();
    }
    receipt
        .as_object_mut()
        .ok_or("Receipt object missing")?
        .remove("pendingPublication");
    checkpoint(
        &mut receipt_file,
        &mut receipt,
        "published_fixture_null_fade_and_event_mute_save_pending",
    )?;
    let mut value = current.value().clone();
    let tracks = value["tracks"]
        .as_array_mut()
        .ok_or("Fixture tracks missing")?;
    for track in tracks {
        if track["id"] == AUDIO_TRACK {
            let raw = track["recording"]["clips"]
                .as_array_mut()
                .ok_or("Audio clips missing")?
                .iter_mut()
                .find(|clip| clip["id"] == "native-audio-first")
                .ok_or("First native clip missing")?;
            raw["fades"] = Value::Null;
        }
        if track["id"] == EVENT_TRACK {
            track["muted"] = true.into();
        }
    }
    value["syntheticClipEditorFixture"]["fadesNullAndRawEventMuteAuthoredByFixtureSave"] =
        true.into();
    let candidate = ProjectDocument::from_value(value)?;
    current = store.save(&candidate, current.revision())?;
    receipt["revision"] = current.revision().into();
    checkpoint(
        &mut receipt_file,
        &mut receipt,
        "fixture_saved_baseline_archive_pending",
    )?;
    let baseline = current.to_json()?;
    if baseline.len() > MAX_JSON {
        return Err("Baseline fixture metadata exceeds bound".into());
    }
    write_new(&baseline_path, &baseline)?;
    let archive = store.export_archive(current.id(), &archive_path)?;
    if archive.asset_count != 1
        || archive.take_count != 1
        || archive.operation_rows != 3
        || archive.history_rows != 5
    {
        return Err(
            "Unexpected baseline closure; fresh project/artifacts retained for review".into(),
        );
    }
    if file_sha(&journal_path)? != source_journal_sha
        || file_sha(&wav_path)? != wav_sha
        || file_sha(&import_path)? != sha(&source_bytes)
        || file_sha(&owned_path)? != object.object_sha256
        || store.imported_original(current.id())?.as_deref() != Some(source_bytes.as_slice())
        || store.load(current.id())?.to_json()? != baseline
    {
        return Err(
            "Final source/baseline preservation comparison failed; retain all evidence".into(),
        );
    }
    extend(
        &mut receipt,
        json!({"baseline":{"projectId":current.id(),"name":current.name(),"revision":current.revision(),"schemaVersion":current.schema_version(),"path":baseline_path,"sha256":sha(&baseline),"byteLength":baseline.len()},"addresses":{"legacy":{"project_id":current.id(),"address":{"representation":"legacy_audio","track_id":LEGACY_TRACK}},"nativeFirst":{"project_id":current.id(),"address":{"representation":"native_audio","track_id":AUDIO_TRACK,"clip_id":"native-audio-first"}},"nativeSecond":{"project_id":current.id(),"address":{"representation":"native_audio","track_id":AUDIO_TRACK,"clip_id":"native-audio-second"}},"rawEventsReadOnly":{"trackId":EVENT_TRACK,"clipId":"native-raw-events","muted":true}},"baselineSettings":current.value()["tracks"],"sourceArchive":{"path":archive.path,"sha256":archive.sha256,"byteLength":archive.byte_length,"historyRows":archive.history_rows,"operationRows":archive.operation_rows,"assetCount":archive.asset_count,"takeCount":archive.take_count}}),
    )?;
    if file_sha(&archive_path)? != archive.sha256 {
        return Err("Baseline archive SHA differs; retain fresh project/evidence".into());
    }
    checkpoint(&mut receipt_file, &mut receipt, "complete")?;
    #[cfg(unix)]
    {
        File::open(&artifacts)?.sync_all()?;
        File::open(&parent)?.sync_all()?;
    }
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}

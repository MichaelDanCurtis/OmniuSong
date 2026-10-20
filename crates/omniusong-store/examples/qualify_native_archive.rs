//! Retained synthetic qualification of native take publication and archive closure.
//! No device input, personal recordings, inference or realtime performance claims.
use omniusong_core::{
    ProjectDocument,
    contracts::{
        CaptureTime, CapturedEvent, EditOperation, EditScope, EndpointId, Envelope, EventId,
        MessageId, MusicalEvent, OperationId, ProjectEditIntent, ProjectId, RawPacket, SessionId,
        SourceAddress, TrackId,
    },
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_recording::*;
use omniusong_store::{NativeStore, TakePublication};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, error::Error, fs, io::Read, path::Path};

type Outcome<T> = std::result::Result<T, Box<dyn Error>>;

fn file_hash(path: &Path) -> Outcome<String> {
    let mut input = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65_536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn descriptor() -> TakeDescriptor {
    let host_clock = HostClock {
        domain: ClockDomainId("synthetic-host".into()),
        epoch: ClockEpochId("synthetic-boot".into()),
        ticks_per_second_numerator: 48_000,
        ticks_per_second_denominator: 1,
    };
    let stream = |id: &str, kind| StreamDescriptor {
        stream_id: id.into(),
        segment_id: format!("{id}-segment"),
        source_id: format!("{id}-source"),
        source_name: format!("Synthetic {id}"),
        source_connection_generation: 1,
        host_clock: host_clock.clone(),
        kind,
        initial_source_position: 0,
    };
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("qualification-original".into()),
        streams: vec![
            stream(
                "audio",
                StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        encoding: PcmEncoding::F32Le,
                        layout: PcmLayout::Interleaved,
                    },
                    audio_epoch: ClockEpochId("synthetic-audio".into()),
                },
            ),
            stream(
                "events",
                StreamKind::RawEvents {
                    format: RawEventFormat::LengthPrefixedCaptureV1,
                },
            ),
        ],
        provenance: BTreeMap::from([(
            "qualification".into(),
            json!({
                "synthetic": true, "capturedFromDevice": false,
                "takeId": "opaque-provider-identifier",
                "precise": serde_json::from_str::<serde_json::Value>("18446744073709551616001").unwrap(),
            }),
        )]),
    }
}

fn header(
    take: &TakeDescriptor,
    stream: usize,
    sequence: u64,
    start: u64,
    count: u64,
) -> ChunkHeader {
    let source = &take.streams[stream];
    let timestamp = |ticks| HostTimestamp {
        domain: source.host_clock.domain.clone(),
        epoch: source.host_clock.epoch.clone(),
        ticks,
    };
    ChunkHeader {
        version: CHUNK_VERSION,
        take_id: take.take_id.clone(),
        stream_id: source.stream_id.clone(),
        segment_id: source.segment_id.clone(),
        sequence,
        range: if stream == 0 {
            SourceRange::AudioFrames { start, count }
        } else {
            SourceRange::Events { start, count }
        },
        interpretation: source.kind.clone(),
        observed_start: timestamp(start),
        observed_end: timestamp(start + count),
        clock_anchors: vec![],
        kind: ChunkKind::Content,
    }
}

fn event(take: &TakeDescriptor, index: u64) -> Outcome<Vec<u8>> {
    let source = &take.streams[1];
    Ok(Envelope::new(
        SessionId("synthetic-session".into()),
        MessageId(format!("packet-{index}")),
        index,
        CapturedEvent {
            event_id: EventId(format!("event-{index}")),
            source: SourceAddress {
                endpoint: EndpointId(source.source_id.clone()),
                connection_generation: 1,
                group: 0,
                channel: Some(0),
                extensions: BTreeMap::new(),
            },
            time: CaptureTime {
                observed: HostTimestamp {
                    domain: source.host_clock.domain.clone(),
                    epoch: source.host_clock.epoch.clone(),
                    ticks: index,
                },
                alignment: None,
                extensions: BTreeMap::new(),
            },
            raw: RawPacket::Midi1 {
                bytes: vec![0x90, 60, 64],
            },
            musical: MusicalEvent::Uninterpreted,
            extensions: BTreeMap::new(),
        },
    )
    .encode()?)
}

fn publication(
    project: &ProjectDocument,
    snapshot: &SealedJournal,
    stream: &str,
    count: u64,
) -> TakePublication {
    TakePublication {
        operation_id: OperationId(format!("publish-{stream}")),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        track_id: TrackId(format!("recorded-{stream}")),
        track_name: format!("Synthetic {stream}"),
        clip: TakeClip {
            id: format!("clip-{stream}"),
            source: TakeReference {
                take_id: snapshot.descriptor().take_id.clone(),
                manifest_sha256: snapshot.manifest().sha256.clone(),
                stream_id: stream.into(),
            },
            range: if stream == "audio" {
                TakeRange::AudioFrames { start: 0, count }
            } else {
                TakeRange::Events { start: 0, count }
            },
            position: TimelinePosition {
                frame: -480,
                sample_rate: 48_000,
            },
            fades: None,
        },
    }
}

fn rename(project: &ProjectDocument, operation: &str, name: &str) -> ProjectEditIntent {
    ProjectEditIntent {
        operation_id: OperationId(operation.into()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        scope: EditScope::Project,
        edit: EditOperation::Rename { name: name.into() },
        // These collide with publication wrapper names and must remain opaque.
        extensions: BTreeMap::from([
            ("kind".into(), json!("vendor-owned")),
            ("version".into(), json!(912)),
            ("intent".into(), json!({"takeId": "opaque"})),
        ]),
    }
}

fn payload_hash(snapshot: &SealedJournal) -> Outcome<(u64, String)> {
    let mut hash = Sha256::new();
    let mut bytes = 0;
    snapshot.visit_chunks(|_, chunk| {
        hash.update(&chunk.payload);
        bytes += chunk.payload.len() as u64;
        Ok(())
    })?;
    Ok((bytes, format!("{:x}", hash.finalize())))
}

fn main() -> Outcome<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Usage: qualify_native_archive NEW_FIXTURE_ROOT RECEIPT_JSON".into());
    }
    let root = Path::new(&args[0]);
    let receipt_path = Path::new(&args[1]);
    fs::create_dir(root)?; // A qualification never overwrites a previous fixture.
    let source_path = root.join("synthetic-original.sqlite3");
    let take = descriptor();
    let mut writer = Journal::create(&source_path, &take)?;
    let audio = vec![0_u8; MAX_AUDIO_CHUNK_BYTES];
    let frames_per_chunk = audio.len() as u64 / 8;
    for sequence in 0..33 {
        writer.append(
            &header(
                &take,
                0,
                sequence,
                sequence * frames_per_chunk,
                frames_per_chunk,
            ),
            &audio,
        )?;
    }
    for sequence in 0..80 {
        let start = sequence * 64;
        let records: Vec<_> = (start..start + 64)
            .map(|index| event(&take, index))
            .collect::<Outcome<_>>()?;
        let references: Vec<_> = records.iter().map(Vec::as_slice).collect();
        writer.append(
            &header(&take, 1, sequence, start, 64),
            &encode_event_chunk(&references)?,
        )?;
    }
    let manifest = writer.seal("qualification-seal")?;
    drop(writer);
    let source_before = file_hash(&source_path)?;
    let snapshot = SealedJournal::open(&source_path)?;
    let original_payload = payload_hash(&snapshot)?;
    let mut store = NativeStore::open(root.join("source-store"))?;
    let mut value = ProjectDocument::new("Synthetic archive source")?
        .value()
        .clone();
    value["vendor"] = serde_json::from_str(
        r#"{"decimal":0.123456789012345678901234567890123456789,"recording":{"takeId":"opaque"}}"#,
    )?;
    let project = store.create(&ProjectDocument::from_value(value)?)?;
    let object = store.ingest_take(&snapshot)?;
    let audio_intent = publication(&project, &snapshot, "audio", frames_per_chunk * 33);
    let audio_project = store.publish_take(&audio_intent)?.document;
    let events_intent = publication(&audio_project, &snapshot, "events", 5120);
    let events_project = store.publish_take(&events_intent)?.document;
    assert_eq!(events_project.take_clips()?.len(), 2);
    let named = store
        .apply_edit(&rename(&events_project, "first-name", "Before undo"))?
        .document;
    let undone_name = store.undo(named.id(), named.revision())?;
    let undone_events = store.undo(named.id(), undone_name.revision())?;
    let undone_audio = store.undo(named.id(), undone_events.revision())?;
    assert_eq!(undone_audio.schema_version(), 1);
    assert!(undone_audio.referenced_takes()?.is_empty());
    let branch = store
        .apply_edit(&rename(
            &undone_audio,
            "branch-name",
            "Truncated redo; originals retained",
        ))?
        .document;
    assert!(store.redo(branch.id(), branch.revision()).is_err());
    let archive = store.export_archive(branch.id(), &root.join("complete.omniusong-native"))?;
    assert_eq!(archive.take_count, 1);
    assert_eq!(archive.operation_rows, 4);
    assert_eq!(archive.history_rows, 1);
    assert_eq!(store.load(branch.id())?, branch);
    drop(store);
    let mut restored_store = NativeStore::open(root.join("restored-store"))?;
    let restored = restored_store.restore_archive(&archive.path)?;
    assert_eq!(restored, branch);
    let replay = restored_store.publish_take(&audio_intent)?;
    assert!(replay.replayed);
    assert_eq!(replay.document, audio_project);
    assert_eq!(restored_store.load(branch.id())?, branch);
    let restored_original = restored_store.open_take(&audio_intent.clip.source)?;
    assert_eq!(restored_original.manifest(), &manifest);
    assert_eq!(payload_hash(&restored_original)?, original_payload);
    assert_eq!(
        restored_store
            .open_take(&events_intent.clip.source)?
            .manifest(),
        &manifest
    );
    assert_eq!(file_hash(&source_path)?, source_before);
    let registered = restored_store.registered_take(&take.take_id)?;
    assert_eq!(registered.object_sha256, object.object_sha256);
    let source_store = NativeStore::open(root.join("source-store"))?;
    assert_eq!(source_store.load(branch.id())?, branch);
    assert_eq!(
        source_store.registered_take(&take.take_id)?.object_sha256,
        object.object_sha256
    );
    let receipt = json!({
        "format":"omniusong.native-take-archive-qualification", "version":1,
        "evidenceDate":"2026-10-20", "fixtureRoot":root,
        "synthetic":true, "sourceCapturedFromDevice":false,
        "audioPayloadBytes":audio.len() as u64*33, "audioChunks":33, "rawEvents":5120,
        "totalChunkCount":manifest.body.chunk_count, "payloadBytes":original_payload.0,
        "payloadSha256":original_payload.1, "manifestSha256":manifest.sha256,
        "ownedObjectSha256":object.object_sha256, "sourceFileSha256":source_before,
        "sourceFileUnchanged":true, "currentProjectHasNoTakeReference":true,
        "truncatedRedoReceiptsRetainOriginal":true, "opaqueNumericAndCollidingMetadataRetained":true,
        "archive":{"path":archive.path,"sha256":archive.sha256,"byteLength":archive.byte_length,
            "historyRows":archive.history_rows,"operationRows":archive.operation_rows,"takeCount":archive.take_count},
        "restoredProjectEqualsOriginal":true,"restoredOriginalPayloadEqualsSource":true,
        "replayedOriginalPublicationDoesNotReplaceNewerProject":true,
        "limits":["Identity-preserving restore into a new store; restore-as-copy into an existing project remains pending.",
            "Synthetic storage/archive fixture, not elapsed recording, native capture, realtime performance, MIDI protocol qualification, listening, UI archive flow or Windows acceptance."]
    });
    fs::write(receipt_path, serde_json::to_vec_pretty(&receipt)?)?;
    println!(
        "Qualified 33 MiB PCM + 5120 raw events, retained solely by truncated-redo operation receipts; restored originals match. Receipt {}",
        receipt_path.display()
    );
    Ok(())
}

//! Explicit native OUTPUT qualification, driven by synthetic GUI events.
//! Never opens input. This example is manual, never part of `cargo test`.
use omniusong_core::{
    ProjectDocument,
    contracts::{OperationId, ProjectId, TakeId, TrackId},
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{
    performance_records::{PerformanceRecord, decode_record},
    performance_writer::inspect_performance,
    *,
};
use omniusong_recording::{CaptureReader, SealedJournal};
use omniusong_store::{NativeStore, TakePublication};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn fingerprint(path: &Path) -> Result<Value> {
    let bytes = fs::read(path)?;
    Ok(json!({"path":path,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes))}))
}
fn command(client: &mut EngineClient, command: EngineCommand) -> Result<EngineResult> {
    client.try_submit(command)?;
    let deadline = Instant::now() + Duration::from_secs(18);
    loop {
        if let Some(reply) = client.try_poll() {
            return Ok(reply.result?);
        }
        if Instant::now() > deadline {
            return Err("Native qualification control reply deadline".into());
        }
        std::thread::sleep(Duration::from_millis(3));
    }
}
fn wait_phase(
    client: &mut EngineClient,
    id: LiveId,
    phase: PerformancePhase,
) -> Result<PerformanceStatus> {
    let deadline = Instant::now() + Duration::from_secs(18);
    loop {
        match command(client, EngineCommand::PerformanceStatus { start_id: id })? {
            EngineResult::Performance(s) if s.phase == phase => {
                assert!(s.is_valid(), "Invalid status: {s:?}");
                return Ok(*s);
            }
            EngineResult::Performance(s) => {
                assert!(s.is_valid(), "Invalid status: {s:?}");
                if matches!(
                    s.phase,
                    PerformancePhase::Failed
                        | PerformancePhase::SealUncertain
                        | PerformancePhase::Interrupted
                ) {
                    return Err(format!("Performance failed: {s:?}").into());
                }
            }
            r => return Err(format!("Wrong status reply: {r:?}").into()),
        }
        if Instant::now() > deadline {
            return Err("Native qualification phase deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn reliable_stop(client: &mut EngineClient) -> Result<(UrgentTicket, EngineUpdate)> {
    let ticket = client.request_stop()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let _ = client.try_poll();
        if let Some(update) = client.try_poll_update()
            && update.urgent.stop.is_some_and(|a| a.ticket == ticket)
        {
            return Ok((ticket, update));
        }
        if Instant::now() > deadline {
            return Err("Stop acknowledgment deadline".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let executable = fs::canonicalize(PathBuf::from(
        args.next().ok_or("Worker executable required")?,
    ))?;
    let requested = PathBuf::from(
        args.next()
            .ok_or("Fresh qualification directory required")?,
    );
    fs::create_dir(&requested)?;
    let root = fs::canonicalize(requested)?;
    let store_root = root.join("store");
    let mut store = NativeStore::open(&store_root)?;
    let store_root = fs::canonicalize(store_root)?;
    let start = PerformanceStart {
        id: LiveId::random(),
        output_device: None,
        gui_clock: LiveClock {
            domain: "qualify.native.gui".into(),
            epoch: uuid::Uuid::new_v4().to_string(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        },
    };
    let mut client = EngineClient::spawn(executable.clone(), store_root.clone())?;
    let initial = command(
        &mut client,
        EngineCommand::StartPerformance {
            request: start.clone(),
        },
    )?;
    let EngineResult::Performance(initial) = initial else {
        return Err("Start omitted attempted identity".into());
    };
    assert_eq!(initial.identity.start_id, start.id);
    assert_eq!(initial.identity.capture.take_id, start.take_id());
    let recording = wait_phase(&mut client, start.id, PerformancePhase::Recording)?;
    let ready = recording.ready.as_ref().ok_or("No real Ready")?;
    let retry = command(
        &mut client,
        EngineCommand::StartPerformance {
            request: start.clone(),
        },
    )?;
    let EngineResult::Performance(retry) = retry else {
        return Err("Start retry wrong result".into());
    };
    assert_eq!(retry.identity, recording.identity);
    assert_eq!(retry.ready, recording.ready);
    let mut changed = start.clone();
    changed.gui_clock.epoch.push_str("-changed");
    let start_conflict =
        match client.try_submit(EngineCommand::StartPerformance { request: changed }) {
            Err(error) => error,
            Ok(()) => {
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    if let Some(reply) = client.try_poll() {
                        break reply.result.unwrap_err();
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };
    // The GUI adopts asynchronous readiness through this nonblocking mailbox;
    // a control reply alone intentionally never arms local note admission.
    let adopted = client
        .try_performance_update()
        .ok_or("No cached real readiness")?;
    assert_eq!(adopted.ready.as_ref(), Some(ready));
    // Four independently identified GUI sources. Leave voices held so Finish
    // must retain its policy cut rather than inventing human NoteOff events.
    let mut originals = Vec::new();
    let mut batches = Vec::new();
    for sequence in 1..=4 {
        let mut batch = LiveBatch::empty(
            ready.scope,
            sequence,
            ready.progress.lease_credit.map(|c| c.token),
        );
        let event = LiveEvent {
            source: sequence as u8,
            connection_generation: 1,
            observed_ticks: (1u64 << 53) + sequence,
            event: LiveEventKind::NoteOn {
                note: 1,
                voice: 1,
                key: 59 + sequence as u8,
                velocity16: 42000 + sequence as u16,
                patch: LivePatch::Keys,
            },
        };
        batch.try_push(event)?;
        client.try_push_live(batch)?;
        originals.push(event);
        batches.push(batch);
    }
    let finish_id = LiveId::random();
    let finish = EngineCommand::FinishPerformance {
        start_id: start.id,
        scope: ready.scope,
        finish_id,
    };
    let begun = command(&mut client, finish.clone())?;
    assert!(matches!(begun, EngineResult::Performance(_)));
    let sealed = wait_phase(&mut client, start.id, PerformancePhase::Sealed)?;
    // Normal manifest publication precedes the writer thread's final Drop.
    // Require its real Stop ACK before acquiring a new exclusive review owner.
    let (_, normal_stop_ack) = reliable_stop(&mut client)?;
    assert_eq!(sealed.finish.as_ref().unwrap().accepted_sequence, Some(4));
    assert_eq!(sealed.cut.as_ref().unwrap().fence.id, finish_id);
    assert!(sealed.native.complete());
    let take = sealed.sealed.as_ref().ok_or("No normal manifest")?;
    let journal = SealedJournal::open(&take.journal_path)?;
    assert!(journal.manifest().body.interrupted.is_none());
    assert_eq!(journal.manifest().sha256, take.manifest_sha256);
    let reader = CaptureReader::open(&take.journal_path)?;
    let original_rows = reader.inspection().original_rows_sha256.clone();
    let semantic = inspect_performance(&reader)?;
    assert!(semantic.problem.is_none());
    assert_eq!(semantic.finish_cut, sealed.cut);
    drop(reader);
    let mut actual_events = Vec::new();
    let mut actual_batches = Vec::new();
    let mut pcm = Sha256::new();
    let mut pcm_bytes = 0;
    let mut nonzero = false;
    let mut finishes = 0;
    for ordinal in 1..=journal.manifest().body.chunk_count {
        let chunk = journal.read_ordinal(ordinal)?;
        if chunk.header.stream_id == "audio" {
            pcm.update(&chunk.payload);
            pcm_bytes += chunk.payload.len();
            nonzero |= chunk
                .payload
                .as_chunks::<4>()
                .0
                .iter()
                .any(|b| f32::from_le_bytes(*b).abs() > 0.0);
            continue;
        }
        let mut at = 0;
        while at < chunk.payload.len() {
            let length = u32::from_le_bytes(chunk.payload[at..at + 4].try_into()?) as usize;
            at += 4;
            match decode_record(&chunk.payload[at..at + length])?.record {
                PerformanceRecord::AppliedEvent { original, .. } => actual_events.push(original),
                PerformanceRecord::Batch { batch, .. } => actual_batches.push(*batch),
                PerformanceRecord::Finish { finish, .. } => {
                    assert_eq!(Some(finish), sealed.cut);
                    finishes += 1;
                }
                _ => {}
            }
            at += length;
        }
    }
    actual_events.sort_by_key(|e| e.source);
    assert_eq!(actual_events, originals);
    assert_eq!(actual_batches, batches);
    assert_eq!(finishes, 1);
    assert!(nonzero);
    assert_eq!(pcm_bytes as u64, take.frames * u64::from(take.channels) * 4);
    let stable_before = fingerprint(&take.journal_path)?;
    let again = command(&mut client, finish)?;
    let EngineResult::Performance(again) = again else {
        return Err("Finish retry wrong result".into());
    };
    assert_eq!(again.sealed, sealed.sealed);
    assert_eq!(again.cut, sealed.cut);
    assert_eq!(fingerprint(&take.journal_path)?, stable_before);
    let owned = store.ingest_take(&journal)?;
    let project = store.create(&ProjectDocument::new(
        "Native output performance · synthetic controls",
    )?)?;
    let reference = TakeReference {
        take_id: TakeId(take.take_id.clone()),
        manifest_sha256: take.manifest_sha256.clone(),
        stream_id: "audio".into(),
    };
    let publication = TakePublication {
        operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
        project_id: ProjectId(project.id().into()),
        expected_revision: project.revision(),
        track_id: TrackId(uuid::Uuid::new_v4().to_string()),
        track_name: "Native procedural output, uncalibrated placement".into(),
        clip: TakeClip {
            id: uuid::Uuid::new_v4().to_string(),
            source: reference.clone(),
            range: TakeRange::AudioFrames {
                start: 0,
                count: take.frames,
            },
            position: TimelinePosition {
                frame: 0,
                sample_rate: take.sample_rate,
            },
            fades: None,
            edit_source: None,
        },
    };
    let attached = store.publish_take(&publication)?.document;
    let undone = store.undo(project.id(), attached.revision())?;
    assert_eq!(undone.tracks(), project.tracks());
    let redone = store.redo(project.id(), undone.revision())?;
    assert_eq!(redone.tracks(), attached.tracks());
    let archive_path = root.join("native-performance.omniusong-native");
    let archive = store.export_archive(project.id(), &archive_path)?;
    let mut restored = NativeStore::open(root.join("restored-store"))?;
    assert_eq!(restored.restore_archive(&archive_path)?, redone);
    assert_eq!(
        restored.registered_take(&TakeId(take.take_id.clone()))?,
        owned
    );
    let restored_take = restored.open_take(&reference)?;
    assert_eq!(restored_take.descriptor(), journal.descriptor());
    assert_eq!(restored_take.manifest(), journal.manifest());
    let restored_chunks = chunks_digest(&restored_take)?;
    assert_eq!(restored_chunks, chunks_digest(&journal)?);
    assert_eq!(fingerprint(&take.journal_path)?, stable_before);
    // Urgent Stop uses a distinct second take; normal Finish authority must not
    // be minted merely because the native closures have retired.
    let mut stopped_start = start.clone();
    stopped_start.id = LiveId::random();
    command(
        &mut client,
        EngineCommand::StartPerformance {
            request: stopped_start.clone(),
        },
    )?;
    let stop_ready = wait_phase(&mut client, stopped_start.id, PerformancePhase::Recording)?;
    let (ticket, stopped) = reliable_stop(&mut client)?;
    assert!(stop_ready.sealed.is_none());
    let stop_status = command(
        &mut client,
        EngineCommand::PerformanceStatus {
            start_id: stopped_start.id,
        },
    )?;
    let EngineResult::Performance(stop_status) = stop_status else {
        return Err("Stopped identity disappeared".into());
    };
    assert!(stop_status.sealed.is_none());
    assert_eq!(stop_status.phase, PerformancePhase::Interrupted);
    assert!(SealedJournal::open(&stop_status.identity.capture.journal_path).is_err());
    drop(client);
    let mut fresh = EngineClient::spawn(executable.clone(), store_root)?;
    command(
        &mut fresh,
        EngineCommand::StartPerformance {
            request: start.clone(),
        },
    )?;
    let existing_refusal = wait_phase(&mut fresh, start.id, PerformancePhase::Failed)?;
    assert!(existing_refusal.ready.is_none() && existing_refusal.sealed.is_none());
    assert_eq!(existing_refusal.identity.capture, take_identity(&sealed));
    assert_eq!(fingerprint(&take.journal_path)?, stable_before);
    let receipt = json!({"format":"omniusong.native-performance-worker-qualification","version":1,"workerExecutable":fingerprint(&executable)?,"start":start,"initial":initial,"recording":recording,"sealed":sealed,"finishReplay":again,"startChangedIntentRejection":start_conflict,"exactOriginalEvents":originals,"exactOriginalBatches":batches,"pcmBytes":pcm_bytes,"pcmSha256":format!("{:x}",pcm.finalize()),"normalManifest":journal.manifest(),"semanticReview":semantic,"stage":stable_before,"originalRowsSha256":original_rows,"archive":fingerprint(&archive_path)?,"archiveCounts":{"history":archive.history_rows,"operations":archive.operation_rows,"takes":archive.take_count},"ownedTake":owned,"restoredAllChunksSha256":restored_chunks,"normalStopAcknowledgment":normal_stop_ack,"stopTicket":ticket,"stopAcknowledgment":stopped,"stopped":stop_status,"existingStageRefusalAfterWorkerRestart":existing_refusal,"nativeInputOpened":false,"nativeOutputOpened":true,"humanPerformance":false,"listeningAcceptance":false,"limits":["Synthetic GUI events driving actual native output. Not physical controller or microphone acceptance.","Placement uncalibrated. No latency or thirty-minute reliability claim.","No new installed UI, model/plugin, installer or Windows acceptance."]});
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{}", root.join("receipt.json").display());
    Ok(())
}
fn take_identity(status: &PerformanceStatus) -> CaptureIdentity {
    status.identity.capture.clone()
}

fn chunks_digest(journal: &SealedJournal) -> Result<String> {
    let mut digest = Sha256::new();
    for ordinal in 1..=journal.manifest().body.chunk_count {
        let chunk = journal.read_ordinal(ordinal)?;
        let header = chunk.header.canonical_bytes()?;
        digest.update((header.len() as u64).to_le_bytes());
        digest.update(header);
        digest.update((chunk.payload.len() as u64).to_le_bytes());
        digest.update(chunk.payload);
    }
    Ok(format!("{:x}", digest.finalize()))
}

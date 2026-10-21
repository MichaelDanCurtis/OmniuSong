#![cfg(feature = "test-hooks")]
//! Actual helper-process exit at atomic append boundaries. process::exit(77)
//! bypasses Rust destructors; this is not physical power-loss qualification.
use omniusong_recording::*;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const PATH_ENV: &str = "OMNIUSONG_APPEND_GROUP_PROCESS_PATH";
const BOUNDARY_ENV: &str = "OMNIUSONG_APPEND_GROUP_PROCESS_BOUNDARY";
const EXIT_CODE: i32 = 77;
const MAX_FIXTURE_BYTES: u64 = 4 * 1024 * 1024;

fn descriptor() -> TakeDescriptor {
    let streams = (0..6)
        .map(|index| StreamDescriptor {
            stream_id: format!("synthetic-{index}"),
            segment_id: format!("segment-{index}"),
            source_id: format!("software-source-{index}"),
            source_name: format!("Synthetic PCM stream {index}; no device"),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("synthetic-frame-clock".into()),
                epoch: ClockEpochId("synthetic-epoch".into()),
                ticks_per_second_numerator: 48_000,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: 48_000,
                    channels: 1,
                    encoding: PcmEncoding::F32Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: ClockEpochId(format!("audio-epoch-{index}")),
            },
            initial_source_position: 1000 + index * 100,
        })
        .collect();
    TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("append-group-process-fixture".into()),
        streams,
        provenance: BTreeMap::from([(
            "fixture".into(),
            json!({"synthetic":true,"human":false,"deviceOpened":false,"processExitCode":EXIT_CODE,"physicalPowerLoss":false}),
        )]),
    }
}
fn headers(take: &TakeDescriptor) -> Vec<ChunkHeader> {
    take.streams
        .iter()
        .enumerate()
        .map(|(index, stream)| {
            let stamp = HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: 20 + index as u64,
            };
            ChunkHeader {
                version: CHUNK_VERSION,
                take_id: take.take_id.clone(),
                stream_id: stream.stream_id.clone(),
                segment_id: stream.segment_id.clone(),
                sequence: 0,
                range: SourceRange::AudioFrames {
                    start: stream.initial_source_position,
                    count: 4,
                },
                interpretation: stream.kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            }
        })
        .collect()
}
fn payload(index: usize) -> Vec<u8> {
    let scale = (index + 1) as f32 / 16.0;
    [-scale, 0.0, scale / 2.0, scale]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect()
}
fn requests<'a>(headers: &'a [ChunkHeader], payloads: &'a [Vec<u8>]) -> Vec<AppendChunk<'a>> {
    headers
        .iter()
        .zip(payloads)
        .map(|(header, payload)| AppendChunk { header, payload })
        .collect()
}

/// Only this explicitly selected helper exits the process. Normal test runs
/// retain one ignored entry; the parent below invokes it exactly and records
/// each checked exit and recovered prefix in its fresh receipt.json artifact.
#[test]
#[ignore = "Owned subprocess helper; invoke only through append_group_process_interruption_is_all_or_nothing"]
fn append_group_process_child_helper() {
    let path =
        PathBuf::from(std::env::var_os(PATH_ENV).expect("Parent must provide fresh fixture path"));
    let boundary = std::env::var(BOUNDARY_ENV).expect("Parent must provide one exact boundary");
    let insert_target = boundary
        .strip_prefix("after-insert-")
        .map(|number| number.parse::<usize>().unwrap());
    assert!(
        insert_target.is_some_and(|target| (1..=6).contains(&target))
            || matches!(boundary.as_str(), "before-commit" | "after-commit")
    );
    let take = descriptor();
    let headers = headers(&take);
    let payloads: Vec<_> = (0..6).map(payload).collect();
    let mut journal = Journal::create(&path, &take).unwrap();
    let mut inserted = 0;
    journal.set_boundary_hook(move |point| {
        if point == WriteBoundary::AfterChunkInsert {
            inserted += 1;
        }
        if (point == WriteBoundary::AfterChunkInsert && insert_target == Some(inserted))
            || (point == WriteBoundary::BeforeChunkCommit && boundary == "before-commit")
            || (point == WriteBoundary::AfterChunkCommit && boundary == "after-commit")
        {
            std::process::exit(EXIT_CODE);
        }
        Ok(())
    });
    let _ = journal
        .append_group(&requests(&headers, &payloads))
        .unwrap();
    panic!("Requested append-group boundary did not exit the owned helper");
}

struct OwnedChild(Option<Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}
fn child_exit(path: &Path, boundary: &str) -> ExitStatus {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "append_group_process_child_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PATH_ENV, path)
        .env(BOUNDARY_ENV, boundary)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = OwnedChild(Some(child));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            let output = child.0.take().unwrap().wait_with_output().unwrap();
            assert_eq!(status, output.status);
            assert_eq!(
                status.code(),
                Some(EXIT_CODE),
                "Boundary {boundary}: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return status;
        }
        if Instant::now() >= deadline {
            let state = child.0.as_mut().unwrap().try_wait().unwrap();
            panic!(
                "Owned append-group helper timed out at {boundary}; state={state:?}; fixture={}",
                path.display()
            );
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn append_group_process_interruption_is_all_or_nothing() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/append-group-process")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root).unwrap();
    let boundaries: Vec<_> = (1..=6)
        .map(|index| format!("after-insert-{index}"))
        .chain(["before-commit".into(), "after-commit".into()])
        .collect();
    let take = descriptor();
    let headers = headers(&take);
    let payloads: Vec<_> = (0..6).map(payload).collect();
    let mut cases = Vec::new();
    let mut fixture_bytes = 0u64;
    for boundary in boundaries {
        let stage = root.join(&boundary);
        fs::create_dir(&stage).unwrap();
        let path = stage.join("take.sqlite3");
        // No parent journal is open while spawning. Reopen happens only after
        // the owned helper has exited/reaped and released its OS owner lock.
        let status = child_exit(&path, &boundary);
        let committed = boundary == "after-commit";
        let expected_chunks = if committed { 6 } else { 0 };
        let mut journal = Journal::open(&path).unwrap();
        assert_eq!(journal.state().unwrap(), TakeState::Interrupted);
        assert_eq!(journal.descriptor(), &take);
        let report = journal.recovery_report().unwrap();
        assert!(
            report.problems.is_empty(),
            "{boundary}: {:?}",
            report.problems
        );
        assert_eq!(report.verified_chunks, expected_chunks);
        assert_eq!(
            report.verified_payload_bytes,
            if committed { 96 } else { 0 }
        );
        assert!(report.unknown_tail);
        let connection =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let rows: i64 = connection
            .query_row("SELECT count(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(u64::try_from(rows).unwrap(), expected_chunks);
        let mut chunk_hashes = Vec::new();
        let mut payload_hashes = Vec::new();
        for (index, (prefix, stream)) in report.streams.iter().zip(&take.streams).enumerate() {
            assert_eq!(prefix.stream_id, stream.stream_id);
            assert_eq!(prefix.next_sequence, u64::from(committed));
            assert_eq!(
                prefix.source_end.value(),
                stream.initial_source_position + if committed { 4 } else { 0 }
            );
            assert_eq!(prefix.recorded_units, if committed { 4 } else { 0 });
            assert_eq!((prefix.gap_units, prefix.gap_chunks), (0, 0));
            let cached: (i64,i64,i64,i64,i64) = connection.query_row("SELECT next_sequence,source_end,recorded_units,gap_units,gap_chunks FROM stream_prefix WHERE stream_id=?1", [&stream.stream_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap();
            assert_eq!(
                cached,
                (
                    i64::from(committed),
                    i64::try_from(prefix.source_end.value()).unwrap(),
                    if committed { 4 } else { 0 },
                    0,
                    0
                )
            );
            if committed {
                let stored = journal.read_chunk(&stream.stream_id, 0).unwrap();
                assert_eq!(stored.header, headers[index]);
                assert_eq!(stored.payload, payloads[index]);
                assert_eq!(
                    stored.sha256,
                    headers[index].sha256(&payloads[index]).unwrap()
                );
                assert_eq!(
                    Sha256::digest(&stored.payload),
                    Sha256::digest(&payloads[index])
                );
                payload_hashes.push(format!("{:x}", Sha256::digest(&stored.payload)));
                chunk_hashes.push(stored.sha256);
            } else {
                assert!(journal.read_chunk(&stream.stream_id, 0).is_err());
            }
        }
        let replayed = if committed {
            let replay = journal
                .append_group(&requests(&headers, &payloads))
                .unwrap();
            assert!(replay.replayed);
            assert_eq!((replay.first_ordinal, replay.last_ordinal), (1, 6));
            true
        } else {
            assert!(matches!(
                journal.append_group(&requests(&headers, &payloads)),
                Err(RecordingError::Interrupted)
            ));
            false
        };
        assert_eq!(journal.recovery_report().unwrap(), report);
        drop(connection);
        drop(journal);
        // Retain only this bounded, synthetic run's originals and receipt.
        for entry in fs::read_dir(&stage).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            fixture_bytes = fixture_bytes
                .checked_add(entry.metadata().unwrap().len())
                .unwrap();
        }
        cases.push(json!({"boundary":boundary,"childExitCode":status.code(),"journalPath":path,"expectedCommitted":committed,"rows":rows,"verifiedChunks":report.verified_chunks,"verifiedPayloadBytes":report.verified_payload_bytes,"allSixPrefixesChecked":true,"chunkSha256":chunk_hashes,"payloadSha256":payload_hashes,"exactGroupRetryReplayed":replayed,"stateAfterReopen":"interrupted","unknownTail":true}));
    }
    let receipt = serde_json::to_vec_pretty(&json!({"qualification":"append-group-owned-process-exit-v1","synthetic":true,"human":false,"deviceOpened":false,"physicalPowerLoss":false,"engineWorkerQualified":false,"helperTest":"append_group_process_child_helper","expectedHelperExitCode":EXIT_CODE,"caseCount":cases.len(),"fixtureBytesBeforeReceipt":fixture_bytes,"maxFixtureBytes":MAX_FIXTURE_BYTES,"cases":cases})).unwrap();
    assert!(
        fixture_bytes
            .checked_add(receipt.len() as u64)
            .and_then(|total| total.checked_add(1))
            .is_some_and(|total| total <= MAX_FIXTURE_BYTES)
    );
    let receipt_path = root.join("receipt.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)
        .unwrap();
    file.write_all(&receipt).unwrap();
    file.write_all(b"\n").unwrap();
    file.sync_all().unwrap();
    eprintln!(
        "Append-group process receipt retained at {}",
        receipt_path.display()
    );
}

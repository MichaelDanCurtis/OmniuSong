//! Independent device-free integration of the actual live tap and durable writer.
//! Every file is a synthetic, isolated fixture; no devices or live catalog access.
use omniusong_core::contracts::{
    AnchorId, AudioFrame, CapturedEvent, ClockAnchor, ClockEpochId, EndpointId, Envelope, TakeId,
};
use omniusong_engine::{
    live::{LiveAdmission, LiveRenderer, recording_live_pair},
    performance_records::{
        LocalRecord, PerformanceConfig, PerformanceRecord, PerformanceSource, STREAM_IDS,
        decode_envelope, encode_record,
    },
    performance_tap::{PerformancePacket, PerformancePacketKind, PerformanceTapOwner, TapInfo},
    performance_writer::{PerformanceWriter, inspect_performance, recover_performance},
    *,
};
use omniusong_recording::{
    CaptureReader, ChunkHeader, Journal, RecordingError, SealedJournal, SourcePosition,
    encode_event_chunk,
};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

type OriginalRow = (i64, String, i64, Vec<u8>, Vec<u8>, String);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-writer-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn sql(&self) -> Connection {
        Connection::open_with_flags(self.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
    }
    fn rows(&self) -> u64 {
        let value: i64 = self
            .sql()
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        u64::try_from(value).unwrap()
    }
    fn originals(&self) -> Vec<OriginalRow> {
        let connection = self.sql();
        let mut statement=connection.prepare("SELECT ordinal,stream_id,sequence,header,payload,sha256 FROM chunks ORDER BY ordinal").unwrap();
        statement
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn config(channels: u16, rate: u32) -> PerformanceConfig {
    PerformanceConfig {
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        info: TapInfo {
            scope: LiveScope {
                worker_generation: LiveId([61; 16]),
                session_id: LiveId([83; 16]),
                panic_generation: 1,
            },
            sample_rate: rate,
            channels,
        },
        gui_clock: LiveClock {
            domain: "review.gui".into(),
            epoch: "review.gui.epoch".into(),
            ticks_per_second_numerator: 1_000_000_000_000,
            ticks_per_second_denominator: 1_000_000_000_000,
        },
        output_epoch: ClockEpochId("review.output.epoch".into()),
        sources: std::array::from_fn(|index| PerformanceSource {
            binding: LiveSourceBinding {
                kind: [
                    LiveSourceKind::ComputerKeys,
                    LiveSourceKind::PointerKeys,
                    LiveSourceKind::Pads,
                    LiveSourceKind::AccessiblePulse,
                ][index],
                handle: index as u8 + 1,
                connection_generation: 1,
            },
            endpoint: EndpointId(format!("review.local-source-{}", index + 1)),
        }),
    }
}
struct Tap {
    admission: LiveAdmission,
    renderer: Option<LiveRenderer>,
    owner: PerformanceTapOwner,
    channels: usize,
}
impl Tap {
    fn new(config: &PerformanceConfig) -> Self {
        let (admission, renderer, owner) = recording_live_pair(
            config.info.scope,
            config.info.sample_rate,
            config.info.channels,
        )
        .unwrap();
        Self {
            admission,
            renderer: Some(renderer),
            owner,
            channels: usize::from(config.info.channels),
        }
    }
    fn admit(&mut self, events: &[LiveEvent]) -> LiveBatch {
        let progress = self.admission.progress();
        let mut batch = LiveBatch::empty(
            progress.scope,
            progress.next_sequence().unwrap(),
            self.admission.lease_credit().map(|v| v.token),
        );
        for event in events {
            batch.try_push(*event).unwrap();
        }
        self.admission.admit(batch).unwrap();
        batch
    }
    fn render(&mut self, frames: usize) -> (Vec<PerformancePacket>, Vec<u8>) {
        let mut samples = vec![0.0; frames * self.channels];
        self.renderer.as_mut().unwrap().render_into(&mut samples);
        let mut packets = Vec::new();
        while let Some(packet) = self.owner.try_pop().unwrap() {
            packets.push(packet);
        }
        (
            packets,
            samples.into_iter().flat_map(f32::to_le_bytes).collect(),
        )
    }
}
impl Drop for Tap {
    fn drop(&mut self) {
        // Required retention order: renderer/producer before admission backing.
        drop(self.renderer.take());
    }
}
fn on(source: u8, voice: u64, ticks: u64, velocity16: u16) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: ticks,
        event: LiveEventKind::NoteOn {
            note: voice,
            voice,
            key: 60,
            velocity16,
            patch: LivePatch::Keys,
        },
    }
}
fn off(source: u8, voice: u64, ticks: u64, release_velocity16: u16) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: ticks,
        event: LiveEventKind::NoteOff {
            voice,
            release_velocity16,
        },
    }
}
fn sustain(source: u8, ticks: u64, down: bool) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: ticks,
        event: LiveEventKind::Sustain { down },
    }
}
fn push(writer: &mut PerformanceWriter, packets: &[PerformancePacket]) {
    for packet in packets {
        writer.push_packet(*packet).unwrap();
    }
}
fn raw_records(payload: &[u8]) -> Vec<Vec<u8>> {
    let mut rest = payload;
    let mut records = Vec::new();
    while !rest.is_empty() {
        assert!(rest.len() >= 4);
        let n = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        assert!(n > 0 && n <= rest.len() - 4);
        records.push(rest[4..4 + n].to_vec());
        rest = &rest[4 + n..];
    }
    records
}
fn capture_records(reader: &CaptureReader) -> Vec<(String, Envelope<CapturedEvent>, LocalRecord)> {
    let mut records = Vec::new();
    for ordinal in 1..=reader.inspection().report.verified_chunks {
        let chunk = reader.read_ordinal(ordinal).unwrap();
        if chunk.header.stream_id == "audio" {
            continue;
        }
        for bytes in raw_records(&chunk.payload) {
            let (envelope, record) = decode_envelope(&bytes).unwrap();
            records.push((chunk.header.stream_id.clone(), envelope, record));
        }
    }
    records
}

#[test]
fn exact_bus_and_full_precision_local_originals_survive_six_stream_recovery() {
    let f = Fixture::new();
    let cfg = config(2, 48_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg.clone()).unwrap();
    let mut originals = Vec::new();
    for source in 1..=4 {
        originals.push(on(source, 1, u64::MAX - 1, u16::MAX));
    }
    tap.admit(&originals);
    assert_eq!(
        writer.progress().received_packets,
        0,
        "Queue admission is not original capture"
    );
    let (packets, mut pcm) = tap.render(257);
    push(&mut writer, &packets);
    let releases: Vec<_> = (1..=4)
        .map(|source| off(source, 1, u64::MAX, if source == 1 { 0 } else { u16::MAX }))
        .collect();
    tap.admit(&releases);
    originals.extend(releases);
    let (packets, tail) = tap.render(511);
    pcm.extend(tail);
    push(&mut writer, &packets);
    assert_eq!(writer.progress().durable_frames, 0);
    let progress = writer.flush().unwrap();
    assert_eq!(progress.durable_frames, 768);
    assert_eq!(progress.durable_callbacks, 2);
    assert!(progress.first_error.is_none());
    assert!(progress.capture_fault.is_none());
    let descriptor = writer.descriptor().clone();
    assert_eq!(descriptor.streams.len(), 6);
    assert_eq!(
        descriptor
            .streams
            .iter()
            .map(|s| s.stream_id.as_str())
            .collect::<Vec<_>>(),
        STREAM_IDS
    );
    for stream in &descriptor.streams[1..5] {
        assert_eq!(
            (
                stream.host_clock.ticks_per_second_numerator,
                stream.host_clock.ticks_per_second_denominator
            ),
            (1, 1)
        );
    }
    assert_eq!(
        descriptor.provenance["omniusong.performance.v1"]["config"]["gui_clock"]["ticks_per_second_denominator"],
        1_000_000_000_000u64
    );
    writer.close_interrupted().unwrap();
    drop(tap);
    let before = fs::read(f.path()).unwrap();
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert!(review.problem.is_none());
    assert!(review.unknown_tail);
    assert_eq!(review.checkpoint.as_ref().unwrap().audio_end, 768);
    let records = capture_records(&reader);
    let mut seen = Vec::new();
    let mut exact_pcm = Vec::new();
    for ordinal in 1..=reader.inspection().report.verified_chunks {
        let chunk = reader.read_ordinal(ordinal).unwrap();
        assert!(chunk.header.clock_anchors.is_empty());
        if chunk.header.stream_id == "audio" {
            exact_pcm.extend_from_slice(&chunk.payload);
        }
    }
    assert_eq!(exact_pcm, pcm);
    for (stream, envelope, record) in records {
        assert!(envelope.payload.time.alignment.is_none());
        assert!(envelope.payload.source.channel.is_none());
        if let PerformanceRecord::AppliedEvent { original, .. } = record.record {
            assert_eq!(stream, format!("gui-{}", original.source));
            assert_eq!(
                envelope.payload.time.observed.ticks,
                original.observed_ticks
            );
            seen.push(original);
        }
    }
    for original in originals {
        assert_eq!(seen.iter().filter(|v| **v == original).count(), 1);
    }
    assert_eq!(seen.len(), 8);
    assert_eq!(
        fs::read(f.path()).unwrap(),
        before,
        "Inspection must not change original bytes"
    );
    let originals_before = f.originals();
    let (manifest, replayed) = recover_performance(reader, &review, "review-recover").unwrap();
    assert!(!replayed);
    assert!(manifest.body.interrupted.is_some());
    assert_eq!(
        manifest.body.streams[0].source_end,
        SourcePosition::AudioFrames(768)
    );
    assert_eq!(f.originals(), originals_before);
    assert_eq!(SealedJournal::open(f.path()).unwrap().manifest(), &manifest);
    let (again, replayed) = recover_performance(
        CaptureReader::open(f.path()).unwrap(),
        &review,
        "review-recover",
    )
    .unwrap();
    assert!(replayed);
    assert_eq!(again, manifest);
    assert!(
        recover_performance(
            CaptureReader::open(f.path()).unwrap(),
            &review,
            "different-recovery"
        )
        .is_err()
    );
}

#[test]
fn live_writer_ownership_and_unrepresentable_clock_fail_without_takeover_or_creation() {
    let f = Fixture::new();
    let mut cfg = config(2, 48_000);
    cfg.gui_clock.ticks_per_second_numerator = 1;
    cfg.gui_clock.ticks_per_second_denominator = 1_000_000_000_000;
    assert!(PerformanceWriter::create(&f.path(), cfg).is_err());
    assert!(!f.path().exists());
    let cfg = config(2, 48_000);
    let mut writer = PerformanceWriter::create(&f.path(), cfg.clone()).unwrap();
    let mut tap = Tap::new(&cfg);
    let (packets, _) = tap.render(1);
    push(&mut writer, &packets);
    writer.flush().unwrap();
    let before = fs::read(f.path()).unwrap();
    assert!(matches!(
        CaptureReader::open(f.path()),
        Err(RecordingError::Owned)
    ));
    assert_eq!(fs::read(f.path()).unwrap(), before);
    writer.close_interrupted().unwrap();
    drop(tap);
    let before = fs::read(f.path()).unwrap();
    assert!(
        CaptureReader::open(f.path())
            .unwrap()
            .recover("generic-is-not-performance")
            .is_err()
    );
    assert_eq!(fs::read(f.path()).unwrap(), before);
    let mut journal = Journal::open(f.path()).unwrap();
    assert!(journal.seal("not-a-finish-proof").is_err());
    assert!(journal.seal_recovered("wrong-prefix-policy").is_err());
}

#[test]
fn rejected_atomic_batch_is_retained_without_gui_events_or_unrecorded_sound() {
    let f = Fixture::new();
    let cfg = config(2, 48_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
    let original = tap.admit(&[on(1, 1, 10, 65535), off(1, 99, 11, 12345)]);
    let (packets, pcm) = tap.render(32);
    assert!(pcm.iter().all(|b| *b == 0));
    push(&mut writer, &packets);
    let progress = writer.flush().unwrap();
    assert!(progress.first_error.is_none());
    assert!(progress.capture_fault.is_some());
    writer.close_interrupted().unwrap();
    drop(tap);
    let reader = CaptureReader::open(f.path()).unwrap();
    let records = capture_records(&reader);
    assert!(
        !records
            .iter()
            .any(|(_, _, r)| matches!(r.record, PerformanceRecord::AppliedEvent { .. }))
    );
    assert!(records.iter().any(|(_,_,r)|matches!(&r.record,PerformanceRecord::Batch{batch,disposition:omniusong_engine::performance_tap::BatchDisposition::Rejected{..},..} if **batch==original)));
    let review = inspect_performance(&reader).unwrap();
    assert!(review.selection.is_some());
    assert!(review.unknown_tail);
}

#[test]
fn panic_disposals_and_lease_boundary_are_originals_not_human_releases() {
    let f = Fixture::new();
    let cfg = config(1, 8_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
    tap.admit(&[on(1, 1, 9_007_199_254_740_993, 65535)]);
    let (packets, _) = tap.render(1);
    push(&mut writer, &packets);
    let original = tap.admit(&[sustain(2, 40, true)]);
    let old_scope = tap.admission.progress().scope;
    let next_scope = tap.admission.begin_panic(old_scope).unwrap();
    let (packets, _) = tap.render(1);
    push(&mut writer, &packets);
    assert!(tap.admission.finish_panic().unwrap().is_some());
    assert_eq!(tap.admission.progress().scope, next_scope);
    tap.admit(&[on(1, 1, 9_007_199_254_740_994, 1)]);
    let (packets, _) = tap.render(8_000);
    push(&mut writer, &packets);
    let progress = writer.flush().unwrap();
    assert!(progress.first_error.is_none());
    assert_eq!(
        progress.capture_fault.unwrap().kind,
        LiveFaultKind::LeaseExpired
    );
    writer.close_interrupted().unwrap();
    drop(tap);
    let reader = CaptureReader::open(f.path()).unwrap();
    let records = capture_records(&reader);
    assert!(records.iter().any(|(_,_,r)|matches!(&r.record,PerformanceRecord::Batch{batch,disposition:omniusong_engine::performance_tap::BatchDisposition::DiscardedPanic,..} if **batch==original)));
    assert!(records.iter().any(|(_,_,r)|matches!(r.record,PerformanceRecord::Clear{clear,..} if clear.frame==8_002 && matches!(clear.reason,omniusong_engine::performance_tap::TapClearReason::Fault{kind:LiveFaultKind::LeaseExpired}))));
    assert!(!records.iter().any(|(_, _, r)| matches!(
        r.record,
        PerformanceRecord::AppliedEvent {
            original: LiveEvent {
                event: LiveEventKind::NoteOff { .. },
                ..
            },
            ..
        }
    )));
    assert_eq!(
        inspect_performance(&reader)
            .unwrap()
            .checkpoint
            .unwrap()
            .scope_after,
        next_scope
    );
}

#[test]
fn incomplete_callback_tail_keeps_received_originals_but_cannot_extend_recovery() {
    let f = Fixture::new();
    let cfg = config(2, 48_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
    tap.admit(&[on(1, 1, 100, 65535)]);
    let (packets, _) = tap.render(64);
    push(&mut writer, &packets);
    writer.flush().unwrap();
    let complete_rows = f.rows();
    let original = tap.admit(&[sustain(2, 200, true)]);
    let (packets, _) = tap.render(512);
    let first_pcm = packets
        .iter()
        .position(|p| matches!(p.kind, PerformancePacketKind::Pcm(_)))
        .unwrap();
    push(&mut writer, &packets[..=first_pcm]);
    let retained = writer.close_interrupted().unwrap();
    assert_eq!(retained.progress.durable_frames, 64);
    assert!(retained.unknown_tail);
    drop(tap);
    assert!(f.rows() > complete_rows);
    let originals = f.originals();
    let reader = CaptureReader::open(f.path()).unwrap();
    assert!(capture_records(&reader).iter().any(
        |(_, _, r)| matches!(&r.record,PerformanceRecord::Batch{batch,..} if **batch==original)
    ));
    let review = inspect_performance(&reader).unwrap();
    assert_eq!(
        review.selection.as_ref().unwrap().chunk_count,
        complete_rows
    );
    assert_eq!(review.checkpoint.as_ref().unwrap().audio_end, 64);
    assert!(review.unknown_tail);
    let (sealed, _) = recover_performance(reader, &review, "keep-partial-original-tail").unwrap();
    assert_eq!(sealed.body.chunk_count, complete_rows);
    assert_eq!(
        sealed.body.streams[0].source_end,
        SourcePosition::AudioFrames(64)
    );
    assert_eq!(f.originals(), originals);
}

#[test]
fn missing_end_without_any_checkpoint_cannot_become_playable_recovery() {
    let f = Fixture::new();
    let cfg = config(1, 48_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
    tap.admit(&[on(1, 1, 90, 65535)]);
    let (packets, _) = tap.render(32);
    push(&mut writer, &packets[..packets.len() - 1]);
    writer.close_interrupted().unwrap();
    drop(tap);
    let reader = CaptureReader::open(f.path()).unwrap();
    assert!(reader.inspection().report.verified_chunks > 0);
    let review = inspect_performance(&reader).unwrap();
    assert!(review.selection.is_none());
    let before = fs::read(f.path()).unwrap();
    assert!(recover_performance(reader, &review, "no-complete-checkpoint").is_err());
    assert_eq!(fs::read(f.path()).unwrap(), before);
}

fn two_groups(f: &Fixture) -> (PerformanceConfig, u64) {
    let cfg = config(2, 48_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg.clone()).unwrap();
    tap.admit(&[on(1, 1, 100, 43210)]);
    let (packets, _) = tap.render(64);
    push(&mut writer, &packets);
    writer.flush().unwrap();
    let first = f.rows();
    tap.admit(&[off(1, 1, 101, 23456)]);
    let (packets, _) = tap.render(64);
    push(&mut writer, &packets);
    writer.flush().unwrap();
    writer.close_interrupted().unwrap();
    drop(tap);
    (cfg, first)
}
fn write_payload(c: &Connection, ordinal: i64, header: &ChunkHeader, payload: &[u8]) -> String {
    let digest = header.sha256(payload).unwrap();
    c.execute(
        "UPDATE chunks SET payload=?1,sha256=?2 WHERE ordinal=?3",
        params![payload, digest, ordinal],
    )
    .unwrap();
    digest
}
fn rehash_tamper(f: &Fixture, cfg: &PerformanceConfig, mode: u8) {
    let c = Connection::open(f.path()).unwrap();
    c.execute_batch("DROP TRIGGER immutable_chunk_update;")
        .unwrap();
    let (cp_ordinal, cp_header, cp_payload): (i64, Vec<u8>, Vec<u8>) = c
        .query_row(
            "SELECT ordinal,header,payload FROM chunks ORDER BY ordinal DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let cp_header: ChunkHeader = serde_json::from_slice(&cp_header).unwrap();
    let records = raw_records(&cp_payload);
    assert_eq!(records.len(), 1);
    let (cp_envelope, mut cp_record) = decode_envelope(&records[0]).unwrap();
    if mode == 2 {
        let PerformanceRecord::Checkpoint { checkpoint } = &mut cp_record.record else {
            panic!("last row is checkpoint")
        };
        checkpoint.audio_end += 1;
        checkpoint.prefixes[0].source_end = SourcePosition::AudioFrames(checkpoint.audio_end);
        checkpoint.prefixes[0].recorded_units += 1;
    } else if mode != 3 {
        let stream = if mode == 0 { "gui-1" } else { "control" };
        let (ordinal,header,payload):(i64,Vec<u8>,Vec<u8>)=c.query_row("SELECT ordinal,header,payload FROM chunks WHERE stream_id=?1 AND ordinal<?2 ORDER BY ordinal DESC LIMIT 1",params![stream,cp_ordinal],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        let header: ChunkHeader = serde_json::from_slice(&header).unwrap();
        let mut records = raw_records(&payload);
        let (mut envelope, mut record) = decode_envelope(&records[0]).unwrap();
        if mode == 0 {
            let PerformanceRecord::AppliedEvent { original, .. } = &mut record.record else {
                panic!("GUI original")
            };
            let LiveEventKind::NoteOff {
                release_velocity16, ..
            } = &mut original.event
            else {
                panic!("second callback release")
            };
            *release_velocity16 ^= 1;
            records[0] = encode_record(
                cfg,
                1,
                &record,
                envelope.payload.time.observed.ticks,
                envelope.sequence,
            )
            .unwrap();
        } else if mode == 1 {
            envelope.session_id.0 = "synthetic-rehashed-wrong-session".into();
            records[0] = envelope.encode().unwrap();
        } else {
            assert_eq!(mode, 4);
            let batch_index = records
                .iter()
                .position(|bytes| {
                    matches!(
                        decode_envelope(bytes).unwrap().1.record,
                        PerformanceRecord::Batch { .. }
                    )
                })
                .unwrap();
            let (mut envelope, _) = decode_envelope(&records[batch_index]).unwrap();
            envelope.payload.time.observed.ticks += 1;
            records[batch_index] = envelope.encode().unwrap();
        }
        let payload =
            encode_event_chunk(&records.iter().map(Vec::as_slice).collect::<Vec<_>>()).unwrap();
        let digest = write_payload(&c, ordinal, &header, &payload);
        let PerformanceRecord::Checkpoint { checkpoint } = &mut cp_record.record else {
            panic!("checkpoint")
        };
        let member = checkpoint
            .chunks
            .iter_mut()
            .find(|m| m.stream_id == header.stream_id && m.sequence == header.sequence)
            .unwrap();
        member.sha256 = digest;
    }
    let encoded = encode_record(
        cfg,
        5,
        &cp_record,
        if let PerformanceRecord::Checkpoint { checkpoint } = &cp_record.record {
            checkpoint.audio_end
        } else {
            unreachable!()
        },
        cp_envelope.sequence,
    )
    .unwrap();
    // Keep the outer observation bound consistent too, so generic validation is
    // genuinely intact and the semantic scanner must detect the contradiction.
    let mut cp_header = cp_header;
    if mode == 2 {
        cp_header.observed_start.ticks += 1;
        cp_header.observed_end.ticks += 1;
    }
    if mode == 3 {
        cp_header.clock_anchors.push(ClockAnchor {
            id: AnchorId("synthetic-forbidden-performance-anchor".into()),
            host: cp_header.observed_start.clone(),
            audio: AudioFrame {
                epoch: cfg.output_epoch.clone(),
                frame: cp_header.observed_start.ticks as i64,
                sample_rate: cfg.info.sample_rate,
            },
            ticks_per_second_numerator: u64::from(cfg.info.sample_rate),
            ticks_per_second_denominator: 1,
            uncertainty_frames: 0,
            extensions: Default::default(),
        });
    }
    if mode == 2 || mode == 3 {
        c.execute(
            "UPDATE chunks SET header=?1 WHERE ordinal=?2",
            params![cp_header.canonical_bytes().unwrap(), cp_ordinal],
        )
        .unwrap();
    }
    write_payload(
        &c,
        cp_ordinal,
        &cp_header,
        &encode_event_chunk(&[&encoded]).unwrap(),
    );
}

#[test]
fn rehashed_link_session_extent_anchor_and_batch_time_cannot_authenticate_false_performance() {
    for mode in 0..5 {
        let f = Fixture::new();
        let (cfg, first) = two_groups(&f);
        rehash_tamper(&f, &cfg, mode);
        let reader = CaptureReader::open(f.path()).unwrap();
        assert!(
            reader.inspection().report.problems.is_empty(),
            "Tamper must preserve generic chunk integrity: mode{mode}"
        );
        let review = inspect_performance(&reader).unwrap();
        assert_eq!(
            review.selection.as_ref().unwrap().chunk_count,
            first,
            "Semantic contradiction mode{mode} must stop before second checkpoint"
        );
        assert!(review.problem.is_some());
        let originals = f.originals();
        let (manifest, _) = recover_performance(reader, &review, "semantic-prefix-only").unwrap();
        assert_eq!(manifest.body.chunk_count, first);
        assert_eq!(f.originals(), originals);
    }
}

#[test]
fn corrupt_final_checkpoint_never_recovers_the_valid_audio_before_it_as_complete() {
    let f = Fixture::new();
    let (_, first) = two_groups(&f);
    let c = Connection::open(f.path()).unwrap();
    c.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=(SELECT max(ordinal) FROM chunks);").unwrap();
    drop(c);
    let originals = f.originals();
    let reader = CaptureReader::open(f.path()).unwrap();
    assert!(reader.inspection().report.verified_chunks > first);
    let review = inspect_performance(&reader).unwrap();
    assert_eq!(review.selection.as_ref().unwrap().chunk_count, first);
    assert_eq!(review.checkpoint.as_ref().unwrap().audio_end, 64);
    let (manifest, _) =
        recover_performance(reader, &review, "retain-bad-final-checkpoint").unwrap();
    assert_eq!(manifest.body.chunk_count, first);
    assert_eq!(f.originals(), originals);
}

#[test]
fn stale_review_and_changed_original_tail_cannot_reuse_recovery_identity() {
    let f = Fixture::new();
    let (_, first) = two_groups(&f);
    let c = Connection::open(f.path()).unwrap();
    c.execute_batch("DROP TRIGGER immutable_chunk_update; UPDATE chunks SET payload=zeroblob(length(payload)) WHERE ordinal=(SELECT max(ordinal) FROM chunks);").unwrap();
    drop(c);
    let reader = CaptureReader::open(f.path()).unwrap();
    let expected = inspect_performance(&reader).unwrap();
    drop(reader);
    let c = Connection::open(f.path()).unwrap();
    c.execute(
        "UPDATE chunks SET payload=?1 WHERE ordinal=(SELECT max(ordinal) FROM chunks)",
        [vec![7u8; 4]],
    )
    .unwrap();
    drop(c);
    let before = fs::read(f.path()).unwrap();
    assert!(
        recover_performance(
            CaptureReader::open(f.path()).unwrap(),
            &expected,
            "old-review"
        )
        .is_err()
    );
    assert_eq!(fs::read(f.path()).unwrap(), before);
    let reader = CaptureReader::open(f.path()).unwrap();
    let current = inspect_performance(&reader).unwrap();
    assert_eq!(current.selection.as_ref().unwrap().chunk_count, first);
    let (manifest, _) = recover_performance(reader, &current, "current-review").unwrap();
    assert_eq!(manifest.body.chunk_count, first);
    let c = Connection::open(f.path()).unwrap();
    c.execute(
        "UPDATE chunks SET payload=?1 WHERE ordinal=(SELECT max(ordinal) FROM chunks)",
        [vec![8u8; 4]],
    )
    .unwrap();
    drop(c);
    let before = fs::read(f.path()).unwrap();
    assert!(
        recover_performance(
            CaptureReader::open(f.path()).unwrap(),
            &current,
            "current-review"
        )
        .is_err()
    );
    assert_eq!(fs::read(f.path()).unwrap(), before);
}

#[test]
fn changing_review_fields_cannot_keep_the_old_recovery_admission() {
    let f = Fixture::new();
    let (_, first) = two_groups(&f);
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    recover_performance(reader, &review, "same-caller-operation").unwrap();
    for mode in 0..3 {
        let mut changed = review.clone();
        if mode == 0 {
            changed.problem = Some("different reviewed request".into());
        } else if mode == 1 {
            changed.selection.as_mut().unwrap().chunk_count = first;
        } else {
            changed.journal_sha256_at_review = "0".repeat(64);
        }
        assert!(
            recover_performance(
                CaptureReader::open(f.path()).unwrap(),
                &changed,
                "same-caller-operation"
            )
            .is_err()
        );
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn writer_whole_group_rollback_and_lost_ack_retry_never_resume_admission() {
    for after_commit in [false, true] {
        let f = Fixture::new();
        let cfg = config(2, 48_000);
        let mut tap = Tap::new(&cfg);
        let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
        tap.admit(&[on(1, 1, 10, 65535)]);
        let (packets, _) = tap.render(32);
        push(&mut writer, &packets);
        let mut inserts = 0;
        writer.set_boundary_hook(move |boundary| {
            if boundary == omniusong_recording::WriteBoundary::AfterChunkInsert {
                inserts += 1;
            }
            if (after_commit && boundary == omniusong_recording::WriteBoundary::AfterChunkCommit)
                || (!after_commit
                    && boundary == omniusong_recording::WriteBoundary::AfterChunkInsert
                    && inserts == 2)
            {
                return Err(RecordingError::Invalid(
                    "Independent injected writer boundary".into(),
                ));
            }
            Ok(())
        });
        assert!(writer.flush().is_err());
        let rows = f.rows();
        assert_eq!(rows > 0, after_commit);
        assert_eq!(writer.progress().durable_frames, 0);
        let error = writer.progress().first_error.unwrap();
        writer.clear_boundary_hook();
        let replay = writer.retry_pending().unwrap();
        assert_eq!(replay.durable_frames, 32);
        assert_eq!(replay.first_error, Some(error));
        let committed = f.rows();
        assert!(committed > 0);
        if after_commit {
            assert_eq!(committed, rows);
        }
        let (packets, _) = tap.render(1);
        assert!(writer.push_packet(packets[0]).is_err());
        assert_eq!(f.rows(), committed);
        drop(writer);
        drop(tap);
        let reader = CaptureReader::open(f.path()).unwrap();
        let review = inspect_performance(&reader).unwrap();
        assert_eq!(review.checkpoint.unwrap().audio_end, 32);
    }
}

#[test]
fn incremental_real_tap_exceeds_legacy_byte_and_event_caps_without_losing_originals() {
    let f = Fixture::new();
    let cfg = config(8, 48_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
    let mut rendered = Sha256::new();
    for callback in 0..65u64 {
        for source in 1..=4 {
            let events: Vec<_> = (0..32u64)
                .map(|index| {
                    sustain(
                        source,
                        9_007_199_254_740_993 + callback * 32 + index,
                        index % 2 == 0,
                    )
                })
                .collect();
            tap.admit(&events);
        }
        let (packets, pcm) = tap.render(16_384);
        rendered.update(&pcm);
        push(&mut writer, &packets);
        let progress = writer.progress();
        assert!(progress.first_error.is_none());
        assert!(progress.capture_fault.is_none());
        assert_eq!(progress.durable_frames, (callback + 1) * 16_384);
    }
    let retained = writer.close_interrupted().unwrap();
    drop(tap);
    assert_eq!(retained.progress.durable_frames, 65 * 16_384);
    assert!(fs::metadata(f.path()).unwrap().len() > 30 * 1024 * 1024);
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert!(review.problem.is_none());
    let cp = review.checkpoint.unwrap();
    assert_eq!(cp.prefixes[0].recorded_units, 65 * 16_384);
    assert_eq!(
        cp.prefixes[1..5]
            .iter()
            .map(|v| v.recorded_units)
            .sum::<u64>(),
        8_320
    );
    let mut stored = Sha256::new();
    let mut audio_bytes = 0usize;
    for ordinal in 1..=reader.inspection().report.verified_chunks {
        let chunk = reader.read_ordinal(ordinal).unwrap();
        if chunk.header.stream_id == "audio" {
            audio_bytes += chunk.payload.len();
            stored.update(&chunk.payload);
        }
    }
    assert!(audio_bytes > 30 * 1024 * 1024);
    assert_eq!(rendered.finalize(), stored.finalize());
}

#[test]
fn forged_known_fault_clear_without_its_end_cause_is_not_a_complete_callback() {
    let f = Fixture::new();
    let cfg = config(1, 48_000);
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
    tap.admission.control().fault(LiveFaultKind::Device, None);
    let (mut packets, _) = tap.render(64);
    let PerformancePacketKind::End(end) = &mut packets.last_mut().unwrap().kind else {
        panic!("actual tap End")
    };
    assert!(end.clears.iter().flatten().any(|clear| matches!(
        clear.reason,
        omniusong_engine::performance_tap::TapClearReason::Fault {
            kind: LiveFaultKind::Device
        }
    )));
    assert_eq!(end.first_fault.unwrap().kind, LiveFaultKind::Device);
    // Corrupt one field of an actual tap callback. This is an invalid journal
    // input test, not simulated evidence of native quiescence or a device race.
    end.first_fault = None;
    push(&mut writer, &packets[..packets.len() - 1]);
    assert!(writer.push_packet(*packets.last().unwrap()).is_err());
    assert!(writer.progress().first_error.is_some());
    assert_eq!(writer.progress().complete_callbacks, 0);
    assert_eq!(f.rows(), 0);
}

#[test]
fn forged_application_after_retained_sticky_capture_fault_cannot_resume_originals() {
    let f = Fixture::new();
    let cfg = config(1, 48_000);
    let scope = cfg.info.scope;
    let mut tap = Tap::new(&cfg);
    let mut writer = PerformanceWriter::create(&f.path(), cfg).unwrap();
    tap.admission.control().fault(LiveFaultKind::Device, None);
    let (packets, _) = tap.render(64);
    push(&mut writer, &packets);
    writer.flush().unwrap();
    assert_eq!(
        writer.progress().capture_fault.unwrap().kind,
        LiveFaultKind::Device
    );
    let committed_rows = f.rows();
    let (mut next, _) = tap.render(64);
    let mut forged = LiveBatch::empty(scope, 1, None);
    forged.try_push(sustain(1, 100, true)).unwrap();
    let callback_index = next[0].callback_index;
    next.insert(
        1,
        PerformancePacket {
            callback_index,
            ordinal: 1,
            kind: PerformancePacketKind::Batch {
                batch: forged,
                disposition: omniusong_engine::performance_tap::BatchDisposition::Applied,
            },
        },
    );
    for (ordinal, packet) in next.iter_mut().enumerate() {
        packet.ordinal = u16::try_from(ordinal).unwrap();
    }
    let PerformancePacketKind::End(end) = &mut next.last_mut().unwrap().kind else {
        panic!("actual subsequent End")
    };
    end.applied_sequence = Some(1);
    push(&mut writer, &next[..next.len() - 1]);
    assert!(writer.push_packet(*next.last().unwrap()).is_err());
    assert_eq!(writer.progress().complete_callbacks, 1);
    assert_eq!(writer.progress().durable_frames, 64);
    assert_eq!(f.rows(), committed_rows);
    drop(writer);
    drop(tap);
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert_eq!(review.checkpoint.unwrap().audio_end, 64);
    assert!(
        !capture_records(&reader)
            .iter()
            .any(|(_, _, record)| matches!(record.record, PerformanceRecord::AppliedEvent { .. }))
    );
}

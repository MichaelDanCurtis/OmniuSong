//! Independent output-free Finish fence, durable v2 and recovery regressions.
//! No native device or normal-seal authority is created by these fixtures.
use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
use omniusong_engine::{
    live::{LiveAdmission, LiveRenderer, recording_live_pair},
    performance_records::{
        PerformanceConfig, PerformanceRecord, PerformanceSource, decode_envelope, encode_record,
    },
    performance_tap::{
        PerformancePacket, PerformancePacketKind, PerformanceTapOwner, TapFinish, TapInfo,
    },
    performance_writer::{PerformanceWriter, inspect_performance, recover_performance},
    *,
};
use omniusong_recording::{CaptureReader, ChunkHeader, Journal, SealedJournal, encode_event_chunk};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    fs,
    path::PathBuf,
};

struct CallbackAllocator;
thread_local! {
    static CALLBACK_ALLOCATIONS: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
fn count_allocations(allocations: usize, deallocations: usize) {
    let _ = CALLBACK_ALLOCATIONS.try_with(|state| {
        let (enabled, a, d) = state.get();
        if enabled {
            state.set((true, a + allocations, d + deallocations));
        }
    });
}
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocations(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocations(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count_allocations(0, 1);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_allocations(1, 1);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

fn measured_callback(work: impl FnOnce()) -> (usize, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            CALLBACK_ALLOCATIONS.with(|state| {
                let (_, a, d) = state.get();
                state.set((false, a, d));
            });
        }
    }
    CALLBACK_ALLOCATIONS.with(|state| state.set((true, 0, 0)));
    let guard = Reset;
    work();
    drop(guard);
    CALLBACK_ALLOCATIONS.with(|state| {
        let (_, a, d) = state.replace((false, 0, 0));
        (a, d)
    })
}

type OriginalRow = (i64, String, i64, Vec<u8>, Vec<u8>, String);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-finish-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("take.sqlite3")
    }
    fn originals(&self) -> Vec<OriginalRow> {
        let c = Connection::open_with_flags(self.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut s = c.prepare("SELECT ordinal,stream_id,sequence,header,payload,sha256 FROM chunks ORDER BY ordinal").unwrap();
        s.query_map([], |r| {
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
fn config() -> PerformanceConfig {
    PerformanceConfig {
        take_id: TakeId(uuid::Uuid::new_v4().to_string()),
        info: TapInfo {
            scope: LiveScope {
                worker_generation: LiveId([17; 16]),
                session_id: LiveId([29; 16]),
                panic_generation: 1,
            },
            sample_rate: 48_000,
            channels: 2,
        },
        gui_clock: LiveClock {
            domain: "finish-review.gui".into(),
            epoch: "finish-review.gui-epoch".into(),
            ticks_per_second_numerator: 1_000_000_000_000,
            ticks_per_second_denominator: 1000,
        },
        output_epoch: ClockEpochId("finish-review.output".into()),
        sources: std::array::from_fn(|i| PerformanceSource {
            binding: LiveSourceBinding {
                kind: [
                    LiveSourceKind::ComputerKeys,
                    LiveSourceKind::PointerKeys,
                    LiveSourceKind::Pads,
                    LiveSourceKind::AccessiblePulse,
                ][i],
                handle: i as u8 + 1,
                connection_generation: 1,
            },
            endpoint: EndpointId(format!("finish-review.source-{}", i + 1)),
        }),
    }
}
struct Tap {
    admission: LiveAdmission,
    renderer: Option<LiveRenderer>,
    owner: PerformanceTapOwner,
}
impl Tap {
    fn new(c: &PerformanceConfig) -> Self {
        let (admission, renderer, owner) =
            recording_live_pair(c.info.scope, c.info.sample_rate, c.info.channels).unwrap();
        Self {
            admission,
            renderer: Some(renderer),
            owner,
        }
    }
    fn admit(&mut self, events: &[LiveEvent]) -> LiveBatch {
        let p = self.admission.progress();
        let mut batch = LiveBatch::empty(
            p.scope,
            p.next_sequence().unwrap(),
            self.admission.lease_credit().map(|c| c.token),
        );
        for e in events {
            batch.try_push(*e).unwrap();
        }
        self.admission.admit(batch).unwrap();
        batch
    }
    fn render(&mut self, frames: usize) -> (Vec<PerformancePacket>, Vec<u8>) {
        let mut out = vec![0.0; frames * 2];
        self.renderer.as_mut().unwrap().render_into(&mut out);
        let mut packets = Vec::new();
        while let Some(p) = self.owner.try_pop().unwrap() {
            packets.push(p);
        }
        (
            packets,
            out.into_iter().flat_map(f32::to_le_bytes).collect(),
        )
    }
}
impl Drop for Tap {
    fn drop(&mut self) {
        drop(self.renderer.take());
    }
}
fn on(voice: u64, ticks: u64) -> LiveEvent {
    LiveEvent {
        source: 1,
        connection_generation: 1,
        observed_ticks: ticks,
        event: LiveEventKind::NoteOn {
            note: voice,
            voice,
            key: 60,
            velocity16: u16::MAX,
            patch: LivePatch::Keys,
        },
    }
}
fn sustain(ticks: u64, down: bool) -> LiveEvent {
    LiveEvent {
        source: 1,
        connection_generation: 1,
        observed_ticks: ticks,
        event: LiveEventKind::Sustain { down },
    }
}
fn push(w: &mut PerformanceWriter, packets: &[PerformancePacket]) {
    for p in packets {
        w.push_packet(*p).unwrap();
    }
}
fn cut(packets: &[PerformancePacket]) -> TapFinish {
    let found: Vec<_> = packets
        .iter()
        .filter_map(|p| match p.kind {
            PerformancePacketKind::Finish(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1);
    found[0]
}
fn raw(payload: &[u8]) -> Vec<Vec<u8>> {
    let mut rest = payload;
    let mut result = Vec::new();
    while !rest.is_empty() {
        assert!(rest.len() >= 4);
        let n = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        assert!(n > 0 && n <= rest.len() - 4);
        result.push(rest[4..4 + n].to_vec());
        rest = &rest[4 + n..];
    }
    result
}
fn prefix_and_final(
    f: &Fixture,
) -> (
    PerformanceConfig,
    PerformanceWriter,
    Tap,
    Vec<PerformancePacket>,
) {
    let c = config();
    let mut w = PerformanceWriter::create_finishable(&f.path(), c.clone()).unwrap();
    let mut tap = Tap::new(&c);
    tap.admit(&[on(1, u64::MAX - 1)]);
    let (first, _) = tap.render(32);
    push(&mut w, &first);
    w.flush().unwrap();
    tap.admission
        .begin_finish(c.info.scope, LiveId([43; 16]))
        .unwrap();
    let (last, _) = tap.render(64);
    (c, w, tap, last)
}

#[test]
fn sixteen_worker_accepted_batches_keep_exact_originals_and_one_terminal_pcm_cut() {
    let f = Fixture::new();
    let c = config();
    let mut tap = Tap::new(&c);
    let mut w = PerformanceWriter::create_finishable(&f.path(), c.clone()).unwrap();
    let mut originals = Vec::new();
    for i in 0..16 {
        originals.push(tap.admit(&[if i == 0 {
            on(1, (1u64 << 53) + 1)
        } else {
            sustain((1u64 << 53) + i + 1, true)
        }]));
    }
    let id = LiveId([41; 16]);
    let fence = tap.admission.begin_finish(c.info.scope, id).unwrap();
    assert_eq!(fence.accepted_sequence, Some(16));
    assert_eq!(tap.admission.begin_finish(c.info.scope, id).unwrap(), fence);
    assert_eq!(tap.admission.finish_ack().unwrap(), None);
    assert_eq!(tap.admission.progress().state, LiveState::Finishing);
    assert!(tap.admission.progress().is_valid());
    assert!(
        tap.admission
            .begin_finish(c.info.scope, LiveId([42; 16]))
            .is_err()
    );
    assert!(
        tap.admission
            .admit(LiveBatch::empty(c.info.scope, 17, None))
            .is_err()
    );
    assert!(tap.admission.begin_panic(c.info.scope).is_err());
    let mut rendered = Sha256::new();
    let mut actual = Vec::new();
    let mut final_cut = None;
    for invocation in 0..4 {
        let (packets, pcm) = tap.render(257);
        rendered.update(pcm);
        let batches: Vec<_> = packets
            .iter()
            .filter_map(|p| match p.kind {
                PerformancePacketKind::Batch { batch, disposition } => Some((batch, disposition)),
                _ => None,
            })
            .collect();
        assert_eq!(batches.len(), 4);
        actual.extend(batches.into_iter().map(|(batch, disposition)| {
            assert_eq!(
                disposition,
                omniusong_engine::performance_tap::BatchDisposition::Applied
            );
            batch
        }));
        if invocation < 3 {
            assert!(
                !packets
                    .iter()
                    .any(|p| matches!(p.kind, PerformancePacketKind::Finish(_)))
            );
            assert_eq!(tap.admission.finish_ack().unwrap(), None);
            assert_eq!(tap.admission.progress().state, LiveState::Finishing);
            assert!(tap.admission.progress().is_valid());
        } else {
            let finish = cut(&packets);
            assert_eq!(finish.fence, fence);
            assert_eq!(finish.cut_frame, 1028);
            assert_eq!(finish.voices_cut, 1);
            assert_eq!(finish.release_tails_cut, 0);
            assert!(matches!(
                packets[packets.len() - 2].kind,
                PerformancePacketKind::Finish(_)
            ));
            assert!(
                matches!(packets.last().unwrap().kind,PerformancePacketKind::End(end) if end.active_voices==0&&end.release_tail_voices==0&&end.applied_sequence==Some(16))
            );
            assert_eq!(tap.admission.finish_ack().unwrap(), Some(finish));
            assert_eq!(tap.admission.progress().state, LiveState::FinishedCut);
            assert!(tap.admission.progress().is_valid());
            final_cut = Some(finish);
        }
        push(&mut w, &packets);
    }
    assert_eq!(actual, originals);
    assert_eq!(w.progress().finish_cut, final_cut);
    assert_eq!(w.progress().durable_frames, 1028);
    let frames = tap.admission.control().frames();
    let (after, silent) = tap.render(2049);
    assert!(after.is_empty());
    assert!(silent.iter().all(|b| *b == 0));
    assert_eq!(tap.admission.control().frames(), frames);
    drop(tap);
    let retained = w.close_interrupted().unwrap();
    assert!(retained.unknown_tail);
    let originals = f.originals();
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert_eq!(review.finish_cut, final_cut);
    assert!(review.unknown_tail);
    assert!(review.problem.is_none());
    let mut stored = Sha256::new();
    let mut events = 0;
    let mut offs = 0;
    let mut finishes = 0;
    for row in &originals {
        if row.1 == "audio" {
            stored.update(&row.4);
            continue;
        }
        for bytes in raw(&row.4) {
            let (envelope, r) = decode_envelope(&bytes).unwrap();
            assert_eq!(r.version, 2);
            assert!(
                envelope
                    .payload
                    .extensions
                    .contains_key("omniusong.performance.v2")
            );
            assert!(
                !envelope
                    .payload
                    .extensions
                    .contains_key("omniusong.performance.v1")
            );
            match r.record {
                PerformanceRecord::AppliedEvent { original, .. } => {
                    events += 1;
                    assert!(original.observed_ticks > (1u64 << 53));
                    offs += usize::from(matches!(original.event, LiveEventKind::NoteOff { .. }));
                }
                PerformanceRecord::Finish { finish, .. } => {
                    assert_eq!(Some(finish), final_cut);
                    finishes += 1;
                }
                _ => (),
            }
        }
    }
    assert_eq!(rendered.finalize(), stored.finalize());
    assert_eq!((events, offs, finishes), (16, 0, 1));
    let (manifest, replayed) =
        recover_performance(reader, &review, "finish-review-recovery").unwrap();
    assert!(!replayed);
    assert!(
        manifest
            .body
            .interrupted
            .as_ref()
            .unwrap()
            .report
            .unknown_tail
    );
    assert_eq!(f.originals(), originals);
    assert!(
        recover_performance(
            CaptureReader::open(f.path()).unwrap(),
            &review,
            "finish-review-recovery"
        )
        .unwrap()
        .1
    );
    assert_eq!(SealedJournal::open(f.path()).unwrap().manifest(), &manifest);
}

#[test]
fn empty_finish_is_not_sequence_zero_and_is_never_normal_seal_authority() {
    let f = Fixture::new();
    let c = config();
    let mut tap = Tap::new(&c);
    let mut w = PerformanceWriter::create_finishable(&f.path(), c.clone()).unwrap();
    let fence = tap
        .admission
        .begin_finish(c.info.scope, LiveId([44; 16]))
        .unwrap();
    assert_eq!(fence.accepted_sequence, None);
    let (packets, pcm) = tap.render(1);
    assert!(pcm.iter().all(|b| *b == 0));
    let finish = cut(&packets);
    assert_eq!((finish.cut_frame, finish.voices_cut), (1, 0));
    push(&mut w, &packets);
    drop(tap);
    w.close_interrupted().unwrap();
    let before = fs::read(f.path()).unwrap();
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert_eq!(review.finish_cut, Some(finish));
    assert!(reader.recover("generic-must-refuse").is_err());
    assert_eq!(fs::read(f.path()).unwrap(), before);
    let mut journal = Journal::open(f.path()).unwrap();
    assert!(journal.seal("not-a-worker-authority").is_err());
    assert!(journal.seal_recovered("not-format-aware").is_err());
}

#[test]
fn missing_terminal_end_retains_finish_original_but_only_the_prior_checkpoint_is_recovered() {
    let f = Fixture::new();
    let (_c, mut w, tap, last) = prefix_and_final(&f);
    let finish = cut(&last);
    push(&mut w, &last[..last.len() - 1]);
    drop(tap);
    w.close_interrupted().unwrap();
    let originals = f.originals();
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert!(review.finish_cut.is_none());
    assert_eq!(review.checkpoint.as_ref().unwrap().audio_end, 32);
    assert!(review.problem.is_some());
    assert!(originals.iter().filter(|r|r.1=="control").flat_map(|r|raw(&r.4)).any(|b|matches!(decode_envelope(&b).unwrap().1.record,PerformanceRecord::Finish { finish:f,.. } if f==finish)));
    let (manifest, _) = recover_performance(reader, &review, "recover-preterminal-prefix").unwrap();
    assert_eq!(manifest.body.streams[0].recorded_units, 32);
    assert_eq!(f.originals(), originals);
}

#[test]
fn forged_terminal_packet_order_fault_and_fence_cannot_commit_a_new_complete_prefix() {
    for mode in 0..5 {
        let f = Fixture::new();
        let (c, mut w, tap, mut last) = prefix_and_final(&f);
        let before = f.originals();
        let finish_index = last.len() - 2;
        match mode {
            0 => {
                last.swap(1, finish_index);
                for (i, p) in last.iter_mut().enumerate() {
                    p.ordinal = i as u16;
                }
            }
            1 => {
                last.insert(finish_index + 1, last[finish_index]);
                for (i, p) in last.iter_mut().enumerate() {
                    p.ordinal = i as u16;
                }
            }
            2 => {
                if let PerformancePacketKind::End(ref mut end) = last.last_mut().unwrap().kind {
                    end.first_fault = Some(LiveFault {
                        scope: c.info.scope,
                        kind: LiveFaultKind::Device,
                        sequence: None,
                        frame: 96,
                    });
                }
            }
            3 => {
                if let PerformancePacketKind::Finish(ref mut finish) = last[finish_index].kind {
                    finish.fence.accepted_sequence = Some(2);
                }
            }
            _ => {
                if let PerformancePacketKind::Finish(ref mut finish) = last[finish_index].kind {
                    finish.fence.scope.panic_generation += 1;
                }
            }
        }
        let mut rejected = false;
        for packet in last {
            if w.push_packet(packet).is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected, "mode {mode}");
        assert_eq!(w.progress().complete_frames, 32);
        assert_eq!(f.originals(), before);
        drop(tap);
        drop(w);
        let reader = CaptureReader::open(f.path()).unwrap();
        let review = inspect_performance(&reader).unwrap();
        assert_eq!(review.checkpoint.unwrap().audio_end, 32);
        assert!(review.finish_cut.is_none());
    }
}

#[test]
fn completed_v2_writer_refuses_post_cut_callbacks_and_v1_refuses_the_new_terminal_packet() {
    let f = Fixture::new();
    let (c, mut w, tap, last) = prefix_and_final(&f);
    push(&mut w, &last);
    let rows = f.originals();
    let mut begin = last[0];
    begin.callback_index += 1;
    if let PerformancePacketKind::Begin(ref mut value) = begin.kind {
        value.start_frame = 96;
    }
    assert!(w.push_packet(begin).is_err());
    assert_eq!(f.originals(), rows);
    drop(tap);
    drop(w);
    let f = Fixture::new();
    let mut old = PerformanceWriter::create(&f.path(), c.clone()).unwrap();
    let mut tap = Tap::new(&c);
    tap.admission
        .begin_finish(c.info.scope, LiveId([45; 16]))
        .unwrap();
    let (last, _) = tap.render(64);
    let mut rejected = false;
    for p in last {
        if old.push_packet(p).is_err() {
            rejected = true;
            break;
        }
    }
    assert!(rejected);
    assert_eq!(old.progress().complete_frames, 0);
    assert!(f.originals().is_empty());
}

fn replace_payload(c: &Connection, ordinal: i64, header: &ChunkHeader, payload: &[u8]) -> String {
    let sha = header.sha256(payload).unwrap();
    c.execute(
        "UPDATE chunks SET payload=?1,sha256=?2 WHERE ordinal=?3",
        params![payload, sha, ordinal],
    )
    .unwrap();
    sha
}
fn tamper_terminal(f: &Fixture, cfg: &PerformanceConfig, mode: u8) {
    let db = Connection::open(f.path()).unwrap();
    db.execute_batch("DROP TRIGGER immutable_chunk_update;")
        .unwrap();
    let (cp_ordinal, cp_header, cp_payload): (i64, Vec<u8>, Vec<u8>) = db
        .query_row(
            "SELECT ordinal,header,payload FROM chunks ORDER BY ordinal DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let cp_header: ChunkHeader = serde_json::from_slice(&cp_header).unwrap();
    let (cp_envelope, mut cp_record) = decode_envelope(&raw(&cp_payload)[0]).unwrap();
    let (ordinal,header,payload):(i64,Vec<u8>,Vec<u8>)=db.query_row("SELECT ordinal,header,payload FROM chunks WHERE stream_id='control' AND ordinal<?1 ORDER BY ordinal DESC LIMIT 1",[cp_ordinal],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    let header: ChunkHeader = serde_json::from_slice(&header).unwrap();
    let mut records = raw(&payload);
    let index = records
        .iter()
        .position(|b| {
            matches!(
                decode_envelope(b).unwrap().1.record,
                PerformanceRecord::Finish { .. }
            )
        })
        .unwrap();
    let (mut envelope, mut record) = decode_envelope(&records[index]).unwrap();
    let PerformanceRecord::Finish {
        callback_index,
        finish,
    } = &mut record.record
    else {
        unreachable!()
    };
    match mode {
        0 => finish.fence.accepted_sequence = Some(55),
        1 => finish.cut_frame -= 1,
        2 => finish.fence.scope.panic_generation += 1,
        _ => *callback_index += 1,
    };
    let observed = finish.cut_frame;
    envelope.payload.time.observed.ticks = observed;
    envelope.payload.extensions.insert(
        "omniusong.performance.v2".into(),
        serde_json::to_value(&record).unwrap(),
    );
    // Deliberately bypass owned semantic validation while retaining a valid
    // generic capture envelope, exact chunk checksum and checkpoint link.
    records[index] = envelope.encode().unwrap();
    let bytes = encode_event_chunk(&records.iter().map(Vec::as_slice).collect::<Vec<_>>()).unwrap();
    let sha = replace_payload(&db, ordinal, &header, &bytes);
    let PerformanceRecord::Checkpoint { checkpoint } = &mut cp_record.record else {
        unreachable!()
    };
    checkpoint
        .chunks
        .iter_mut()
        .find(|m| m.stream_id == "control" && m.sequence == header.sequence)
        .unwrap()
        .sha256 = sha;
    let at = checkpoint.audio_end;
    let encoded = encode_record(cfg, 5, &cp_record, at, cp_envelope.sequence).unwrap();
    replace_payload(
        &db,
        cp_ordinal,
        &cp_header,
        &encode_event_chunk(&[&encoded]).unwrap(),
    );
}

#[test]
fn self_consistent_rehashed_finish_contradictions_recover_only_the_prior_checkpoint() {
    for mode in 0..4 {
        let f = Fixture::new();
        let (c, mut w, tap, last) = prefix_and_final(&f);
        push(&mut w, &last);
        drop(tap);
        w.close_interrupted().unwrap();
        tamper_terminal(&f, &c, mode);
        let originals = f.originals();
        let reader = CaptureReader::open(f.path()).unwrap();
        assert!(
            reader.inspection().report.problems.is_empty(),
            "generic validity mode {mode}"
        );
        let review = inspect_performance(&reader).unwrap();
        assert!(review.problem.is_some());
        assert!(review.finish_cut.is_none());
        assert_eq!(review.checkpoint.as_ref().unwrap().audio_end, 32);
        let (manifest, _) =
            recover_performance(reader, &review, "recover-before-forged-finish").unwrap();
        assert_eq!(manifest.body.streams[0].recorded_units, 32);
        assert_eq!(f.originals(), originals);
    }
}

#[test]
fn fault_after_callback_cut_does_not_retroactively_supply_normal_seal_authority() {
    let f = Fixture::new();
    let (_c, mut w, tap, last) = prefix_and_final(&f);
    let finish = cut(&last);
    push(&mut w, &last);
    assert_eq!(tap.admission.finish_ack().unwrap(), Some(finish));
    tap.admission.control().fault(LiveFaultKind::Device, None);
    assert_eq!(
        tap.admission.control().first_fault().unwrap().kind,
        LiveFaultKind::Device
    );
    assert_eq!(tap.admission.finish_ack(), Err(LiveSubmitError::Faulted));
    assert_eq!(tap.admission.progress().state, LiveState::Faulted);
    drop(tap);
    let retained = w.close_interrupted().unwrap();
    assert!(retained.unknown_tail);
    assert_eq!(retained.progress.finish_cut, Some(finish));
    let reader = CaptureReader::open(f.path()).unwrap();
    let review = inspect_performance(&reader).unwrap();
    assert!(review.unknown_tail);
    assert_eq!(review.finish_cut, Some(finish));
    let (manifest, _) =
        recover_performance(reader, &review, "late-fault-is-recovery-only").unwrap();
    assert!(manifest.body.interrupted.is_some());
}

#[test]
fn maximal_final_finish_and_following_silence_allocate_and_deallocate_nothing() {
    let c = config();
    let (mut admission, mut renderer, mut owner) =
        recording_live_pair(c.info.scope, 48_000, 8).unwrap();
    // All 64 held voices and all four batch slots are prepared outside the
    // measured callback, as are both output buffers and the immutable request.
    for sequence in 1..=4 {
        let mut batch = LiveBatch::empty(c.info.scope, sequence, None);
        if sequence <= 2 {
            for offset in 1..=32 {
                let voice = (sequence - 1) * 32 + offset;
                batch.try_push(on(voice, voice)).unwrap();
            }
        } else {
            batch.try_push(sustain(64 + sequence, true)).unwrap();
        }
        admission.admit(batch).unwrap();
    }
    let fence = admission
        .begin_finish(c.info.scope, LiveId([49; 16]))
        .unwrap();
    let mut final_output = vec![0.0; LIVE_MAX_CALLBACK_FRAMES * 8];
    let mut silent_output = vec![1.0; 2049 * 8];
    assert_eq!(
        measured_callback(|| renderer.render_into(&mut final_output)),
        (0, 0)
    );
    assert!(final_output.iter().any(|value| *value != 0.0));
    let finish = admission.finish_ack().unwrap().unwrap();
    assert_eq!(finish.fence, fence);
    assert_eq!(
        (
            finish.cut_frame,
            finish.voices_cut,
            finish.release_tails_cut
        ),
        (LIVE_MAX_CALLBACK_FRAMES as u64, 64, 0)
    );
    let mut packets = Vec::new();
    while let Some(packet) = owner.try_pop().unwrap() {
        packets.push(packet);
    }
    assert_eq!(packets.len(), 71);
    assert_eq!(packets[69].kind, PerformancePacketKind::Finish(finish));
    assert!(
        matches!(packets[70].kind,PerformancePacketKind::End(end) if end.active_voices==0&&end.first_fault.is_none())
    );
    assert_eq!(
        measured_callback(|| renderer.render_into(&mut silent_output)),
        (0, 0)
    );
    assert!(silent_output.iter().all(|value| value.to_bits() == 0));
    assert!(owner.try_pop().unwrap().is_none());
    assert_eq!(admission.finish_ack().unwrap(), Some(finish));
    assert_eq!(admission.progress().frames, finish.cut_frame);
    drop(renderer);
}

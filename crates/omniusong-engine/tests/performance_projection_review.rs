//! Actual device-free renderer/writer evidence, authenticated recovered sealing,
//! and public bounded projection. No native normal-seal permit is manufactured.
use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
use omniusong_engine::{
    LiveBatch, LiveClock, LiveEvent, LiveEventKind, LiveId, LivePatch, LiveScope,
    LiveSourceBinding, LiveSourceKind,
    live::{LiveAdmission, LiveRenderer, recording_live_pair},
    performance_records::{PerformanceConfig, PerformanceSource},
    performance_tap::{
        PerformancePacket, PerformancePacketKind, PerformanceTapOwner, TapClearReason, TapInfo,
        TapTransitionKind,
    },
    performance_writer::{
        PerformanceWriter, inspect_performance, inspect_sealed_performance, recover_performance,
    },
};
use omniusong_performance::projection::*;
use omniusong_recording::{
    CaptureReader, InspectionBudget, InspectionLimits, InspectionResource, RecordingError,
    SealedJournal,
};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    config: PerformanceConfig,
}
impl Fixture {
    fn record(v2: bool, work: impl FnOnce(&mut Tap, &mut PerformanceWriter)) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/performance-projection-review")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let path = root.join("take.sqlite3");
        let config = PerformanceConfig {
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            info: TapInfo {
                scope: LiveScope {
                    worker_generation: LiveId([51; 16]),
                    session_id: LiveId([52; 16]),
                    panic_generation: 1,
                },
                sample_rate: 8000,
                channels: 1,
            },
            gui_clock: LiveClock {
                domain: "projection-review.gui".into(),
                epoch: "projection-review.gui-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
            output_epoch: ClockEpochId("projection-review.render-epoch".into()),
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
                endpoint: EndpointId(format!("projection-review.source-{}", i + 1)),
            }),
        };
        let mut writer = if v2 {
            PerformanceWriter::create_finishable(&path, config.clone())
        } else {
            PerformanceWriter::create(&path, config.clone())
        }
        .unwrap();
        let mut tap = Tap::new(&config);
        work(&mut tap, &mut writer);
        drop(tap);
        writer.close_interrupted().unwrap();
        Self { root, path, config }
    }
    fn recover(&self) {
        let reader = CaptureReader::open(&self.path).unwrap();
        let review = inspect_performance(&reader).unwrap();
        assert!(review.selection.is_some());
        assert!(
            review.problem.is_none(),
            "real fixture must have a complete checked prefix: {:?}",
            review.problem
        );
        recover_performance(reader, &review, "projection-review.recovered-only").unwrap();
    }
    fn bytes(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fs::read_dir(&self.root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.strip_prefix(&self.root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                )
            })
            .collect()
    }
    fn request(&self) -> PerformanceProjectionRequest {
        let reader = SealedJournal::open(&self.path).unwrap();
        let review = inspect_sealed_performance(&reader).unwrap();
        assert!(
            reader.manifest().body.interrupted.is_some(),
            "software evidence stays explicitly recovered"
        );
        PerformanceProjectionRequest {
            version: 1,
            source: PerformanceProjectionSourcePin {
                take_id: self.config.take_id.0.clone(),
                manifest_sha256: reader.manifest().sha256.clone(),
                descriptor_sha256: review.descriptor_sha256,
                original_rows_sha256: review.original_rows_sha256,
                journal_sha256_at_review: review.journal_sha256_at_review,
                checkpoint_sha256: review.checkpoint_sha256.unwrap(),
                selection: review.selection.unwrap(),
            },
            region: PerformanceFrameRegion {
                start: 0,
                end: review.checkpoint.unwrap().audio_end,
            },
            sources: vec![1, 2, 3, 4],
            tempo_bits: 120.0f64.to_bits(),
            destination_start_tick: 1000,
            rounding: PerformanceProjectionRounding::NearestTiesUp,
            duration: PerformanceProjectionDuration::ReleaseGate,
            collapsed: PerformanceProjectionPolicy::Reject,
            incomplete: PerformanceProjectionPolicy::Reject,
            region_crossing: PerformanceProjectionPolicy::Reject,
        }
    }
    fn project(
        &self,
        request: &PerformanceProjectionRequest,
    ) -> omniusong_recording::Result<PerformanceProjectionReview> {
        let (budget, _) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&self.path, &budget)?;
        project_sealed_performance_bounded(&reader, request)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Projection fixture retained: {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct Tap {
    admission: LiveAdmission,
    renderer: Option<LiveRenderer>,
    owner: PerformanceTapOwner,
}
impl Tap {
    fn new(config: &PerformanceConfig) -> Self {
        let (admission, renderer, owner) = recording_live_pair(config.info.scope, 8000, 1).unwrap();
        Self {
            admission,
            renderer: Some(renderer),
            owner,
        }
    }
    fn admit(&mut self, events: &[LiveEvent]) {
        let progress = self.admission.progress();
        let mut batch = LiveBatch::empty(
            progress.scope,
            progress.next_sequence().unwrap(),
            self.admission.lease_credit().map(|c| c.token),
        );
        for event in events {
            batch.try_push(*event).unwrap();
        }
        self.admission.admit(batch).unwrap();
    }
    fn packets(&mut self, frames: usize) -> Vec<PerformancePacket> {
        self.renderer
            .as_mut()
            .unwrap()
            .render_into(&mut vec![0.0; frames]);
        let mut packets = Vec::new();
        while let Some(packet) = self.owner.try_pop().unwrap() {
            packets.push(packet);
        }
        packets
    }
    fn render(&mut self, writer: &mut PerformanceWriter, frames: usize) {
        push(writer, &self.packets(frames));
        writer.flush().unwrap();
    }
    fn finish(&mut self, writer: &mut PerformanceWriter, frames: usize) {
        self.admission
            .begin_finish(self.admission.progress().scope, LiveId([43; 16]))
            .unwrap();
        self.render(writer, frames);
    }
}
impl Drop for Tap {
    fn drop(&mut self) {
        drop(self.renderer.take());
    }
}
fn push(writer: &mut PerformanceWriter, packets: &[PerformancePacket]) {
    for packet in packets {
        writer.push_packet(*packet).unwrap();
    }
}
fn event(source: u8, ticks: u64, event: LiveEventKind) -> LiveEvent {
    LiveEvent {
        source,
        connection_generation: 1,
        observed_ticks: ticks,
        event,
    }
}
fn on(source: u8, voice: u64, ticks: u64, velocity16: u16) -> LiveEvent {
    event(
        source,
        ticks,
        LiveEventKind::NoteOn {
            note: voice,
            voice,
            key: 60,
            velocity16,
            patch: LivePatch::Keys,
        },
    )
}
fn off(source: u8, voice: u64, ticks: u64, velocity16: u16) -> LiveEvent {
    event(
        source,
        ticks,
        LiveEventKind::NoteOff {
            voice,
            release_velocity16: velocity16,
        },
    )
}
fn sustain(source: u8, ticks: u64, down: bool) -> LiveEvent {
    event(source, ticks, LiveEventKind::Sustain { down })
}
fn pulse(
    source: u8,
    voice: u64,
    ticks: u64,
    duration_frames: u32,
    release_velocity16: u16,
) -> LiveEvent {
    event(
        source,
        ticks,
        LiveEventKind::Pulse {
            note: voice,
            voice,
            key: 60,
            velocity16: 65535,
            release_velocity16,
            patch: LivePatch::Keys,
            duration_frames,
        },
    )
}
fn budget(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, cancelled.clone()).unwrap(),
        cancelled,
    )
}

#[test]
fn real_v1_v2_overlap_and_multisource_projection_preserves_exact_values_and_source_bytes() {
    for v2 in [false, true] {
        let fixture = Fixture::record(v2, |tap, writer| {
            tap.admit(&[
                on(1, 1, u64::MAX - 8, 0),
                on(2, 1, 900_000_000_000_001, 65535),
            ]);
            tap.render(writer, 80);
            tap.admit(&[
                on(1, 2, u64::MAX - 7, 32769),
                off(2, 1, 900_000_000_000_002, 65535),
            ]);
            tap.render(writer, 80);
            tap.admit(&[off(1, 1, u64::MAX - 6, 0)]);
            tap.render(writer, 80);
            tap.admit(&[off(1, 2, u64::MAX - 5, 60001)]);
            tap.render(writer, 400);
            if v2 {
                tap.finish(writer, 64);
            }
        });
        fixture.recover();
        let request = fixture.request();
        let before = fixture.bytes();
        let projected = fixture.project(&request).unwrap();
        assert_eq!(
            projected,
            fixture.project(&request).unwrap(),
            "fresh cumulative budgets produce deterministic reviews"
        );
        assert_eq!(projected.profile_version, if v2 { 2 } else { 1 });
        assert_eq!(projected.sample_rate, 8000);
        assert_eq!(projected.counts.observations, 3);
        assert_eq!(projected.counts.applied_events, 6);
        assert_eq!(
            projected.notes.len(),
            3,
            "Batch originals and GUI witnesses are not counted twice"
        );
        let actual: Vec<_> = projected
            .notes
            .iter()
            .map(|n| {
                (
                    n.identity.source,
                    n.identity.voice,
                    n.onset_frame,
                    n.gate_end_frame,
                    n.values.start_tick,
                    n.values.duration_ticks,
                    n.values.velocity16,
                )
            })
            .collect();
        assert_eq!(
            actual,
            [
                (1, 1, 0, 160, 1000, 38, 0),
                (2, 1, 0, 80, 1000, 19, 65535),
                (1, 2, 80, 240, 1019, 39, 32769)
            ]
        );
        assert_eq!(projected.notes[0].onset_gui_ticks, u64::MAX - 8);
        assert_eq!(
            projected.notes[2].onset_record.applied_source_ordinal,
            Some(1)
        );
        assert_eq!(projected.notes[2].onset_record.callback_index, 2);
        assert_eq!(projected.notes[1].onset_record.event_index, Some(1));
        assert!(
            projected
                .notes
                .iter()
                .all(|n| n.identity.connection_generation == 1
                    && n.values.pitch == 60
                    && n.identity_sha256.len() == 64)
        );
        assert!(
            projected
                .disclosures
                .contains(&PerformanceProjectionDisclosure::UnknownTailExcluded)
        );
        assert_eq!(
            fixture.bytes(),
            before,
            "all held readers have closed before physical comparison"
        );
    }
}

#[test]
fn actual_sustain_physical_release_pulse_and_drums_keep_distinct_gate_and_envelope_facts() {
    let fixture = Fixture::record(true, |tap, writer| {
        tap.admit(&[
            on(1, 1, 1, 60001),
            sustain(1, 2, true),
            sustain(2, 1, true),
            pulse(2, 1, 2, 80, 65535),
            sustain(4, 1, true),
            pulse(4, 1, 2, 40, 0),
            event(
                3,
                1,
                LiveEventKind::NoteOn {
                    note: 1,
                    voice: 1,
                    key: 60,
                    velocity16: 32768,
                    patch: LivePatch::Drums,
                },
            ),
        ]);
        tap.render(writer, 40);
        tap.admit(&[off(1, 1, 3, 43210), off(1, 1, 4, 123), off(4, 1, 3, 60001)]);
        tap.render(writer, 40);
        tap.admit(&[sustain(2, 3, false), sustain(1, 5, false)]);
        tap.render(writer, 2000);
        tap.admit(&[off(4, 1, 4, 7), off(2, 1, 4, 9)]);
        tap.render(writer, 64);
        tap.finish(writer, 64);
    });
    fixture.recover();
    let request = fixture.request();
    let before = fixture.bytes();
    let review = fixture.project(&request).unwrap();
    assert_eq!(review.notes.len(), 4);
    assert_eq!(review.counts.repeated_note_offs, 1);
    assert_eq!(review.counts.late_note_offs, 2);
    let note = |source| {
        review
            .notes
            .iter()
            .find(|n| n.identity.source == source)
            .unwrap()
    };
    let sustained = note(1);
    let key_up = sustained.physical_key_up.as_ref().unwrap();
    assert_eq!(
        (
            key_up.frame,
            key_up.release_velocity16,
            key_up.observed_ticks
        ),
        (40, 43210, 3)
    );
    assert_eq!(sustained.gate_end_frame, 80);
    assert!(matches!(
        sustained.gate_end.cause,
        PerformanceGateEndCause::SustainUp {}
    ));
    assert_eq!(sustained.gate_end.release_velocity16, Some(43210));
    assert_eq!(sustained.gate_end.observed_ticks, Some(5));
    assert!(sustained.voice_ended.as_ref().unwrap().frame > sustained.gate_end_frame);
    let automatic = note(2);
    assert_eq!(automatic.gate_end_frame, 80);
    assert!(matches!(
        automatic.gate_end.cause,
        PerformanceGateEndCause::AutoRelease {}
    ));
    assert_eq!(automatic.gate_end.release_velocity16, Some(65535));
    assert_eq!(automatic.gate_end.observed_ticks, None);
    assert_eq!(
        automatic.physical_key_up.as_ref().unwrap().frame,
        2080,
        "later GUI off does not replace the actual automatic gate"
    );
    assert_eq!(automatic.pulse.unwrap().release_velocity16, 65535);
    let early = note(4);
    assert_eq!(early.gate_end_frame, 40);
    assert!(matches!(
        early.gate_end.cause,
        PerformanceGateEndCause::NoteOff {}
    ));
    assert_eq!(early.gate_end.release_velocity16, Some(60001));
    assert_eq!(
        early.pulse.unwrap().release_velocity16,
        0,
        "planned automatic release stays separate from its earlier physical release"
    );
    let drums = note(3);
    assert!(drums.pulse.is_none());
    assert_eq!(drums.patch, LivePatch::Drums);
    assert_eq!(
        drums.gate_end_frame, 1600,
        "ordinary drum NoteOn has the renderer's rate/5 schedule"
    );
    assert_eq!(drums.gate_end.release_velocity16, Some(0));
    assert!(matches!(
        drums.gate_end.cause,
        PerformanceGateEndCause::AutoRelease {}
    ));
    assert!(drums.physical_key_up.is_none() && drums.voice_ended.is_none());
    assert!(
        drums
            .hard_cut
            .as_ref()
            .is_some_and(|cut| matches!(cut.cause, PerformanceGateEndCause::Finish {}))
    );
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn real_panic_discards_queued_batch_and_separates_reused_voice_ids_without_human_off() {
    for v2 in [false, true] {
        let fixture = Fixture::record(v2, |tap, writer| {
            tap.admit(&[on(1, 1, 90000, 65535)]);
            tap.render(writer, 80);
            tap.admit(&[on(2, 1, 1, 65535)]);
            let next = tap
                .admission
                .begin_panic(tap.admission.progress().scope)
                .unwrap();
            tap.render(writer, 80);
            assert!(tap.admission.finish_panic().unwrap().is_some());
            assert_eq!(tap.admission.progress().scope, next);
            tap.admit(&[on(1, 1, 90001, 1)]);
            tap.render(writer, 80);
            tap.admit(&[off(1, 1, 90002, 65535)]);
            tap.render(writer, 400);
            if v2 {
                tap.finish(writer, 64);
            }
        });
        fixture.recover();
        let before = fixture.bytes();
        let review = fixture.project(&fixture.request()).unwrap();
        assert_eq!(review.counts.discarded_batches, 1);
        assert_eq!(review.counts.observations, 2);
        assert_eq!(review.notes.len(), 2);
        let first = &review.notes[0];
        let reused = &review.notes[1];
        assert_eq!(
            (
                first.identity.source,
                first.identity.voice,
                first.identity.scope.panic_generation
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (
                reused.identity.source,
                reused.identity.voice,
                reused.identity.scope.panic_generation
            ),
            (1, 1, 2)
        );
        assert_ne!(first.identity_sha256, reused.identity_sha256);
        assert_eq!((first.onset_frame, first.gate_end_frame), (0, 80));
        assert!(first.physical_key_up.is_none() && first.voice_ended.is_none());
        assert!(matches!(
            first.gate_end.cause,
            PerformanceGateEndCause::Clear {
                reason: TapClearReason::Panic { next_generation: 2 }
            }
        ));
        assert!(first.hard_cut.as_ref().is_some_and(|cut| cut.frame == 80));
        assert_eq!((reused.onset_frame, reused.gate_end_frame), (160, 240));
        assert!(matches!(
            reused.gate_end.cause,
            PerformanceGateEndCause::NoteOff {}
        ));
        assert!(
            review
                .disclosures
                .contains(&PerformanceProjectionDisclosure::ObservedHardCutsRetained)
        );
        assert_eq!(fixture.bytes(), before);
    }
}

#[test]
fn rejected_atomic_batch_preserves_its_original_but_projects_none_of_its_earlier_valid_events() {
    let fixture = Fixture::record(false, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535), off(1, 99, 2, 12345)]);
        tap.render(writer, 80);
    });
    fixture.recover();
    let before = fixture.bytes();
    let review = fixture.project(&fixture.request()).unwrap();
    assert_eq!(review.counts.rejected_batches, 1);
    assert_eq!(review.counts.applied_events, 0);
    assert_eq!(review.counts.observations, 0);
    assert!(review.notes.is_empty() && review.omissions.is_empty());
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_finish_gate_is_an_engine_cut_while_finish_outside_selected_checkpoint_is_not_authority() {
    let complete = Fixture::record(true, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535)]);
        tap.render(writer, 80);
        tap.finish(writer, 80);
    });
    complete.recover();
    let before = complete.bytes();
    let review = complete.project(&complete.request()).unwrap();
    assert_eq!(review.notes.len(), 1);
    let note = &review.notes[0];
    assert_eq!(
        (
            note.onset_frame,
            note.gate_end_frame,
            note.values.duration_ticks
        ),
        (0, 160, 38)
    );
    assert!(matches!(
        note.gate_end.cause,
        PerformanceGateEndCause::Finish {}
    ));
    assert!(note.gate_end.release_velocity16.is_none() && note.gate_end.observed_ticks.is_none());
    assert!(note.physical_key_up.is_none() && note.voice_ended.is_none());
    assert!(note.hard_cut.as_ref().is_some_and(
        |cut| cut.frame == 160 && matches!(cut.cause, PerformanceGateEndCause::Finish {})
    ));
    assert_eq!(complete.bytes(), before);

    let partial = Fixture::record(true, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535)]);
        tap.render(writer, 80);
        tap.admission
            .begin_finish(tap.admission.progress().scope, LiveId([44; 16]))
            .unwrap();
        let last = tap.packets(80);
        assert!(
            last.iter()
                .any(|p| matches!(p.kind, PerformancePacketKind::Finish(_)))
        );
        assert!(matches!(
            last.last().unwrap().kind,
            PerformancePacketKind::End(_)
        ));
        push(writer, &last[..last.len() - 1]);
    });
    let reader = CaptureReader::open(&partial.path).unwrap();
    let selected = inspect_performance(&reader).unwrap();
    assert!(selected.problem.is_some());
    assert!(selected.finish_cut.is_none());
    assert_eq!(selected.checkpoint.as_ref().unwrap().audio_end, 80);
    recover_performance(reader, &selected, "projection-review.partial-terminal").unwrap();
    let before = partial.bytes();
    let mut request = partial.request();
    assert_eq!(request.region.end, 80);
    assert!(matches!(
        partial.project(&request),
        Err(RecordingError::Invalid(_))
    ));
    request.incomplete = PerformanceProjectionPolicy::OmitWithDisclosure;
    let review = partial.project(&request).unwrap();
    assert_eq!(review.counts.incomplete, 1);
    assert_eq!(review.counts.hard_cuts, 0);
    assert!(review.notes.is_empty());
    assert!(review.omissions.iter().all(|n| n.gate_end_frame.is_none()));
    assert!(
        !review
            .disclosures
            .contains(&PerformanceProjectionDisclosure::ObservedHardCutsRetained)
    );
    assert_eq!(partial.bytes(), before);
}

#[test]
fn zero_frame_and_collapsed_tick_gates_and_unknown_tail_require_explicit_omission_policy() {
    let fixture = Fixture::record(false, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535), off(1, 1, 2, 65535)]);
        tap.render(writer, 1);
        tap.admit(&[on(1, 2, 3, 65535), on(2, 1, 1, 65535)]);
        tap.render(writer, 1);
        tap.admit(&[off(1, 2, 4, 65535)]);
        tap.render(writer, 80);
    });
    fixture.recover();
    let before = fixture.bytes();
    let mut request = fixture.request();
    assert!(matches!(
        fixture.project(&request),
        Err(RecordingError::Invalid(_))
    ));
    request.collapsed = PerformanceProjectionPolicy::OmitWithDisclosure;
    assert!(matches!(
        fixture.project(&request),
        Err(RecordingError::Invalid(_))
    ));
    request.incomplete = PerformanceProjectionPolicy::OmitWithDisclosure;
    let review = fixture.project(&request).unwrap();
    assert!(review.notes.is_empty());
    assert_eq!(review.counts.collapsed, 2);
    assert_eq!(review.counts.incomplete, 1);
    assert_eq!(review.omissions.len(), 3);
    assert_eq!(
        (
            review.omissions[0].onset_frame,
            review.omissions[0].gate_end_frame
        ),
        (0, Some(0))
    );
    assert!(review.omissions.iter().any(|o| o.reason
        == PerformanceProjectionOmissionReason::Incomplete
        && o.gate_end_frame.is_none()));
    assert!(
        review
            .disclosures
            .contains(&PerformanceProjectionDisclosure::UnknownTailExcluded)
    );
    assert!(
        !review
            .disclosures
            .contains(&PerformanceProjectionDisclosure::ObservedHardCutsRetained)
    );
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn real_source_region_crossings_are_omitted_not_trimmed_and_exclusions_use_counts_and_digest() {
    let fixture = Fixture::record(false, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535)]);
        tap.render(writer, 80);
        tap.admit(&[off(1, 1, 2, 65535), on(2, 1, 1, 65535)]);
        tap.render(writer, 80);
        tap.admit(&[off(2, 1, 2, 65535)]);
        tap.render(writer, 80);
    });
    fixture.recover();
    let before = fixture.bytes();
    let mut request = fixture.request();
    request.region = PerformanceFrameRegion {
        start: 40,
        end: 120,
    };
    assert!(matches!(
        fixture.project(&request),
        Err(RecordingError::Invalid(_))
    ));
    request.region_crossing = PerformanceProjectionPolicy::OmitWithDisclosure;
    let crossing = fixture.project(&request).unwrap();
    assert!(crossing.notes.is_empty());
    assert_eq!(crossing.counts.region_crossing, 2);
    assert!(
        crossing
            .omissions
            .iter()
            .all(|o| o.reason == PerformanceProjectionOmissionReason::RegionCrossing)
    );
    request.region = PerformanceFrameRegion {
        start: 80,
        end: 160,
    };
    let exact = fixture.project(&request).unwrap();
    assert_eq!(exact.notes.len(), 1);
    assert_eq!(
        (
            exact.notes[0].onset_frame,
            exact.notes[0].gate_end_frame,
            exact.notes[0].values.start_tick
        ),
        (80, 160, 1000)
    );
    assert_eq!(exact.counts.outside_region, 1);
    assert!(
        exact.omissions.is_empty(),
        "outside-region history does not consume selected omission capacity"
    );
    request.sources = vec![1];
    let selected = fixture.project(&request).unwrap();
    assert!(selected.notes.is_empty() && selected.omissions.is_empty());
    assert_eq!(
        (
            selected.counts.observations,
            selected.counts.outside_region,
            selected.counts.excluded_source
        ),
        (2, 1, 1)
    );
    assert_ne!(
        selected.unselected_observations_sha256,
        exact.unselected_observations_sha256
    );
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn coherent_writer_checkpoints_with_false_automatic_transition_semantics_are_refused() {
    for mode in 0..3 {
        let fixture = Fixture::record(false, |tap, writer| {
            tap.admit(&[pulse(1, 1, 1, 40, 65535)]);
            let mut packets = tap.packets(400);
            let end = packets
                .iter_mut()
                .find_map(|packet| match &mut packet.kind {
                    PerformancePacketKind::End(end) => Some(end),
                    _ => None,
                })
                .unwrap();
            let transition = end
                .transitions
                .iter_mut()
                .flatten()
                .find(|t| match mode {
                    0 | 2 => matches!(t.kind, TapTransitionKind::AutoRelease { .. }),
                    1 => matches!(t.kind, TapTransitionKind::VoiceEnded),
                    _ => unreachable!(),
                })
                .unwrap();
            match mode {
                0 => transition.frame += 1,
                1 => transition.frame -= 1,
                2 => transition.voice = 2,
                _ => unreachable!(),
            }
            // The actual writer regenerates canonical envelopes, chunk hashes,
            // checkpoint membership/index hashes and then the recovered manifest.
            // Its existing structural validator accepts this coherent source;
            // the projection must independently reject the false voice lifecycle.
            push(writer, &packets);
            writer.flush().unwrap();
        });
        fixture.recover();
        let request = fixture.request();
        let before = fixture.bytes();
        assert!(
            matches!(fixture.project(&request), Err(RecordingError::Invalid(_))),
            "mode {mode}"
        );
        assert_eq!(
            fixture.bytes(),
            before,
            "semantic refusal retains every canonical original"
        );
    }
}

#[test]
fn coherently_rehashed_rejected_then_applied_order_cannot_invent_a_renderer_continuation() {
    let fixture = Fixture::record(false, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535)]);
        tap.admit(&[off(1, 99, 2, 12345)]);
        let mut packets = tap.packets(80);
        let batches: Vec<_> = packets
            .iter()
            .enumerate()
            .filter_map(|(i, p)| matches!(p.kind, PerformancePacketKind::Batch { .. }).then_some(i))
            .collect();
        assert_eq!(batches.len(), 2);
        let first = packets[batches[0]].kind;
        packets[batches[0]].kind = packets[batches[1]].kind;
        packets[batches[1]].kind = first;
        // Retain the real fault/Clear/End, exact Applied seq1 and rejected seq2,
        // but invert their callback order. Canonical writer/checkpoint hashing
        // accepts it; a real renderer stops dequeueing immediately on rejection.
        push(writer, &packets);
        writer.flush().unwrap();
    });
    fixture.recover();
    let request = fixture.request();
    let before = fixture.bytes();
    assert!(matches!(
        fixture.project(&request),
        Err(RecordingError::Invalid(_))
    ));
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn projection_refuses_every_changed_source_pin_and_keeps_cancel_and_work_limits_typed_sticky() {
    let fixture = Fixture::record(false, |tap, writer| {
        tap.admit(&[on(1, 1, 1, 65535)]);
        tap.render(writer, 80);
        tap.admit(&[off(1, 1, 2, 65535)]);
        tap.render(writer, 400);
    });
    fixture.recover();
    let request = fixture.request();
    let before = fixture.bytes();
    for pin in 0..9 {
        let mut changed = request.clone();
        match pin {
            0 => changed.source.take_id = "different-valid-take".into(),
            1 => changed.source.manifest_sha256 = "0".repeat(64),
            2 => changed.source.descriptor_sha256 = "0".repeat(64),
            3 => changed.source.original_rows_sha256 = "0".repeat(64),
            4 => changed.source.journal_sha256_at_review = "0".repeat(64),
            5 => changed.source.checkpoint_sha256 = "0".repeat(64),
            6 => changed.source.selection.chunk_index_sha256 = "0".repeat(64),
            7 => changed.source.selection.chunk_count += 1,
            8 => changed.region.end += 1,
            _ => unreachable!(),
        }
        assert!(
            matches!(fixture.project(&changed), Err(RecordingError::Invalid(_))),
            "pin {pin}"
        );
    }
    {
        let unbounded = SealedJournal::open(&fixture.path).unwrap();
        assert!(matches!(
            project_sealed_performance_bounded(&unbounded, &request),
            Err(RecordingError::Invalid(_))
        ));
    }
    {
        let (budget, cancelled) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&fixture.path, &budget).unwrap();
        cancelled.store(true, Ordering::Release);
        let mut malformed = request.clone();
        malformed.version = 2;
        assert!(matches!(
            project_sealed_performance_bounded(&reader, &malformed),
            Err(RecordingError::InspectionCancelled)
        ));
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionCancelled)
        ));
    }
    {
        let limits = InspectionLimits::default();
        let (budget, _) = budget(limits);
        let reader = SealedJournal::open_bounded(&fixture.path, &budget).unwrap();
        budget
            .charge_work(limits.max_work_bytes - budget.usage().work_bytes)
            .unwrap();
        assert!(matches!(
            project_sealed_performance_bounded(&reader, &request),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                ..
            })
        ));
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                ..
            })
        ));
    }
    {
        let length = fs::metadata(&fixture.path).unwrap().len();
        let (budget, _) = budget(InspectionLimits {
            max_file_bytes: length - 1,
            ..Default::default()
        });
        assert!(matches!(
            SealedJournal::open_bounded(&fixture.path, &budget),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::FileBytes,
                ..
            })
        ));
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionLimit {
                resource: InspectionResource::FileBytes,
                ..
            })
        ));
    }
    assert_eq!(fixture.bytes(), before);
}

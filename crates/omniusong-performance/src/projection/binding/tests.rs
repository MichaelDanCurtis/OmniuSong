use super::*;
use crate::live_types::LiveId;
use omniusong_recording::{InspectionLimits, InspectionResource};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn budget() -> InspectionBudget {
    InspectionBudget::new(
        InspectionLimits::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn stable_id(projection: &str, voice_sha: &str) -> NoteId {
    // Independent raw-byte oracle, not the production helper or typed digest.
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-projection-document-note.v1\0");
    hash.update((projection.len() as u64).to_le_bytes());
    hash.update(projection.as_bytes());
    hash.update((voice_sha.len() as u64).to_le_bytes());
    hash.update(voice_sha.as_bytes());
    NoteId(format!("pn-{:x}", hash.finalize()))
}
fn rehash(review: &mut PerformanceProjectionReview) {
    review.review_sha256.clear();
    let bytes = serde_json::to_vec(&*review).unwrap();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-projection-review.v1\0");
    hash.update(bytes);
    review.review_sha256 = format!("{:x}", hash.finalize());
}
fn location(index: u64) -> PerformanceRecordLocation {
    PerformanceRecordLocation {
        stream_id: "control".into(),
        stream_event_ordinal: index,
        chunk_ordinal: 2,
        record_index: index as u32,
        callback_index: 1,
        batch_ordinal: Some(1),
        event_index: Some(index as u8),
        applied_source_ordinal: Some(index),
    }
}
fn fixture() -> (PerformanceProjectionBinding, PerformanceProjectionReview) {
    let mut request = super::super::tests::request();
    request.region.end = 200;
    let scope = LiveScope {
        worker_generation: LiveId([1; 16]),
        session_id: LiveId([2; 16]),
        panic_generation: 1,
    };
    let specs = [
        (1, 1, LivePatch::Keys),
        (1, 2, LivePatch::Keys),
        (1, 3, LivePatch::Bass),
        (2, 1, LivePatch::Pad),
    ];
    request.sources = vec![1, 2];
    let mut notes = Vec::new();
    for (index, (source, voice, patch)) in specs.into_iter().enumerate() {
        let identity = PerformanceVoiceIdentity {
            take_id: request.source.take_id.clone(),
            scope,
            source,
            connection_generation: 1,
            note: voice,
            voice,
        };
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.performance-projected-note.v1\0");
        hash.update(serde_json::to_vec(&identity).unwrap());
        let identity_sha256 = format!("{:x}", hash.finalize());
        let onset_frame = index as u64 * 32;
        notes.push(ProjectedPerformanceNote {
            identity,
            identity_sha256,
            values: NoteValues {
                pitch: 60 + index as u8,
                start_tick: onset_frame / 4,
                duration_ticks: 4,
                velocity16: 0,
            },
            onset_frame,
            gate_end_frame: onset_frame + 16,
            onset_gui_ticks: 100 + index as u64,
            patch,
            pulse: None,
            onset_record: location(index as u64),
            physical_key_up: Some(PerformancePhysicalRelease {
                frame: onset_frame + 16,
                release_velocity16: 2222,
                observed_ticks: 200 + index as u64,
                record: location(index as u64 + 4),
            }),
            gate_end: PerformanceGateEnd {
                cause: PerformanceGateEndCause::NoteOff {},
                release_velocity16: Some(2222),
                observed_ticks: Some(200 + index as u64),
                record: location(index as u64 + 4),
            },
            voice_ended: None,
            hard_cut: None,
        });
    }
    let mut review = PerformanceProjectionReview {
        version: 1,
        request,
        profile_version: 2,
        sample_rate: 8000,
        gui_clock: LiveClock {
            domain: "fixture-gui".into(),
            epoch: "fixture-epoch".into(),
            ticks_per_second_numerator: 1_000_000,
            ticks_per_second_denominator: 1,
        },
        notes,
        omissions: Vec::new(),
        unselected_observations_sha256: "d".repeat(64),
        counts: PerformanceProjectionCounts {
            observations: 4,
            notes: 4,
            applied_events: 8,
            ..Default::default()
        },
        disclosures: vec![PerformanceProjectionDisclosure::ProceduralDerivationNotAi],
        review_sha256: String::new(),
    };
    rehash(&mut review);
    let mut lanes = Vec::new();
    for (source, instrument, track_id) in [
        (1, ProjectionInstrument::Keys, "lane-keys"),
        (1, ProjectionInstrument::Bass, "lane-bass"),
        (2, ProjectionInstrument::Pad, "lane-pad"),
    ] {
        let original_notes = review
            .notes
            .iter()
            .filter(|note| note.identity.source == source && patch(note.patch) == instrument)
            .map(|note| ProjectionNoteBinding {
                note_id: stable_id("projection-one", &note.identity_sha256),
                identity_sha256: note.identity_sha256.clone(),
                values: note.values,
            })
            .collect();
        lanes.push(ProjectionLaneBinding {
            source,
            instrument,
            track_id: track_id.into(),
            original_notes,
        });
    }
    let binding = PerformanceProjectionBinding {
        id: "projection-one".into(),
        version: 1,
        source: TakeReference {
            take_id: TakeId(review.request.source.take_id.clone()),
            manifest_sha256: review.request.source.manifest_sha256.clone(),
            stream_id: "audio".into(),
        },
        review: RetainedProjectionReview {
            version: 1,
            canonical_json: serde_json::to_string(&review).unwrap(),
            review_sha256: review.review_sha256.clone(),
        },
        lanes,
    };
    (binding, review)
}
fn choice(source: u8, instrument: ProjectionInstrument, id: &str) -> ProjectionLaneChoice {
    ProjectionLaneChoice {
        source,
        instrument,
        track_id: id.into(),
        track_name: "Editable human performance".into(),
    }
}

#[test]
fn stable_note_ids_match_independent_length_framing_and_projection_namespace() {
    let governor = budget();
    let voice = "a".repeat(64);
    let expected = stable_id("projection-one", &voice);
    assert_eq!(
        document_note_id("projection-one", &voice, &governor).unwrap(),
        expected
    );
    assert_eq!(expected.0.len(), 67);
    assert!(expected.0.starts_with("pn-"));
    assert_ne!(
        expected,
        document_note_id("projection-two", &voice, &governor).unwrap()
    );
    assert_ne!(
        expected,
        document_note_id("projection-one", &"b".repeat(64), &governor).unwrap()
    );
    assert!(document_note_id("bad@projection", &voice, &governor).is_err());
}

#[test]
fn canonical_encoding_checks_exact_group_note_bijection_and_original_order() {
    let (valid, _) = fixture();
    check_retained_binding_encoding_bounded(&valid, &budget()).unwrap();
    let mut bad = valid.clone();
    bad.lanes[0].original_notes[0].values.pitch += 1;
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad.lanes[0].original_notes[0].note_id = NoteId("unrelated-valid-id".into());
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad.lanes[0].original_notes.swap(0, 1);
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad.lanes.swap(0, 1);
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad.lanes[2].instrument = ProjectionInstrument::Drums;
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad.lanes[0].original_notes.remove(0);
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad.lanes.remove(2);
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid;
    bad.id = "projection-other".into();
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
}

#[test]
fn noncanonical_unknown_and_self_inconsistent_retained_encodings_refuse() {
    let (valid, review) = fixture();
    let mut bad = valid.clone();
    bad.review.canonical_json.insert(0, ' ');
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad.review.canonical_json.push('\n');
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut extra = serde_json::to_value(&review).unwrap();
    extra["unknownOwnedField"] = true.into();
    let mut bad = valid.clone();
    bad.review.canonical_json = serde_json::to_string(&extra).unwrap();
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut forged = review.clone();
    forged.review_sha256 = "0".repeat(64);
    let mut bad = valid.clone();
    bad.review.canonical_json = serde_json::to_string(&forged).unwrap();
    bad.review.review_sha256 = forged.review_sha256;
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut false_counts = review;
    false_counts.counts.observations += 1;
    rehash(&mut false_counts);
    let mut bad = valid.clone();
    bad.review.canonical_json = serde_json::to_string(&false_counts).unwrap();
    bad.review.review_sha256 = false_counts.review_sha256;
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
    let mut bad = valid;
    bad.source.take_id = TakeId("other-original".into());
    assert!(check_retained_binding_encoding_bounded(&bad, &budget()).is_err());
}

#[test]
fn a_coherent_false_musical_review_has_encoding_consistency_only() {
    let (mut binding, mut review) = fixture();
    review.notes[0].values.pitch = 111;
    binding.lanes[0].original_notes[0].values.pitch = 111;
    rehash(&mut review);
    binding.review.review_sha256 = review.review_sha256.clone();
    binding.review.canonical_json = serde_json::to_string(&review).unwrap();
    // A source-free unit result deliberately cannot prove what was performed.
    // The separate actual-source fixture must reject this through the held
    // AuthenticatedPerformance::validate_binding rederivation.
    check_retained_binding_encoding_bounded(&binding, &budget()).unwrap();
}

#[test]
fn explicit_lane_choices_are_sorted_unique_bounded_and_names_are_safe() {
    let hash = "a".repeat(64);
    let valid = vec![
        choice(1, ProjectionInstrument::Keys, "keys"),
        choice(1, ProjectionInstrument::Bass, "bass"),
        choice(2, ProjectionInstrument::Pad, "pad"),
    ];
    choices_shape("projection", &hash, &valid, &budget()).unwrap();
    let mut bad = valid.clone();
    bad.swap(0, 1);
    assert!(choices_shape("projection", &hash, &bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad[1].instrument = ProjectionInstrument::Keys;
    assert!(choices_shape("projection", &hash, &bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad[1].track_id = bad[0].track_id.clone();
    assert!(choices_shape("projection", &hash, &bad, &budget()).is_err());
    let mut bad = valid.clone();
    bad[0].source = 0;
    assert!(choices_shape("projection", &hash, &bad, &budget()).is_err());
    for name in [
        "   ".into(),
        "bad\nname".into(),
        "x".repeat(161),
        "🎹".repeat(161),
    ] {
        let mut bad = valid.clone();
        bad[0].track_name = name;
        assert!(choices_shape("projection", &hash, &bad, &budget()).is_err());
    }
    let mut exact = valid.clone();
    exact[0].track_name = "🎹".repeat(160);
    choices_shape("projection", &hash, &exact, &budget()).unwrap();
    assert!(choices_shape("projection", &hash, &[], &budget()).is_err());
    assert!(choices_shape("projection", &hash, &vec![valid[0].clone(); 17], &budget()).is_err());
    assert!(choices_shape("projection", &"A".repeat(64), &valid, &budget()).is_err());
}

#[test]
fn sticky_cancellation_and_work_limits_win_before_decode_and_return_no_check() {
    let (binding, _) = fixture();
    let flag = Arc::new(AtomicBool::new(true));
    let governor = InspectionBudget::new(InspectionLimits::default(), flag.clone()).unwrap();
    assert!(matches!(
        check_retained_binding_encoding_bounded(&binding, &governor),
        Err(RecordingError::InspectionCancelled)
    ));
    flag.store(false, Ordering::Release);
    assert!(matches!(
        check_retained_binding_encoding_bounded(&binding, &governor),
        Err(RecordingError::InspectionCancelled)
    ));
    let limited = InspectionBudget::new(
        InspectionLimits {
            max_work_bytes: 1,
            ..Default::default()
        },
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(matches!(
        check_retained_binding_encoding_bounded(&binding, &limited),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        })
    ));
    let mut oversized = binding;
    oversized.review.canonical_json = " ".repeat(MAX_PROJECTION_REVIEW_BYTES + 1);
    assert!(check_retained_binding_encoding_bounded(&oversized, &budget()).is_err());
}

#[test]
fn bounded_streaming_serializer_matches_existing_typed_review_bytes() {
    let (_, review) = fixture();
    let governor = budget();
    let json = canonical_review(&review, &governor).unwrap();
    assert_eq!(json, serde_json::to_string(&review).unwrap());
    canonical_matches(&review, &json, &governor).unwrap();
    assert!(canonical_matches(&review, &format!("{json} "), &governor).is_err());
}

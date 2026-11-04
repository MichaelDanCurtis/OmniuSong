//! Actual recovered sources, independent stable-ID/hash oracles, and the
//! distinction between portable encoding checks and held-source authority.
use super::*;
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, ProjectDocument,
    performance_projection::{
        PerformanceProjectionBinding, ProjectionInstrument, ProjectionLaneChoice,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn source(v2: bool) -> Fixture {
    let fixture = Fixture::record(v2, |tap, writer| {
        tap.admit(&[
            on(1, 1, 1, 0),
            event(
                1,
                2,
                LiveEventKind::NoteOn {
                    note: 2,
                    voice: 2,
                    key: 48,
                    velocity16: 65535,
                    patch: LivePatch::Bass,
                },
            ),
            event(
                2,
                1,
                LiveEventKind::Pulse {
                    note: 1,
                    voice: 1,
                    key: 67,
                    velocity16: 41234,
                    release_velocity16: 54321,
                    patch: LivePatch::Pad,
                    duration_frames: 80,
                },
            ),
            event(
                3,
                1,
                LiveEventKind::NoteOn {
                    note: 1,
                    voice: 1,
                    key: 36,
                    velocity16: 1,
                    patch: LivePatch::Drums,
                },
            ),
        ]);
        tap.render(writer, 80);
        tap.admit(&[off(1, 1, 3, 0), off(1, 2, 4, 65535)]);
        tap.render(writer, 2000);
        if v2 {
            tap.finish(writer, 64);
        }
    });
    fixture.recover();
    fixture
}
fn lanes(prefix: &str) -> Vec<ProjectionLaneChoice> {
    [
        (1, ProjectionInstrument::Keys, "keys"),
        (1, ProjectionInstrument::Bass, "bass"),
        (2, ProjectionInstrument::Pad, "pad"),
        (3, ProjectionInstrument::Drums, "drums"),
    ]
    .into_iter()
    .map(|(source, instrument, name)| ProjectionLaneChoice {
        source,
        instrument,
        track_id: format!("{prefix}-{name}"),
        track_name: format!("Original {prefix} {name}"),
    })
    .collect()
}
fn invalid<T: std::fmt::Debug>(result: omniusong_recording::Result<T>) {
    assert!(
        matches!(result, Err(RecordingError::Invalid(_))),
        "Expected semantic refusal, got {result:?}"
    );
}
fn document_note_id(projection: &str, identity: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"OmniuSong.performance-projection-document-note.v1\0");
    for field in [projection.as_bytes(), identity.as_bytes()] {
        digest.update((field.len() as u64).to_le_bytes());
        digest.update(field);
    }
    format!("pn-{:x}", digest.finalize())
}
fn rehash_review(review: &mut PerformanceProjectionReview) {
    review.review_sha256.clear();
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-projection-review.v1\0");
    hash.update(serde_json::to_vec(&*review).unwrap());
    review.review_sha256 = format!("{:x}", hash.finalize());
}
fn retain_review(binding: &mut PerformanceProjectionBinding, review: &PerformanceProjectionReview) {
    binding.review.canonical_json = serde_json::to_string(review).unwrap();
    binding.review.review_sha256 = review.review_sha256.clone();
}
fn checked_encoding(binding: &PerformanceProjectionBinding) -> omniusong_recording::Result<()> {
    let (governor, _) = budget(InspectionLimits::default());
    check_retained_binding_encoding_bounded(binding, &governor)
}

#[test]
fn real_v1_v2_held_projection_builds_deterministic_bindings_and_exact_muted_candidates() {
    for v2 in [false, true] {
        let fixture = source(v2);
        let request = fixture.request();
        let original = fixture.bytes();
        {
            let (governor, _) = budget(InspectionLimits::default());
            let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
            let held = AuthenticatedPerformance::open(&reader).unwrap();
            assert_eq!(held.source_pin(), &request.source);
            let review = held.project(&request).unwrap();
            assert_eq!(review, fixture.project(&request).unwrap());
            assert_eq!(review.profile_version, if v2 { 2 } else { 1 });
            assert_eq!(review.notes.len(), 4);
            let choices = lanes("first");
            let binding = held
                .build_binding("projection-a", &request, &review.review_sha256, &choices)
                .unwrap();
            assert_eq!(
                binding,
                held.build_binding("projection-a", &request, &review.review_sha256, &choices)
                    .unwrap()
            );
            assert_eq!(binding.source.take_id.0, request.source.take_id);
            assert_eq!(
                binding.source.manifest_sha256,
                request.source.manifest_sha256
            );
            assert_eq!(binding.source.stream_id, "audio");
            assert_eq!(
                binding.review.canonical_json,
                serde_json::to_string(&review).unwrap()
            );
            let mut independently_hashed = review.clone();
            rehash_review(&mut independently_hashed);
            assert_eq!(independently_hashed.review_sha256, review.review_sha256);
            checked_encoding(&binding).unwrap();
            held.validate_binding(&binding).unwrap();
            let mut ids = std::collections::BTreeSet::new();
            for lane in &binding.lanes {
                assert_eq!(lane.original_notes.len(), 1);
                let note = &lane.original_notes[0];
                let original_note = review
                    .notes
                    .iter()
                    .find(|original| original.identity_sha256 == note.identity_sha256)
                    .unwrap();
                assert_eq!(
                    note.note_id.0,
                    document_note_id("projection-a", &original_note.identity_sha256)
                );
                assert_eq!(note.values, original_note.values);
                assert_eq!(lane.source, original_note.identity.source);
                assert!(ids.insert(note.note_id.0.clone()));
            }
            assert_eq!(binding.lanes[0].original_notes[0].values.velocity16, 0);
            assert_eq!(binding.lanes[1].original_notes[0].values.velocity16, 65535);
            assert_eq!(binding.lanes[2].original_notes[0].values.pitch, 67);
            assert_eq!(binding.lanes[3].original_notes[0].values.pitch, 36);
            let original_document =
                ProjectDocument::new("Authored material remains exact").unwrap();
            let before = original_document.to_json().unwrap();
            invalid_core(original_document.append_performance_projection(
                &binding,
                &choices,
                NoteTimelinePolicy::PreserveLoop,
                false,
            ));
            let candidate = original_document
                .append_performance_projection(
                    &binding,
                    &choices,
                    NoteTimelinePolicy::PreserveLoop,
                    true,
                )
                .unwrap();
            assert_eq!(original_document.to_json().unwrap(), before);
            assert_eq!(candidate.id(), original_document.id());
            assert_eq!(candidate.revision(), original_document.revision());
            assert_eq!(candidate.schema_version(), 3);
            assert_eq!(
                &candidate.tracks()[..original_document.tracks().len()],
                original_document.tracks()
            );
            assert_eq!(
                candidate
                    .performance_projections()
                    .unwrap()
                    .unwrap()
                    .entries,
                vec![binding.clone()]
            );
            assert_eq!(
                candidate.referenced_takes().unwrap(),
                [binding.source.clone()].into()
            );
            for (choice, lane) in choices.iter().zip(&binding.lanes) {
                let track = candidate
                    .tracks()
                    .iter()
                    .find(|track| track["id"] == choice.track_id)
                    .unwrap();
                assert_eq!(track["kind"], "notes");
                assert_eq!(track["muted"], true);
                assert_eq!(track["name"], choice.track_name);
                assert_eq!(track["instrument"], choice.instrument.as_str());
                let notes = track["notes"].as_array().unwrap();
                assert_eq!(notes.len(), lane.original_notes.len());
                for (note, original) in notes.iter().zip(&lane.original_notes) {
                    assert_eq!(note["id"], original.note_id.0);
                    assert_eq!(note["pitch"], original.values.pitch);
                    assert_eq!(note["startTick"], original.values.start_tick);
                    assert_eq!(note["durationTicks"], original.values.duration_ticks);
                    assert_eq!(note["velocity16"], original.values.velocity16);
                }
            }
            held.verify_unchanged().unwrap();
        }
        assert_eq!(
            fixture.bytes(),
            original,
            "Close all held readers before comparing physical source bytes"
        );
    }
}
fn invalid_core<T: std::fmt::Debug>(result: std::result::Result<T, omniusong_core::DocumentError>) {
    assert!(
        result.is_err(),
        "Pure constructor accepted a forbidden mutation: {result:?}"
    );
}

#[test]
fn lane_admission_requires_the_exact_nonempty_source_patch_partition_and_review() {
    let fixture = source(true);
    let request = fixture.request();
    let original = fixture.bytes();
    {
        let (governor, _) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let review = held.project(&request).unwrap();
        let choices = lanes("strict");
        let mut wrong = Vec::new();
        let mut reversed = choices.clone();
        reversed.reverse();
        wrong.push(reversed);
        let mut duplicate = choices.clone();
        duplicate[1] = duplicate[0].clone();
        wrong.push(duplicate);
        let mut missing = choices.clone();
        missing.pop();
        wrong.push(missing);
        let mut extra = choices.clone();
        extra.push(ProjectionLaneChoice {
            source: 4,
            instrument: ProjectionInstrument::Keys,
            track_id: "empty-extra".into(),
            track_name: "No actual notes".into(),
        });
        wrong.push(extra);
        let mut wrong_patch = choices.clone();
        wrong_patch[2].instrument = ProjectionInstrument::Keys;
        wrong.push(wrong_patch);
        let mut reused_track = choices.clone();
        reused_track[1].track_id = reused_track[0].track_id.clone();
        wrong.push(reused_track);
        let mut unnamed = choices.clone();
        unnamed[0].track_name = " ".into();
        wrong.push(unnamed);
        for choices in wrong {
            invalid(held.build_binding(
                "strict-binding",
                &request,
                &review.review_sha256,
                &choices,
            ));
        }
        invalid(held.build_binding("strict-binding", &request, &"0".repeat(64), &choices));
        let mut changed = request.clone();
        changed.destination_start_tick += 1;
        invalid(held.build_binding("strict-binding", &changed, &review.review_sha256, &choices));
        let mut wrong_source = request.clone();
        wrong_source.source.original_rows_sha256 = "0".repeat(64);
        invalid(held.project(&wrong_source));
        held.verify_unchanged().unwrap();
    }
    assert_eq!(fixture.bytes(), original);
}

#[test]
fn canonical_encoding_rejects_layout_digest_identity_value_and_extra_field_substitution() {
    let fixture = source(true);
    let request = fixture.request();
    let original = fixture.bytes();
    {
        let (governor, _) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let review = held.project(&request).unwrap();
        let binding = held
            .build_binding(
                "canonical-original",
                &request,
                &review.review_sha256,
                &lanes("canonical"),
            )
            .unwrap();
        let mut wrong = Vec::new();
        let mut whitespace = binding.clone();
        whitespace.review.canonical_json = serde_json::to_string_pretty(&review).unwrap();
        wrong.push(whitespace);
        let mut digest = binding.clone();
        digest.review.review_sha256 = "0".repeat(64);
        wrong.push(digest);
        let mut note_id = binding.clone();
        note_id.lanes[0].original_notes[0].note_id.0 = format!("pn-{}", "f".repeat(64));
        wrong.push(note_id);
        let mut identity = binding.clone();
        identity.lanes[0].original_notes[0].identity_sha256 = "e".repeat(64);
        wrong.push(identity);
        let mut values = binding.clone();
        values.lanes[0].original_notes[0].values.velocity16 = 1;
        wrong.push(values);
        let mut missing = binding.clone();
        missing.lanes.pop();
        wrong.push(missing);
        let mut swapped = binding.clone();
        swapped.lanes.swap(0, 1);
        wrong.push(swapped);
        let mut duplicate = binding.clone();
        let repeated = duplicate.lanes[0].original_notes[0].clone();
        duplicate.lanes[0].original_notes.push(repeated);
        wrong.push(duplicate);
        let mut source = binding.clone();
        source.source.stream_id = "control".into();
        wrong.push(source);
        let mut inner_extra: Value = serde_json::from_str(&binding.review.canonical_json).unwrap();
        inner_extra["notes"][0]["extra_claim"] = json!({"authenticated":true});
        let mut extra = binding.clone();
        extra.review.canonical_json = serde_json::to_string(&inner_extra).unwrap();
        wrong.push(extra);
        for binding in wrong {
            invalid(checked_encoding(&binding));
            invalid(held.validate_binding(&binding));
        }
        // Outer transport DTOs must reject unknown keys too; opaque third-party
        // fields elsewhere in a musical document do not widen this owned DTO.
        let mut choice = serde_json::to_value(&lanes("strict")[0]).unwrap();
        choice["claimedAudioPermission"] = true.into();
        assert!(serde_json::from_value::<ProjectionLaneChoice>(choice).is_err());
        let mut encoded = serde_json::to_value(&binding).unwrap();
        encoded["signature"] = "caller-authored".into();
        assert!(serde_json::from_value::<PerformanceProjectionBinding>(encoded).is_err());
        held.verify_unchanged().unwrap();
    }
    assert_eq!(fixture.bytes(), original);
}

#[test]
fn internally_consistent_rehashed_false_music_pins_and_voice_identity_have_no_source_authority() {
    let fixture = source(true);
    let request = fixture.request();
    let original = fixture.bytes();
    {
        let (governor, _) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let review = held.project(&request).unwrap();
        let binding = held
            .build_binding(
                "coherent-original",
                &request,
                &review.review_sha256,
                &lanes("coherent"),
            )
            .unwrap();
        for attack in 0..3 {
            let mut false_review = review.clone();
            let mut false_binding = binding.clone();
            match attack {
                0 => {
                    // The real renderer/writer recorded key60. A caller can
                    // rehash key61 and keep the portable bijection consistent.
                    assert_eq!(false_review.notes[0].values.pitch, 60);
                    false_review.notes[0].values.pitch = 61;
                    false_binding.lanes[0].original_notes[0].values.pitch = 61;
                }
                1 => false_review.request.source.descriptor_sha256 = "0".repeat(64),
                2 => {
                    false_review.notes[0].identity.voice += 1000;
                    let mut hash = Sha256::new();
                    hash.update(b"OmniuSong.performance-projected-note.v1\0");
                    hash.update(serde_json::to_vec(&false_review.notes[0].identity).unwrap());
                    let identity = format!("{:x}", hash.finalize());
                    false_review.notes[0].identity_sha256 = identity.clone();
                    let original = &mut false_binding.lanes[0].original_notes[0];
                    original.identity_sha256 = identity.clone();
                    original.note_id.0 = document_note_id(&binding.id, &identity);
                }
                _ => unreachable!(),
            }
            rehash_review(&mut false_review);
            retain_review(&mut false_binding, &false_review);
            false_binding.validate().unwrap();
            checked_encoding(&false_binding).expect("Canonical/digest/bijection checking must not be described as source authentication");
            invalid(held.validate_binding(&false_binding));
        }
        held.validate_binding(&binding).unwrap();
        held.verify_unchanged().unwrap();
    }
    assert_eq!(fixture.bytes(), original);
}

#[test]
fn current_musical_edits_and_deleted_lanes_do_not_rewrite_or_discard_the_original_binding() {
    let fixture = source(true);
    let request = fixture.request();
    let original = fixture.bytes();
    {
        let (governor, _) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let review = held.project(&request).unwrap();
        let choices = lanes("editable");
        let binding = held
            .build_binding(
                "retained-original",
                &request,
                &review.review_sha256,
                &choices,
            )
            .unwrap();
        let document = ProjectDocument::new("Edit ordinary notes")
            .unwrap()
            .append_performance_projection(
                &binding,
                &choices,
                NoteTimelinePolicy::PreserveLoop,
                true,
            )
            .unwrap();
        let address = DocumentNoteAddress {
            track_id: binding.lanes[0].track_id.clone(),
            note_id: binding.lanes[0].original_notes[0].note_id.0.clone(),
        };
        let mut current_values = binding.lanes[0].original_notes[0].values;
        current_values.pitch = 72;
        current_values.start_tick += 240;
        current_values.velocity16 = 12345;
        let edited = document
            .update_note(&address, current_values, NoteTimelinePolicy::PreserveLoop)
            .unwrap();
        let mut edited_json = edited.value().clone();
        edited_json["tempo"] = json!(127.75);
        let track = edited_json["tracks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|track| track["id"] == address.track_id)
            .unwrap();
        track["instrument"] = "pad".into();
        let edited = ProjectDocument::from_value(edited_json).unwrap();
        assert_eq!(
            edited.performance_projections().unwrap().unwrap().entries,
            vec![binding.clone()]
        );
        held.validate_binding(&edited.performance_projections().unwrap().unwrap().entries[0])
            .unwrap();
        let deleted = edited.delete_note(&address).unwrap();
        let mut value = deleted.value().clone();
        value["tracks"]
            .as_array_mut()
            .unwrap()
            .retain(|track| !choices.iter().any(|lane| track["id"] == lane.track_id));
        let without_lanes = ProjectDocument::from_value(value).unwrap();
        assert_eq!(
            without_lanes
                .performance_projections()
                .unwrap()
                .unwrap()
                .entries,
            vec![binding.clone()]
        );
        assert_eq!(
            without_lanes.referenced_takes().unwrap(),
            [binding.source.clone()].into()
        );
        assert!(
            without_lanes
                .projection_track_id_reserved(&address.track_id)
                .unwrap()
        );
        assert!(
            without_lanes
                .projection_note_id_reserved(&address.note_id)
                .unwrap()
        );
        invalid_core(without_lanes.append_performance_projection(
            &binding,
            &choices,
            NoteTimelinePolicy::PreserveLoop,
            false,
        ));
        // A different projection ID still cannot steal a deleted original's
        // reserved lane identity, even though its deterministic note IDs differ.
        let replacement = held
            .build_binding(
                "another-original",
                &request,
                &review.review_sha256,
                &choices,
            )
            .unwrap();
        invalid_core(without_lanes.append_performance_projection(
            &replacement,
            &choices,
            NoteTimelinePolicy::PreserveLoop,
            false,
        ));
        held.validate_binding(&binding).unwrap();
        held.verify_unchanged().unwrap();
    }
    assert_eq!(fixture.bytes(), original);
}

#[test]
fn multiple_derivations_share_one_governor_and_keep_distinct_projection_and_note_identities() {
    let fixture = source(false);
    let request = fixture.request();
    let original = fixture.bytes();
    {
        let (free_budget, _) = budget(InspectionLimits::default());
        let free_reader = SealedJournal::open_bounded(&fixture.path, &free_budget).unwrap();
        let before_free = free_budget.usage().work_bytes;
        let free = project_sealed_performance_bounded(&free_reader, &request).unwrap();
        let free_work = free_budget.usage().work_bytes - before_free;
        let (governor, _) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let authenticated = governor.usage();
        let first_review = held.project(&request).unwrap();
        let first_pass = governor.usage();
        assert_eq!(first_review, free);
        assert!(
            first_pass.work_bytes - authenticated.work_bytes < free_work,
            "An existing authenticated context must avoid the wrapper's additional full-source authentication work"
        );
        assert_eq!(first_pass.file_bytes, authenticated.file_bytes);
        let first_lanes = lanes("first-owned");
        let first = held
            .build_binding(
                "first-projection",
                &request,
                &first_review.review_sha256,
                &first_lanes,
            )
            .unwrap();
        let after_first = governor.usage();
        let mut later = request.clone();
        later.destination_start_tick += 2000;
        let second_review = held.project(&later).unwrap();
        let second_lanes = lanes("second-owned");
        let second = held
            .build_binding(
                "second-projection",
                &later,
                &second_review.review_sha256,
                &second_lanes,
            )
            .unwrap();
        let after_second = governor.usage();
        assert!(
            authenticated.work_bytes < after_first.work_bytes
                && after_first.work_bytes < after_second.work_bytes
        );
        assert!(after_first.record_visits < after_second.record_visits);
        assert_eq!(after_second.file_bytes, authenticated.file_bytes);
        let first_ids: std::collections::BTreeSet<_> = first
            .lanes
            .iter()
            .flat_map(|lane| &lane.original_notes)
            .map(|note| note.note_id.0.as_str())
            .collect();
        assert!(
            second
                .lanes
                .iter()
                .flat_map(|lane| &lane.original_notes)
                .all(|note| !first_ids.contains(note.note_id.0.as_str()))
        );
        let document = ProjectDocument::new("Two retained projections")
            .unwrap()
            .append_performance_projection(
                &first,
                &first_lanes,
                NoteTimelinePolicy::PreserveLoop,
                true,
            )
            .unwrap()
            .append_performance_projection(
                &second,
                &second_lanes,
                NoteTimelinePolicy::PreserveLoop,
                false,
            )
            .unwrap();
        assert_eq!(
            document.performance_projections().unwrap().unwrap().entries,
            vec![first.clone(), second.clone()]
        );
        assert_eq!(document.referenced_takes().unwrap().len(), 1);
        assert_eq!(document.revision(), 0);
        held.validate_binding(&first).unwrap();
        held.validate_binding(&second).unwrap();
        held.verify_unchanged().unwrap();
    }
    assert_eq!(fixture.bytes(), original);
}

#[test]
fn held_binding_operations_keep_cancellation_and_work_exhaustion_sticky_including_bad_requests() {
    let fixture = source(true);
    let request = fixture.request();
    let original = fixture.bytes();
    for cancel in [true, false] {
        let limits = InspectionLimits::default();
        let (governor, cancellation) = budget(limits);
        let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let review = held.project(&request).unwrap();
        let choices = lanes("stopped");
        let binding = held
            .build_binding("stopped-binding", &request, &review.review_sha256, &choices)
            .unwrap();
        if cancel {
            cancellation.store(true, Ordering::Release);
        } else {
            governor
                .charge_work(limits.max_work_bytes - governor.usage().work_bytes)
                .unwrap();
        }
        let mut malformed = request.clone();
        malformed.version = 2;
        let mut malformed_binding = binding.clone();
        malformed_binding.version = 2;
        let results = [
            held.project(&malformed).map(|_| ()),
            held.build_binding("stopped-binding", &malformed, "invalid", &[])
                .map(|_| ()),
            held.validate_binding(&malformed_binding),
            held.verify_unchanged(),
            check_retained_binding_encoding_bounded(&malformed_binding, &governor),
            governor.checkpoint(),
        ];
        for result in results {
            if cancel {
                assert!(
                    matches!(result, Err(RecordingError::InspectionCancelled)),
                    "Cancellation lost: {result:?}"
                );
            } else {
                assert!(
                    matches!(
                        result,
                        Err(RecordingError::InspectionLimit {
                            resource: InspectionResource::WorkBytes,
                            ..
                        })
                    ),
                    "Work stop lost: {result:?}"
                );
            }
        }
    }
    assert_eq!(fixture.bytes(), original);
}

#[test]
fn held_authentication_requires_bounded_source_and_preserves_typed_file_limits() {
    let fixture = source(false);
    let original = fixture.bytes();
    {
        let unbounded = SealedJournal::open(&fixture.path).unwrap();
        assert!(matches!(
            AuthenticatedPerformance::open(&unbounded),
            Err(RecordingError::Invalid(_))
        ));
    }
    let length = fs::metadata(&fixture.path).unwrap().len();
    let (governor, _) = budget(InspectionLimits {
        max_file_bytes: length - 1,
        ..Default::default()
    });
    assert!(matches!(
        SealedJournal::open_bounded(&fixture.path, &governor),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::FileBytes,
            ..
        })
    ));
    assert!(matches!(
        governor.checkpoint(),
        Err(RecordingError::InspectionLimit {
            resource: InspectionResource::FileBytes,
            ..
        })
    ));
    assert_eq!(fixture.bytes(), original);
}

#[cfg(unix)]
#[test]
fn replacing_the_reviewed_path_with_identical_bytes_does_not_replace_held_source_authority() {
    let fixture = source(true);
    let request = fixture.request();
    let original = fs::read(&fixture.path).unwrap();
    let parked = fixture.root.join("held-original-parked.sqlite3");
    let after_replacement;
    {
        let (governor, _) = budget(InspectionLimits::default());
        let reader = SealedJournal::open_bounded(&fixture.path, &governor).unwrap();
        let held = AuthenticatedPerformance::open(&reader).unwrap();
        let review = held.project(&request).unwrap();
        let choices = lanes("replaced");
        let binding = held
            .build_binding("replaced-path", &request, &review.review_sha256, &choices)
            .unwrap();
        fs::rename(&fixture.path, &parked).unwrap();
        fs::write(&fixture.path, &original).unwrap();
        after_replacement = fixture.bytes();
        invalid(held.verify_unchanged());
        invalid(held.project(&request));
        invalid(held.build_binding("replaced-path", &request, &review.review_sha256, &choices));
        invalid(held.validate_binding(&binding));
    }
    assert_eq!(fs::read(&parked).unwrap(), original);
    assert_eq!(fs::read(&fixture.path).unwrap(), original);
    assert_eq!(
        fixture.bytes(),
        after_replacement,
        "All refusal paths preserve both the real held original and the replacement"
    );
}

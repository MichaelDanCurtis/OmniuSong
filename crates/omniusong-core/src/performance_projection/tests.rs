use super::*;
use crate::contracts::TakeId;
use crate::{DocumentNoteAddress, NoteTimelinePolicy};
use serde_json::json;

fn values() -> NoteValues {
    NoteValues {
        pitch: 60,
        start_tick: 120,
        duration_ticks: 960,
        velocity16: 0,
    }
}
fn binding() -> PerformanceProjectionBinding {
    let source = TakeReference {
        take_id: TakeId("take-original".into()),
        manifest_sha256: "a".repeat(64),
        stream_id: "audio".into(),
    };
    let hash = "b".repeat(64);
    let canonical_json = json!({"version":1,"review_sha256":hash,"request":{"source":{"take_id":source.take_id.0,"manifest_sha256":source.manifest_sha256}},"notes":[],"foreign:exact":{"integer":18446744073709551615u64}}).to_string();
    PerformanceProjectionBinding {
        id: "projection-one".into(),
        version: 1,
        source,
        review: RetainedProjectionReview {
            version: 1,
            canonical_json,
            review_sha256: hash,
        },
        lanes: vec![ProjectionLaneBinding {
            source: 1,
            track_id: "projected-lane-a".into(),
            instrument: ProjectionInstrument::Keys,
            original_notes: vec![ProjectionNoteBinding {
                note_id: NoteId("original-a".into()),
                identity_sha256: "c".repeat(64),
                values: values(),
            }],
        }],
    }
}
fn registry() -> PerformanceProjectionRegistry {
    PerformanceProjectionRegistry {
        version: 1,
        entries: vec![binding()],
    }
}
fn encoded_registry() -> Value {
    serde_json::to_value(registry()).unwrap()
}
fn note(id: &str) -> Value {
    json!({"id":id,"pitch":60,"startTick":120,"durationTicks":960,"velocity16":0,"expression":[],"vendor:note":{"retain":true}})
}
fn project_value() -> Value {
    let mut value = ProjectDocument::new("Retained original interpretation")
        .unwrap()
        .value()
        .clone();
    value["schemaVersion"] = PROJECTION_SCHEMA_VERSION.into();
    value["performanceProjections"] = encoded_registry();
    value["tracks"][0]["id"] = "projected-lane-a".into();
    value["tracks"][0]["notes"] = json!([note("original-a")]);
    value["vendor:exact"] = serde_json::from_str(r#"{"integer":18446744073709551616000001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
    value
}
fn project() -> ProjectDocument {
    ProjectDocument::from_value(project_value()).unwrap()
}

#[test]
fn strict_owned_versions_unknown_fields_identifiers_and_values_refuse() {
    registry().validate().unwrap();
    let base = encoded_registry();
    for level in 0..3 {
        let mut bad = base.clone();
        match level {
            0 => bad["version"] = 2.into(),
            1 => bad["entries"][0]["version"] = 2.into(),
            _ => bad["entries"][0]["review"]["version"] = 2.into(),
        }
        assert!(registry_from_value(&bad).is_err());
    }
    for level in 0..6 {
        let mut bad = base.clone();
        let owner = match level {
            0 => &mut bad,
            1 => &mut bad["entries"][0],
            2 => &mut bad["entries"][0]["review"],
            3 => &mut bad["entries"][0]["lanes"][0],
            4 => &mut bad["entries"][0]["lanes"][0]["originalNotes"][0],
            _ => &mut bad["entries"][0]["lanes"][0]["originalNotes"][0]["values"],
        };
        owner["unknownOwnedField"] = true.into();
        assert!(registry_from_value(&bad).is_err());
    }
    let cases = [
        ("id", json!("bad @ projection")),
        ("id", json!("p".repeat(97))),
    ];
    for (key, value) in cases {
        let mut bad = base.clone();
        bad["entries"][0][key] = value;
        assert!(registry_from_value(&bad).is_err());
    }
    for source in [0, 5] {
        let mut bad = base.clone();
        bad["entries"][0]["lanes"][0]["source"] = source.into();
        assert!(registry_from_value(&bad).is_err());
    }
    let mut bad = base.clone();
    bad["entries"][0]["lanes"][0]["trackId"] = "unicode-🎹".into();
    assert!(registry_from_value(&bad).is_err());
    let mut bad = base.clone();
    bad["entries"][0]["lanes"][0]["instrument"] = "voice".into();
    assert!(registry_from_value(&bad).is_err());
    let mut bad = base.clone();
    bad["entries"][0]["lanes"][0]["originalNotes"][0]["noteId"] = "".into();
    assert!(registry_from_value(&bad).is_err());
    let mut bad = base.clone();
    bad["entries"][0]["lanes"][0]["originalNotes"][0]["identitySha256"] = "C".repeat(64).into();
    assert!(registry_from_value(&bad).is_err());
    let mut bad = base;
    bad["entries"][0]["lanes"][0]["originalNotes"][0]["values"]["duration_ticks"] = 0.into();
    assert!(registry_from_value(&bad).is_err());
}

#[test]
fn thin_retained_envelope_binds_take_manifest_and_embedded_digest_without_claiming_semantics() {
    let original = binding();
    for replacement in [
        json!(null),
        json!([]),
        json!({"version":2,"review_sha256":original.review.review_sha256,"request":{"source":{"take_id":"take-original","manifest_sha256":"a".repeat(64)}}}),
    ] {
        let mut bad = original.clone();
        bad.review.canonical_json = replacement.to_string();
        assert!(bad.validate().is_err());
    }
    for (take, manifest, hash) in [
        ("another-take".to_owned(), "a".repeat(64), "b".repeat(64)),
        ("take-original".to_owned(), "c".repeat(64), "b".repeat(64)),
        ("take-original".to_owned(), "a".repeat(64), "d".repeat(64)),
    ] {
        let mut bad = original.clone();
        bad.review.canonical_json = json!({"version":1,"review_sha256":hash,"request":{"source":{"take_id":take,"manifest_sha256":manifest}}}).to_string();
        assert!(bad.validate().is_err());
    }
    let mut bad = original.clone();
    bad.source.stream_id = "gui-1".into();
    assert!(bad.validate().is_err());
    let mut bad = original.clone();
    bad.review.review_sha256 = "B".repeat(64);
    assert!(bad.validate().is_err());
    let mut bad = original.clone();
    bad.review.canonical_json.push_str("{}");
    assert!(bad.validate().is_err());

    let mut retained = original;
    // Core deliberately accepts a shape-only envelope. The source interpreter
    // must subsequently reject these noncanonical/malformed musical details.
    retained.review.canonical_json = format!(
        " \n {{\"version\":1,\"review_sha256\":\"{}\",\"request\":{{\"source\":{{\"take_id\":\"take-original\",\"manifest_sha256\":\"{}\"}}}},\"notes\":\"shared-layer-only\",\"foreign\":184467440737095516160001}} \n",
        "b".repeat(64),
        "a".repeat(64)
    );
    let exact = retained.review.canonical_json.clone();
    retained.validate().unwrap();
    let encoded = serde_json::to_value(&retained).unwrap();
    let decoded: PerformanceProjectionBinding = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.review.canonical_json, exact);
}

#[test]
fn review_utf8_bytes_and_array_capacities_are_checked_before_expensive_projection() {
    let original = binding();
    let mut exact = original.review.clone();
    exact
        .canonical_json
        .push_str(&" ".repeat(MAX_RETAINED_PROJECTION_REVIEW_BYTES - exact.canonical_json.len()));
    exact.validate_for(&original.source).unwrap();
    exact.canonical_json.push(' ');
    assert!(exact.validate_for(&original.source).is_err());
    let mut unicode = original.review.clone();
    unicode.canonical_json = "🎹".repeat(MAX_RETAINED_PROJECTION_REVIEW_BYTES / 4 + 1);
    assert!(unicode.validate_for(&original.source).is_err());
    let mut empty = registry();
    empty.entries[0].lanes.clear();
    assert!(empty.validate().is_err());
    let mut empty = registry();
    empty.entries[0].lanes[0].original_notes.clear();
    assert!(empty.validate().is_err());
    let mut max = PerformanceProjectionRegistry {
        version: 1,
        entries: Vec::new(),
    };
    for index in 0..MAX_PERFORMANCE_PROJECTION_ENTRIES {
        let mut entry = binding();
        entry.id = format!("projection-{index}");
        entry.lanes[0].track_id = format!("lane-{index}");
        entry.lanes[0].original_notes[0].note_id = NoteId(format!("note-{index}"));
        max.entries.push(entry);
    }
    max.validate().unwrap();
    max.entries.push(binding());
    assert!(max.validate().is_err());

    let instruments = [
        ProjectionInstrument::Keys,
        ProjectionInstrument::Bass,
        ProjectionInstrument::Drums,
        ProjectionInstrument::Pad,
    ];
    let mut entry = binding();
    entry.lanes.clear();
    for source in 1..=4 {
        for (index, instrument) in instruments.iter().enumerate() {
            entry.lanes.push(ProjectionLaneBinding {
                source,
                instrument: *instrument,
                track_id: format!("lane-{source}-{index}"),
                original_notes: vec![ProjectionNoteBinding {
                    note_id: NoteId(format!("note-{source}-{index}")),
                    identity_sha256: format!("{:064x}", source as usize * 4 + index),
                    values: values(),
                }],
            });
        }
    }
    entry.validate().unwrap();
    entry.lanes.push(binding().lanes.remove(0));
    assert!(entry.validate().is_err());
}

#[test]
fn twenty_thousand_original_note_limit_is_aggregate_and_voice_identity_is_per_entry() {
    let mut first = binding();
    first.lanes[0].original_notes.clear();
    for index in 0..MAX_ORIGINAL_PROJECTION_NOTES / 2 {
        first.lanes[0].original_notes.push(ProjectionNoteBinding {
            note_id: NoteId(format!("a-{index}")),
            identity_sha256: format!("{index:064x}"),
            values: values(),
        });
    }
    let mut second = first.clone();
    second.id = "projection-two".into();
    second.lanes[0].track_id = "projected-lane-b".into();
    for (index, note) in second.lanes[0].original_notes.iter_mut().enumerate() {
        note.note_id = NoteId(format!("b-{index}"));
    }
    // Complete original voice digests can recur in separately retained
    // interpretations, but reserved document note IDs cannot.
    let mut max = PerformanceProjectionRegistry {
        version: 1,
        entries: vec![first, second],
    };
    max.validate().unwrap();
    max.entries[1].lanes[0]
        .original_notes
        .push(ProjectionNoteBinding {
            note_id: NoteId("one-too-many".into()),
            identity_sha256: "f".repeat(64),
            values: values(),
        });
    assert!(max.validate().is_err());
    let mut too_many = binding();
    too_many.lanes[0].original_notes =
        vec![too_many.lanes[0].original_notes[0].clone(); MAX_ORIGINAL_PROJECTION_NOTES + 1];
    assert!(too_many.validate().is_err());
}

#[test]
fn projection_lane_and_note_names_are_reserved_across_the_entire_registry() {
    let first = binding();
    let mut second = first.clone();
    second.id = "projection-two".into();
    second.lanes[0].track_id = "projected-lane-b".into();
    second.lanes[0].original_notes[0].note_id = NoteId("original-b".into());
    let good = PerformanceProjectionRegistry {
        version: 1,
        entries: vec![first.clone(), second.clone()],
    };
    good.validate().unwrap();
    let mut bad = good.clone();
    bad.entries[1].id = first.id.clone();
    assert!(bad.validate().is_err());
    let mut bad = good.clone();
    bad.entries[1].lanes[0].track_id = first.lanes[0].track_id.clone();
    assert!(bad.validate().is_err());
    let mut bad = good;
    bad.entries[1].lanes[0].original_notes[0].note_id =
        first.lanes[0].original_notes[0].note_id.clone();
    assert!(bad.validate().is_err());
    let mut duplicate_lane = first;
    let mut other = second.lanes.remove(0);
    other.original_notes[0].identity_sha256 = "d".repeat(64);
    duplicate_lane.lanes.push(other);
    assert!(duplicate_lane.validate().is_err());
    duplicate_lane.lanes[1].instrument = ProjectionInstrument::Bass;
    duplicate_lane.validate().unwrap();
    duplicate_lane.lanes[1].original_notes[0].identity_sha256 = "c".repeat(64);
    assert!(duplicate_lane.validate().is_err());
}

#[test]
fn current_note_edits_and_track_instrument_changes_preserve_original_binding_exactly() {
    let source = project();
    let original = source.value()["performanceProjections"].clone();
    let address = DocumentNoteAddress {
        track_id: "projected-lane-a".into(),
        note_id: "original-a".into(),
    };
    let changed = NoteValues {
        pitch: 72,
        start_tick: 333,
        duration_ticks: 1440,
        velocity16: 54321,
    };
    let edited = source
        .update_note(&address, changed, NoteTimelinePolicy::PreserveLoop)
        .unwrap();
    assert_eq!(edited.value()["performanceProjections"], original);
    assert_eq!(
        edited.value()["tracks"][0]["notes"][0]["vendor:note"],
        source.value()["tracks"][0]["notes"][0]["vendor:note"]
    );
    let mut value = edited.value().clone();
    value["tracks"][0]["instrument"] = "pad".into();
    value["tracks"][0]["name"] = "My edited synth".into();
    let changed_track = ProjectDocument::from_value(value).unwrap();
    assert_eq!(changed_track.value()["performanceProjections"], original);
    let restored = changed_track
        .restore_note(&address, NoteTimelinePolicy::PreserveLoop)
        .unwrap();
    assert_eq!(restored.value()["performanceProjections"], original);
    assert_eq!(restored.value()["tracks"][0]["notes"][0]["pitch"], 60);
    assert_eq!(restored.value()["tracks"][0]["instrument"], "pad");
    assert_eq!(source.value()["tracks"][0]["notes"][0]["pitch"], 60);
}

#[test]
fn source_ownership_and_all_reservations_survive_note_and_whole_lane_deletion() {
    let source = project();
    let address = DocumentNoteAddress {
        track_id: "projected-lane-a".into(),
        note_id: "original-a".into(),
    };
    let removed = source.delete_note(&address).unwrap();
    assert!(removed.projection_note_id_reserved("original-a").unwrap());
    assert!(
        removed
            .projection_track_id_reserved("projected-lane-a")
            .unwrap()
    );
    assert!(!removed.projection_note_id_reserved("unrelated").unwrap());
    assert!(
        removed
            .add_note(&address, values(), NoteTimelinePolicy::PreserveLoop)
            .is_err()
    );
    let other_lane = DocumentNoteAddress {
        track_id: removed.tracks()[1]["id"].as_str().unwrap().into(),
        note_id: "original-a".into(),
    };
    assert!(
        removed
            .add_note(&other_lane, values(), NoteTimelinePolicy::PreserveLoop)
            .is_err()
    );
    let mut value = removed.value().clone();
    value["tracks"].as_array_mut().unwrap().remove(0);
    let removed_lane = ProjectDocument::from_value(value).unwrap();
    assert!(
        removed_lane
            .projection_note_id_reserved("original-a")
            .unwrap()
    );
    assert!(
        removed_lane
            .projection_track_id_reserved("projected-lane-a")
            .unwrap()
    );
    assert_eq!(
        removed_lane.projection_source_references().unwrap(),
        BTreeSet::from([binding().source])
    );
    assert_eq!(
        removed_lane.value()["performanceProjections"],
        source.value()["performanceProjections"]
    );
    assert_eq!(
        removed_lane.value()["vendor:exact"],
        source.value()["vendor:exact"]
    );
}

#[test]
fn reserved_notes_cannot_move_to_other_notes_or_audio_tracks_even_after_original_deletion() {
    for audio in [false, true] {
        let mut value = project_value();
        value["tracks"][0]["notes"] = json!([]);
        if audio {
            value["tracks"][1]["kind"] = "audio".into();
        }
        value["tracks"][1]["notes"] = json!([note("original-a")]);
        assert!(ProjectDocument::from_value(value).is_err());
    }
    let mut wrong_kind = project_value();
    wrong_kind["tracks"][0]["kind"] = "audio".into();
    assert!(ProjectDocument::from_value(wrong_kind).is_err());
    let mut missing_lane = project_value();
    missing_lane["tracks"].as_array_mut().unwrap().remove(0);
    missing_lane["tracks"][0]["notes"] = json!([note("original-a")]);
    assert!(ProjectDocument::from_value(missing_lane).is_err());
    let mut unrelated = project_value();
    unrelated["tracks"][1]["notes"] = json!([note("ordinary-note")]);
    assert!(ProjectDocument::from_value(unrelated).is_ok());
}

#[test]
fn recording_track_opaque_notes_do_not_claim_reservations_but_cannot_reuse_a_lane_id() {
    let mut value = project_value();
    value["tracks"][1]["kind"] = "recording".into();
    value["tracks"][1]["recording"] = json!({"version":1,"clips":[]});
    value["tracks"][1]["notes"] = json!([note("original-a")]);
    let retained = ProjectDocument::from_value(value.clone()).unwrap();
    assert_eq!(
        retained.value()["tracks"][1]["notes"],
        value["tracks"][1]["notes"]
    );
    value["tracks"][1]["id"] = "projected-lane-a".into();
    value["tracks"].as_array_mut().unwrap().remove(0);
    assert!(ProjectDocument::from_value(value).is_err());
}

#[test]
fn explicit_schema_promotion_preserves_legacy_values_and_refuses_any_opaque_registry_collision() {
    for native in [false, true] {
        let mut value = ProjectDocument::new("Promotion source")
            .unwrap()
            .value()
            .clone();
        if native {
            value["schemaVersion"] = 2.into();
        }
        value["vendor:exact"] = serde_json::from_str(r#"{"counter":184467440737095516160001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
        let original = ProjectDocument::from_value(value.clone()).unwrap();
        assert!(original.performance_projections().unwrap().is_none());
        assert!(original.projection_source_references().unwrap().is_empty());
        let promoted = original.upgrade_performance_projection_schema().unwrap();
        assert_eq!(promoted.schema_version(), PROJECTION_SCHEMA_VERSION);
        assert_eq!(
            promoted.performance_projections().unwrap(),
            Some(PerformanceProjectionRegistry {
                version: 1,
                entries: Vec::new()
            })
        );
        let mut expected = value.clone();
        expected["schemaVersion"] = 3.into();
        expected["performanceProjections"] = json!({"version":1,"entries":[]});
        assert_eq!(promoted.value(), &expected);
        assert_eq!(original.value(), &value);
        assert!(promoted.upgrade_performance_projection_schema().is_err());
        for opaque in [
            Value::Null,
            json!({"version":1,"entries":[]}),
            json!({"foreign":"retained"}),
            json!("foreign"),
            json!([]),
        ] {
            let mut collision = value.clone();
            collision["performanceProjections"] = opaque;
            let source = ProjectDocument::from_value(collision.clone()).unwrap();
            assert!(source.performance_projections().unwrap().is_none());
            assert!(!source.projection_note_id_reserved("original-a").unwrap());
            assert!(source.upgrade_performance_projection_schema().is_err());
            assert_eq!(source.value(), &collision);
        }
    }
    let mut missing = project_value();
    missing
        .as_object_mut()
        .unwrap()
        .remove("performanceProjections");
    assert!(ProjectDocument::from_value(missing).is_err());
    let mut null = project_value();
    null["performanceProjections"] = Value::Null;
    assert!(ProjectDocument::from_value(null).is_err());
}

fn lane_choices(binding: &PerformanceProjectionBinding) -> Vec<ProjectionLaneChoice> {
    binding
        .lanes
        .iter()
        .map(|lane| ProjectionLaneChoice {
            source: lane.source,
            instrument: lane.instrument,
            track_id: lane.track_id.clone(),
            track_name: "Editable interpretation 🎹".into(),
        })
        .collect()
}

// These fixtures intentionally exercise only the complete document constructor.
// Their thin retained envelope grants no shared-layer or store authority.
#[test]
fn complete_candidate_preserves_authored_values_and_creates_muted_ordinary_lanes() {
    for schema in [1, 2, 3] {
        let mut original_value = ProjectDocument::new("Authored song")
            .unwrap()
            .value()
            .clone();
        original_value["schemaVersion"] = schema.into();
        original_value["revision"] = 42.into();
        original_value["vendor:exact"] = project_value()["vendor:exact"].clone();
        if schema == 3 {
            original_value["performanceProjections"] = json!({"version":1,"entries":[]});
        }
        let original = ProjectDocument::from_value(original_value.clone()).unwrap();
        let binding = binding();
        let choices = lane_choices(&binding);
        let result = original
            .append_performance_projection(
                &binding,
                &choices,
                NoteTimelinePolicy::PreserveLoop,
                true,
            )
            .unwrap();
        assert_eq!(original.value(), &original_value);
        assert_eq!(result.id(), original.id());
        assert_eq!(result.revision(), 42);
        assert_eq!(result.schema_version(), 3);
        assert_eq!(result.bars(), original.bars());
        assert_eq!(
            &result.tracks()[..original.tracks().len()],
            original.tracks()
        );
        assert_eq!(
            result.value()["vendor:exact"],
            original_value["vendor:exact"]
        );
        assert_eq!(
            result.performance_projections().unwrap().unwrap().entries,
            vec![binding.clone()]
        );
        assert_eq!(
            result.referenced_takes().unwrap(),
            BTreeSet::from([binding.source])
        );
        let track = result.tracks().last().unwrap();
        assert_eq!(track["kind"], "notes");
        assert_eq!(track["name"], choices[0].track_name);
        assert_eq!(track["instrument"], "keys");
        assert_eq!(track["muted"], true);
        assert_eq!(track["armed"], false);
        assert_eq!(track["locked"], false);
        assert_eq!(
            track["notes"][0],
            json!({
                "id":"original-a","pitch":60,"startTick":120,
                "durationTicks":960,"velocity16":0,"expression":[]
            })
        );
        let mut expected = original_value;
        expected["schemaVersion"] = 3.into();
        expected["performanceProjections"] = result.value()["performanceProjections"].clone();
        expected["tracks"]
            .as_array_mut()
            .unwrap()
            .push(track.clone());
        assert_eq!(
            result.value(),
            &expected,
            "Every unrelated value remains exact"
        );
    }
}

#[test]
fn complete_candidate_requires_explicit_promotion_and_retains_opaque_collisions() {
    let binding = binding();
    let choices = lane_choices(&binding);
    for schema in [1, 2] {
        let mut value = ProjectDocument::new("No automatic promotion")
            .unwrap()
            .value()
            .clone();
        value["schemaVersion"] = schema.into();
        let source = ProjectDocument::from_value(value.clone()).unwrap();
        assert!(
            source
                .append_performance_projection(
                    &binding,
                    &choices,
                    NoteTimelinePolicy::PreserveLoop,
                    false
                )
                .is_err()
        );
        assert_eq!(source.value(), &value);
        for opaque in [
            Value::Null,
            json!({"version":1,"entries":[]}),
            json!("foreign"),
        ] {
            let mut collision = value.clone();
            collision["performanceProjections"] = opaque;
            let source = ProjectDocument::from_value(collision.clone()).unwrap();
            assert!(
                source
                    .append_performance_projection(
                        &binding,
                        &choices,
                        NoteTimelinePolicy::PreserveLoop,
                        true
                    )
                    .is_err()
            );
            assert_eq!(source.value(), &collision);
        }
    }
}

#[test]
fn complete_candidate_refuses_existing_and_deleted_reserved_identities() {
    let original = ProjectDocument::new("Existing identities").unwrap();
    let mut original_value = original.value().clone();
    original_value["tracks"][0]["notes"] = json!([note("original-a")]);
    let source = ProjectDocument::from_value(original_value.clone()).unwrap();
    let mut proposed = binding();
    assert!(
        source
            .append_performance_projection(
                &proposed,
                &lane_choices(&proposed),
                NoteTimelinePolicy::PreserveLoop,
                true
            )
            .is_err()
    );
    assert_eq!(source.value(), &original_value);
    proposed.lanes[0].original_notes[0].note_id = NoteId("new-projected-note".into());
    proposed.lanes[0].track_id = original.tracks()[0]["id"].as_str().unwrap().into();
    assert!(
        source
            .append_performance_projection(
                &proposed,
                &lane_choices(&proposed),
                NoteTimelinePolicy::PreserveLoop,
                true
            )
            .is_err()
    );

    let source = project();
    let mut without_lanes = source.value().clone();
    without_lanes["tracks"].as_array_mut().unwrap().remove(0);
    let source = ProjectDocument::from_value(without_lanes.clone()).unwrap();
    proposed = binding();
    proposed.id = "second-projection".into();
    proposed.lanes[0].original_notes[0].note_id = NoteId("new-projected-note".into());
    assert!(
        source
            .append_performance_projection(
                &proposed,
                &lane_choices(&proposed),
                NoteTimelinePolicy::PreserveLoop,
                false
            )
            .is_err()
    );
    proposed.lanes[0].track_id = "new-lane".into();
    proposed.lanes[0].original_notes[0].note_id = NoteId("original-a".into());
    assert!(
        source
            .append_performance_projection(
                &proposed,
                &lane_choices(&proposed),
                NoteTimelinePolicy::PreserveLoop,
                false
            )
            .is_err()
    );
    proposed.lanes[0].original_notes[0].note_id = NoteId("new-projected-note".into());
    proposed.id = "projection-one".into();
    assert!(
        source
            .append_performance_projection(
                &proposed,
                &lane_choices(&proposed),
                NoteTimelinePolicy::PreserveLoop,
                false
            )
            .is_err()
    );
    assert_eq!(source.value(), &without_lanes);
}

#[test]
fn complete_candidate_requires_exact_ordered_destinations_and_bounded_names() {
    let source = ProjectDocument::new("All lanes or none").unwrap();
    let before = source.value().clone();
    let mut binding = binding();
    let mut second = binding.lanes[0].clone();
    second.instrument = ProjectionInstrument::Bass;
    second.track_id = "projected-lane-b".into();
    second.original_notes[0].note_id = NoteId("original-b".into());
    second.original_notes[0].identity_sha256 = "d".repeat(64);
    binding.lanes.push(second);
    let choices = lane_choices(&binding);
    source
        .append_performance_projection(&binding, &choices, NoteTimelinePolicy::PreserveLoop, true)
        .unwrap();
    for bad in [
        vec![],
        choices[..1].to_vec(),
        vec![choices[1].clone(), choices[0].clone()],
    ] {
        assert!(
            source
                .append_performance_projection(
                    &binding,
                    &bad,
                    NoteTimelinePolicy::PreserveLoop,
                    true
                )
                .is_err()
        );
    }
    for field in ["source", "instrument", "track"] {
        let mut bad = choices.clone();
        match field {
            "source" => bad[0].source = 0,
            "instrument" => bad[0].instrument = ProjectionInstrument::Drums,
            _ => bad[0].track_id = "another-destination".into(),
        }
        assert!(
            source
                .append_performance_projection(
                    &binding,
                    &bad,
                    NoteTimelinePolicy::PreserveLoop,
                    true
                )
                .is_err()
        );
    }
    for name in [" ".into(), "embedded\ncontrol".into(), "🎹".repeat(161)] {
        let mut bad = choices.clone();
        bad[0].track_name = name;
        assert!(
            source
                .append_performance_projection(
                    &binding,
                    &bad,
                    NoteTimelinePolicy::PreserveLoop,
                    true
                )
                .is_err()
        );
    }
    let mut exact = choices.clone();
    exact[0].track_name = "🎹".repeat(160);
    source
        .append_performance_projection(&binding, &exact, NoteTimelinePolicy::PreserveLoop, true)
        .unwrap();
    assert_eq!(source.value(), &before);
}

#[test]
fn complete_candidate_loop_policy_never_trims_and_refuses_excess_native_duration() {
    let source = ProjectDocument::new("Explicit loop policy").unwrap();
    let before = source.value().clone();
    let mut binding = binding();
    binding.lanes[0].original_notes[0].values.start_tick = crate::PPQ * 4 * (source.bars() + 2) - 1;
    binding.lanes[0].original_notes[0].values.duration_ticks = 2;
    let choices = lane_choices(&binding);
    let preserve = source
        .append_performance_projection(&binding, &choices, NoteTimelinePolicy::PreserveLoop, true)
        .unwrap();
    assert_eq!(preserve.bars(), source.bars());
    let extend = NoteTimelinePolicy::ExtendLoop {
        allow_native_schema_upgrade: false,
    };
    let longer = source
        .append_performance_projection(&binding, &choices, extend, true)
        .unwrap();
    assert_eq!(longer.bars(), source.bars() + 3);
    binding.lanes[0].original_notes[0].values = values();
    assert_eq!(
        source
            .append_performance_projection(&binding, &choices, extend, true)
            .unwrap()
            .bars(),
        source.bars()
    );
    binding.lanes[0].original_notes[0].values.start_tick =
        crate::native::MAX_NATIVE_BARS * crate::PPQ * 4;
    assert!(
        source
            .append_performance_projection(&binding, &choices, extend, true)
            .is_err()
    );
    assert_eq!(source.value(), &before);
}

#[test]
fn complete_candidate_refuses_track_capacity_before_returning_any_partial_result() {
    let mut value = ProjectDocument::new("Track capacity")
        .unwrap()
        .value()
        .clone();
    let template = value["tracks"][0].clone();
    value["tracks"] = Value::Array(
        (0..64)
            .map(|index| {
                let mut track = template.clone();
                track["id"] = format!("authored-{index}").into();
                track
            })
            .collect(),
    );
    let source = ProjectDocument::from_value(value.clone()).unwrap();
    let binding = binding();
    assert!(
        source
            .append_performance_projection(
                &binding,
                &lane_choices(&binding),
                NoteTimelinePolicy::PreserveLoop,
                true
            )
            .is_err()
    );
    assert_eq!(source.value(), &value);
}

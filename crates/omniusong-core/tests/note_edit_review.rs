//! Independent document-note identity, preservation and timeline regressions.
use omniusong_core::{
    DocumentNoteAddress, NoteTimelinePolicy, NoteValues, ProjectDocument,
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId, Validate},
};
use serde_json::{Value, json};

const KEEP: NoteTimelinePolicy = NoteTimelinePolicy::PreserveLoop;
fn values() -> NoteValues {
    NoteValues {
        pitch: 60,
        start_tick: 17,
        duration_ticks: 961,
        velocity16: 32769,
    }
}
fn address(track: &str, note: &str) -> DocumentNoteAddress {
    DocumentNoteAddress {
        track_id: track.into(),
        note_id: note.into(),
    }
}
fn base() -> ProjectDocument {
    let mut value = ProjectDocument::new("Independent note review")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["id"] = "track".into();
    value["tracks"][0]["notes"] = json!([{
        "id":"note", "pitch":60, "startTick":17, "durationTicks":961, "velocity16":32769,
        "input":{"sourceId":"controller", "sourceName":"Exact keys", "protocol":"midi2", "group":15, "channel":15, "receivedTimeMs":1.25, "releaseVelocity16":60001, "keyDurationTicks":300, "endReason":"sustain-release"},
        "model":{"estimate":"not a measured performance"}, "vendor":{"id":"note","takeId":"opaque"}
    }]);
    value["inputCapture"] = json!({"version":1,"droppedEvents":0,"events":[{"trackId":"track","tick":17,"sourceId":"controller","sourceName":"Exact keys","receivedTimeMs":1.25,"hostTime":"18446744073709551615","format":"ump","words":[1083194368u32,4294967295u32]}]});
    value["history"] = json!([{"foreign":"legacy history remains exact"}]);
    value["vendor"] = serde_json::from_str(r#"{"integer":18446744073709551616001,"decimal":0.123456789012345678901234567890123456789}"#).unwrap();
    ProjectDocument::from_value(value).unwrap()
}
fn note(document: &ProjectDocument) -> &Value {
    &document.value()["tracks"][0]["notes"][0]
}

#[test]
fn exact_existing_keys_and_repeated_pitches_select_only_the_addressed_note() {
    let mut value = base().value().clone();
    value["tracks"][0]["id"] = "\0🎹/track".into();
    value["inputCapture"]["events"][0]["trackId"] = "\0🎹/track".into();
    value["tracks"][0]["notes"][0]["id"] = "".into();
    let mut second = value["tracks"][0]["notes"][0].clone();
    second["id"] = "other".into();
    value["tracks"][0]["notes"]
        .as_array_mut()
        .unwrap()
        .push(second.clone());
    let mut other_track = value["tracks"][0].clone();
    other_track["id"] = "different".into();
    value["tracks"]
        .as_array_mut()
        .unwrap()
        .push(other_track.clone());
    let original = ProjectDocument::from_value(value).unwrap();
    let changed = original
        .update_note(
            &address("\0🎹/track", ""),
            NoteValues {
                velocity16: 0,
                ..values()
            },
            KEEP,
        )
        .unwrap();
    assert_eq!(note(&changed)["velocity16"], 0);
    assert_eq!(changed.value()["tracks"][0]["notes"][1], second);
    assert_eq!(
        changed.value()["tracks"]
            .as_array()
            .unwrap()
            .last()
            .unwrap(),
        &other_track
    );
    assert_eq!(
        changed.value()["inputCapture"],
        original.value()["inputCapture"]
    );
    assert_eq!(note(&original)["velocity16"], 32769);
}

#[test]
fn first_snapshot_preserves_absent_null_and_empty_expression_and_restore_keeps_it() {
    for expression in [None, Some(Value::Null), Some(json!([]))] {
        let mut value = base().value().clone();
        if let Some(expression) = expression.clone() {
            value["tracks"][0]["notes"][0]["expression"] = expression;
        }
        let original = ProjectDocument::from_value(value).unwrap();
        let changed = original
            .update_note(
                &address("track", "note"),
                NoteValues {
                    velocity16: 0,
                    ..values()
                },
                KEEP,
            )
            .unwrap();
        let snapshot = note(&changed)["editSource"].clone();
        assert_eq!(snapshot["version"], 1);
        assert_eq!(snapshot.get("expression"), expression.as_ref());
        assert!(snapshot.get("input").is_none());
        let changed_again = changed
            .update_note(
                &address("track", "note"),
                NoteValues {
                    pitch: 127,
                    velocity16: 65535,
                    ..values()
                },
                KEEP,
            )
            .unwrap();
        assert_eq!(note(&changed_again)["editSource"], snapshot);
        let restored = changed_again
            .restore_note(&address("track", "note"), KEEP)
            .unwrap();
        assert_eq!(note(&restored)["editSource"], snapshot);
        let mut restored_note = note(&restored).clone();
        restored_note.as_object_mut().unwrap().remove("editSource");
        assert_eq!(restored_note, *note(&original));
        assert!(
            restored
                .restore_note(&address("track", "note"), KEEP)
                .is_err()
        );
        for key in ["inputCapture", "history", "vendor"] {
            assert_eq!(changed_again.value()[key], original.value()[key]);
        }
        let wire = String::from_utf8(restored.to_json().unwrap()).unwrap();
        assert!(wire.contains("18446744073709551616001"));
        assert!(wire.contains("0.123456789012345678901234567890123456789"));
    }
}

#[test]
fn source_extensions_and_active_relative_curves_survive_move_and_restore() {
    let expression = json!([{"type":"pitch","vendor":"lane","points":[{"tick":0,"value32":2147483648u32},{"tick":961,"value32":4294967295u32,"opaque":"point"}]}]);
    let mut value = base().value().clone();
    value["tracks"][0]["notes"][0]["expression"] = expression.clone();
    value["tracks"][0]["notes"][0]["editSource"] = json!({"version":1,"pitch":50,"startTick":11,"durationTicks":1000,"velocity16":1,"expression":null,"foreign":{"operation_id":"unchanged"}});
    let original = ProjectDocument::from_value(value).unwrap();
    let moved = original
        .update_note(
            &address("track", "note"),
            NoteValues {
                start_tick: 12345,
                ..values()
            },
            KEEP,
        )
        .unwrap();
    assert_eq!(note(&moved)["expression"], expression);
    assert_eq!(note(&moved)["editSource"], note(&original)["editSource"]);
    assert!(
        moved
            .update_note(
                &address("track", "note"),
                NoteValues {
                    duration_ticks: 960,
                    ..values()
                },
                KEEP
            )
            .is_err()
    );
    let restored = moved.restore_note(&address("track", "note"), KEEP).unwrap();
    assert_eq!(note(&restored)["pitch"], 50);
    assert_eq!(note(&restored)["expression"], Value::Null);
    assert_eq!(note(&restored)["editSource"], note(&original)["editSource"]);
    assert_eq!(note(&restored)["input"], note(&original)["input"]);
}

#[test]
fn immutable_source_curve_does_not_constrain_a_different_active_curve() {
    let mut value = base().value().clone();
    value["tracks"][0]["notes"][0]["expression"] = json!([]);
    value["tracks"][0]["notes"][0]["editSource"] = json!({"version":1,"pitch":60,"startTick":17,"durationTicks":10000,"velocity16":100,"expression":[{"type":"pressure","points":[{"tick":10000,"value32":123}]}]});
    let original = ProjectDocument::from_value(value).unwrap();
    let shorter = original
        .update_note(
            &address("track", "note"),
            NoteValues {
                duration_ticks: 1,
                ..values()
            },
            KEEP,
        )
        .unwrap();
    assert_eq!(note(&shorter)["durationTicks"], 1);
    assert_eq!(note(&shorter)["editSource"], note(&original)["editSource"]);
    assert_eq!(
        note(
            &shorter
                .restore_note(&address("track", "note"), KEEP)
                .unwrap()
        )["durationTicks"],
        10000
    );
}

#[test]
fn preserve_loop_accepts_long_existing_notes_and_exact_odd_ticks() {
    let mut value = base().value().clone();
    value["bars"] = 1.into();
    value["tracks"][0]["notes"][0]["startTick"] = 1_000_000_000u64.into();
    value["tracks"][0]["notes"][0]["durationTicks"] = 100_000_000u64.into();
    let original = ProjectDocument::from_value(value).unwrap();
    let changed = original
        .update_note(
            &address("track", "note"),
            NoteValues {
                start_tick: 1_000_000_000,
                duration_ticks: 100_000_000,
                velocity16: 1,
                ..values()
            },
            KEEP,
        )
        .unwrap();
    assert_eq!(changed.schema_version(), 1);
    assert_eq!(changed.bars(), 1);
    assert_eq!(note(&changed)["startTick"], 1_000_000_000u64);
    let odd = changed
        .update_note(
            &address("track", "note"),
            NoteValues {
                start_tick: 19,
                duration_ticks: 963,
                velocity16: 32770,
                ..values()
            },
            KEEP,
        )
        .unwrap();
    assert_eq!(note(&odd)["startTick"], 19);
    assert_eq!(note(&odd)["durationTicks"], 963);
    assert_eq!(note(&odd)["velocity16"], 32770);
}

#[test]
fn loop_extension_is_explicit_never_shrinks_and_promotes_only_when_required() {
    let original = base();
    let long = NoteValues {
        start_tick: 128 * 3840,
        duration_ticks: 1,
        ..values()
    };
    let no_upgrade = NoteTimelinePolicy::ExtendLoop {
        allow_native_schema_upgrade: false,
    };
    let upgrade = NoteTimelinePolicy::ExtendLoop {
        allow_native_schema_upgrade: true,
    };
    let before = original.to_json().unwrap();
    assert!(
        original
            .update_note(&address("track", "note"), long, no_upgrade)
            .is_err()
    );
    assert_eq!(original.to_json().unwrap(), before);
    let extended = original
        .update_note(&address("track", "note"), long, upgrade)
        .unwrap();
    assert_eq!(extended.schema_version(), 2);
    assert_eq!(extended.bars(), 129);
    let shortened = extended
        .update_note(&address("track", "note"), values(), no_upgrade)
        .unwrap();
    assert_eq!(shortened.bars(), 129);
    assert_eq!(shortened.schema_version(), 2);
    let short = original
        .update_note(
            &address("track", "note"),
            NoteValues {
                velocity16: 2,
                ..values()
            },
            upgrade,
        )
        .unwrap();
    assert_eq!(short.schema_version(), 1);
}

#[test]
fn extent_only_update_has_no_source_snapshot_and_true_noops_reject() {
    let mut value = base().value().clone();
    value["bars"] = 1.into();
    value["tracks"][0]["notes"][0]["startTick"] = 4000.into();
    let original = ProjectDocument::from_value(value).unwrap();
    let same = NoteValues {
        start_tick: 4000,
        ..values()
    };
    assert!(
        original
            .update_note(&address("track", "note"), same, KEEP)
            .is_err()
    );
    assert!(
        original
            .restore_note(&address("track", "note"), KEEP)
            .is_err()
    );
    let extended = original
        .update_note(
            &address("track", "note"),
            same,
            NoteTimelinePolicy::ExtendLoop {
                allow_native_schema_upgrade: false,
            },
        )
        .unwrap();
    assert_eq!(extended.bars(), 2);
    assert_eq!(note(&extended), note(&original));
    assert!(note(&extended).get("editSource").is_none());
}

#[test]
fn add_requires_a_fresh_strict_identity_but_existing_legacy_identity_is_editable() {
    let original = base();
    let before = original.to_json().unwrap();
    for id in ["", "spaces are legacy only", "🎹"] {
        assert!(
            original
                .add_note(&address("track", id), values(), KEEP)
                .is_err()
        );
    }
    assert!(
        original
            .add_note(&address("track", "note"), values(), KEEP)
            .is_err()
    );
    let added = original
        .add_note(
            &address("track", "new-note"),
            NoteValues {
                velocity16: 0,
                ..values()
            },
            KEEP,
        )
        .unwrap();
    let new_note = added.value()["tracks"][0]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "new-note")
        .unwrap();
    assert_eq!(
        new_note,
        &json!({"id":"new-note","pitch":60,"startTick":17,"durationTicks":961,"velocity16":0,"expression":[]})
    );
    let deleted = added.delete_note(&address("track", "new-note")).unwrap();
    assert_eq!(deleted, original);
    assert_eq!(original.to_json().unwrap(), before);
}

#[test]
fn locked_audio_and_recording_tracks_reject_every_note_mutation_without_reinterpretation() {
    for mode in ["locked", "audio", "recording"] {
        let mut value = base().value().clone();
        if mode == "locked" {
            value["tracks"][0]["locked"] = true.into();
        } else {
            value["tracks"][0]["kind"] = mode.into();
        }
        if mode == "recording" {
            value["schemaVersion"] = 2.into();
            value["tracks"][0]["recording"] = json!({"version":1,"clips":[]});
            value["tracks"][0]["notes"] = json!([{"id":"note","opaque":"not a native note"}]);
        }
        let original = ProjectDocument::from_value(value).unwrap();
        let before = original.to_json().unwrap();
        assert!(
            original
                .add_note(&address("track", "new"), values(), KEEP)
                .is_err()
        );
        assert!(
            original
                .update_note(
                    &address("track", "note"),
                    NoteValues {
                        pitch: 61,
                        ..values()
                    },
                    KEEP
                )
                .is_err()
        );
        assert!(original.delete_note(&address("track", "note")).is_err());
        assert!(
            original
                .restore_note(&address("track", "note"), KEEP)
                .is_err()
        );
        assert_eq!(original.to_json().unwrap(), before);
    }
}

#[test]
fn typed_scope_and_numeric_rejection_precede_any_document_change() {
    let document = base();
    for edit in [
        EditOperation::AddNote {
            values: values(),
            timeline: KEEP,
        },
        EditOperation::UpdateNote {
            values: values(),
            timeline: KEEP,
        },
        EditOperation::DeleteNote,
        EditOperation::RestoreNote { timeline: KEEP },
    ] {
        let mut intent = ProjectEditIntent {
            operation_id: OperationId("review".into()),
            project_id: ProjectId(document.id().into()),
            expected_revision: 0,
            scope: EditScope::Project,
            edit,
            extensions: Default::default(),
        };
        assert!(intent.validate().is_err());
        intent.scope = EditScope::DocumentNote {
            address: address("track", "new"),
        };
        intent.validate().unwrap();
    }
    for invalid in [
        NoteValues {
            pitch: 128,
            ..values()
        },
        NoteValues {
            duration_ticks: 0,
            ..values()
        },
        NoteValues {
            start_tick: u64::MAX,
            ..values()
        },
        NoteValues {
            duration_ticks: 100_000_001,
            ..values()
        },
    ] {
        assert!(
            document
                .update_note(&address("track", "note"), invalid, KEEP)
                .is_err()
        );
    }
    for wire in [
        r#"{"pitch":60,"start_tick":1.5,"duration_ticks":1,"velocity16":1}"#,
        r#"{"pitch":60,"start_tick":1,"duration_ticks":1,"velocity16":65536}"#,
        r#"{"pitch":60,"start_tick":1,"duration_ticks":1,"velocity16":1,"quantize":true}"#,
    ] {
        assert!(serde_json::from_str::<NoteValues>(wire).is_err());
    }
}

#[test]
fn snapshot_growth_rechecks_the_metadata_limit_without_losing_the_original() {
    let mut value = base().value().clone();
    value["tracks"][0]["notes"][0]["expression"] =
        json!([{"type":"pressure","points":[],"vendor":"x".repeat(1_000_000)}]);
    let original = ProjectDocument::from_value(value).unwrap();
    let before = original.to_json().unwrap();
    assert!(
        original
            .update_note(
                &address("track", "note"),
                NoteValues {
                    pitch: 61,
                    ..values()
                },
                KEEP
            )
            .is_err()
    );
    assert_eq!(original.to_json().unwrap(), before);
    assert!(note(&original).get("editSource").is_none());
    assert!(
        original
            .upgrade_native()
            .unwrap()
            .update_note(
                &address("track", "note"),
                NoteValues {
                    pitch: 61,
                    ..values()
                },
                KEEP
            )
            .is_ok()
    );
}

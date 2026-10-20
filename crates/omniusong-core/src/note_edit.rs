//! Pure, exact note mutations. The store owns revision/history/operation admission.
//! A document-note address is an equality key, never a path or transport identity.
use crate::contracts::{NoteId, Validate};
use crate::{
    DocumentError, DocumentNoteAddress, NoteTimelinePolicy, NoteValues, PPQ, ProjectDocument,
    Result, native, require,
};
use serde_json::{Value, json};

fn contract(error: crate::contracts::ContractError) -> DocumentError {
    DocumentError(error.to_string())
}
fn track_index(document: &ProjectDocument, address: &DocumentNoteAddress) -> Result<usize> {
    address.validate().map_err(contract)?;
    let index = document
        .tracks()
        .iter()
        .position(|track| track["id"].as_str() == Some(address.track_id.as_str()))
        .ok_or_else(|| DocumentError("Note track was not found".into()))?;
    let track = &document.tracks()[index];
    require(
        track["kind"] == "notes",
        "Choose a note track; audio and recording metadata are not editable notes",
    )?;
    require(
        track["locked"] == false,
        "Unlock this track before editing notes",
    )?;
    Ok(index)
}
fn note_index(
    document: &ProjectDocument,
    track: usize,
    address: &DocumentNoteAddress,
) -> Result<usize> {
    document.tracks()[track]["notes"]
        .as_array()
        .expect("validated note array")
        .iter()
        .position(|note| note["id"].as_str() == Some(address.note_id.as_str()))
        .ok_or_else(|| DocumentError("Note was not found".into()))
}
fn values(note: &Value) -> NoteValues {
    NoteValues {
        pitch: note["pitch"].as_u64().expect("validated pitch") as u8,
        start_tick: note["startTick"].as_u64().expect("validated start"),
        duration_ticks: note["durationTicks"].as_u64().expect("validated duration"),
        velocity16: note["velocity16"].as_u64().expect("validated velocity") as u16,
    }
}
fn write_values(note: &mut Value, values: NoteValues) {
    note["pitch"] = values.pitch.into();
    note["startTick"] = values.start_tick.into();
    note["durationTicks"] = values.duration_ticks.into();
    note["velocity16"] = values.velocity16.into();
}
fn original_snapshot(note: &Value) -> Value {
    let before = values(note);
    let mut source = json!({"version":1,"pitch":before.pitch,"startTick":before.start_tick,"durationTicks":before.duration_ticks,"velocity16":before.velocity16});
    // Missing, null and [] are different source representations. No normalization.
    if let Some(expression) = note.get("expression") {
        source["expression"] = expression.clone();
    }
    source
}
fn expression_tail(note: &Value) -> u64 {
    note.get("expression")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|lane| lane["points"].as_array().into_iter().flatten())
        .filter_map(|point| point["tick"].as_u64())
        .max()
        .unwrap_or(0)
}
fn timeline(value: &mut Value, values: NoteValues, policy: NoteTimelinePolicy) -> Result<()> {
    let NoteTimelinePolicy::ExtendLoop {
        allow_native_schema_upgrade,
    } = policy
    else {
        return Ok(());
    };
    let end = values.end_tick().map_err(contract)?;
    let ticks_per_bar = PPQ
        .checked_mul(4)
        .ok_or_else(|| DocumentError("Bar tick overflow".into()))?;
    let required = end.div_ceil(ticks_per_bar);
    let current = value["bars"].as_u64().expect("validated bars");
    let next = current.max(required);
    if next > 128 && value["schemaVersion"] == 1 {
        require(
            allow_native_schema_upgrade,
            "Extending beyond 128 bars requires explicit native schema upgrade permission",
        )?;
        value["schemaVersion"] = native::NATIVE_SCHEMA_VERSION.into();
    }
    require(
        next <= native::MAX_NATIVE_BARS,
        "Note extent exceeds native timeline bounds",
    )?;
    value["bars"] = next.into();
    Ok(())
}
fn finish(original: &ProjectDocument, value: Value) -> Result<ProjectDocument> {
    require(
        value != *original.value(),
        "Note edit would not change the document",
    )?;
    // Revalidate all schema/size/expression bounds before exposing a candidate.
    ProjectDocument::from_value(value)
}
impl ProjectDocument {
    /// Caller generates a stable new note ID once; no IDs/timestamps are minted here.
    pub fn add_note(
        &self,
        address: &DocumentNoteAddress,
        new: NoteValues,
        policy: NoteTimelinePolicy,
    ) -> Result<Self> {
        new.validate().map_err(contract)?;
        NoteId(address.note_id.clone())
            .validate()
            .map_err(contract)?;
        let track = track_index(self, address)?;
        let notes = self.tracks()[track]["notes"]
            .as_array()
            .expect("validated notes");
        require(
            notes.len() < 20_000,
            "Note track has reached its note limit",
        )?;
        require(
            !notes
                .iter()
                .any(|note| note["id"].as_str() == Some(address.note_id.as_str())),
            "Note identity already exists in this track",
        )?;
        let mut value = self.value().clone();
        value["tracks"][track]["notes"].as_array_mut().expect("validated notes").push(json!({"id":address.note_id,"pitch":new.pitch,"startTick":new.start_tick,"durationTicks":new.duration_ticks,"velocity16":new.velocity16,"expression":[]}));
        timeline(&mut value, new, policy)?;
        finish(self, value)
    }
    /// Only the four requested musical values change. The first source is immutable.
    pub fn update_note(
        &self,
        address: &DocumentNoteAddress,
        new: NoteValues,
        policy: NoteTimelinePolicy,
    ) -> Result<Self> {
        new.validate().map_err(contract)?;
        let track = track_index(self, address)?;
        let index = note_index(self, track, address)?;
        let before = &self.tracks()[track]["notes"][index];
        let tail = expression_tail(before);
        require(
            new.duration_ticks >= tail,
            format!("Keep at least {tail} ticks to preserve this note's expression"),
        )?;
        let mut value = self.value().clone();
        let note = &mut value["tracks"][track]["notes"][index];
        if values(before) != new {
            if note.get("editSource").is_none() {
                note["editSource"] = original_snapshot(before);
            }
            write_values(note, new);
        }
        timeline(&mut value, new, policy)?;
        finish(self, value)
    }
    /// The durable store's before snapshot retains the entire removed note.
    pub fn delete_note(&self, address: &DocumentNoteAddress) -> Result<Self> {
        let track = track_index(self, address)?;
        let index = note_index(self, track, address)?;
        let mut value = self.value().clone();
        value["tracks"][track]["notes"]
            .as_array_mut()
            .expect("validated notes")
            .remove(index);
        finish(self, value)
    }
    /// Restore schema-owned musical fields/expression; retain the entire source receipt.
    /// This restores an existing edited note. Undo restores a deleted note.
    pub fn restore_note(
        &self,
        address: &DocumentNoteAddress,
        policy: NoteTimelinePolicy,
    ) -> Result<Self> {
        let track = track_index(self, address)?;
        let index = note_index(self, track, address)?;
        let before = &self.tracks()[track]["notes"][index];
        let source = before
            .get("editSource")
            .ok_or_else(|| DocumentError("This note has no original edit snapshot".into()))?;
        let restored = values(source);
        let mut value = self.value().clone();
        let note = &mut value["tracks"][track]["notes"][index];
        write_values(note, restored);
        if let Some(expression) = source.get("expression") {
            note["expression"] = expression.clone();
        } else {
            note.as_object_mut()
                .expect("validated note")
                .remove("expression");
        }
        timeline(&mut value, restored, policy)?;
        finish(self, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(expression: Option<Value>) -> (ProjectDocument, DocumentNoteAddress) {
        let mut value = ProjectDocument::new("Exact original")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["id"] = "🎹\0".into();
        let mut note = json!({"id":"","pitch":60,"startTick":123,"durationTicks":987,"velocity16":49123,"input":{"sourceId":"synthetic","sourceName":"Synthetic fixture","protocol":"midi2","group":3,"channel":2,"receivedTimeMs":10.25,"keyDurationTicks":500,"releaseVelocity16":37155},"vendor":serde_json::from_str::<Value>(r#"{"integer":184467440737095516160001,"decimal":0.12345678901234567890123456789}"#).unwrap()});
        if let Some(expression) = expression {
            note["expression"] = expression;
        }
        value["tracks"][0]["notes"] = json!([note,{"id":"repeat","pitch":60,"startTick":123,"durationTicks":987,"velocity16":1}]);
        (
            ProjectDocument::from_value(value).unwrap(),
            DocumentNoteAddress {
                track_id: "🎹\0".into(),
                note_id: String::new(),
            },
        )
    }
    fn changed() -> NoteValues {
        NoteValues {
            pitch: 65,
            start_tick: 3957,
            duration_ticks: 1201,
            velocity16: 0,
        }
    }
    #[test]
    fn first_source_and_expression_representations_are_exact_forever() {
        for expression in [
            None,
            Some(Value::Null),
            Some(json!([])),
            Some(
                json!([{"type":"pitch","points":[{"tick":700,"value32":2271560481u32}],"vendor":"retained"}]),
            ),
        ] {
            let (source, address) = fixture(expression);
            let original = source.clone();
            let edited = source
                .update_note(&address, changed(), NoteTimelinePolicy::PreserveLoop)
                .unwrap();
            assert_eq!(source, original);
            assert_eq!(edited.bars(), source.bars());
            let old = &source.tracks()[0]["notes"][0];
            let new = &edited.tracks()[0]["notes"][0];
            assert_eq!(new.get("expression"), old.get("expression"));
            assert_eq!(new["input"], old["input"]);
            assert_eq!(new["vendor"], old["vendor"]);
            let again = edited
                .update_note(
                    &address,
                    NoteValues {
                        velocity16: u16::MAX,
                        ..changed()
                    },
                    NoteTimelinePolicy::PreserveLoop,
                )
                .unwrap();
            assert_eq!(
                again.tracks()[0]["notes"][0]["editSource"],
                new["editSource"]
            );
            let restored = again
                .restore_note(&address, NoteTimelinePolicy::PreserveLoop)
                .unwrap();
            let mut expected = old.clone();
            expected["editSource"] = new["editSource"].clone();
            assert_eq!(restored.tracks()[0]["notes"][0], expected);
            assert_eq!(
                restored.tracks()[0]["notes"][1],
                source.tracks()[0]["notes"][1]
            );
            assert!(
                restored
                    .restore_note(&address, NoteTimelinePolicy::PreserveLoop)
                    .is_err()
            );
        }
    }
    #[test]
    fn original_snapshot_opaque_fields_survive_restore_and_later_edit() {
        let (source, address) = fixture(None);
        let edited = source
            .update_note(&address, changed(), NoteTimelinePolicy::PreserveLoop)
            .unwrap();
        let mut value = edited.value().clone();
        value["tracks"][0]["notes"][0]["editSource"]["foreign"] =
            json!({"id":"not a reference","data":source.tracks()[0]["notes"][0]["vendor"]});
        let edited = ProjectDocument::from_value(value).unwrap();
        let restored = edited
            .restore_note(&address, NoteTimelinePolicy::PreserveLoop)
            .unwrap();
        let again = restored
            .update_note(&address, changed(), NoteTimelinePolicy::PreserveLoop)
            .unwrap();
        assert_eq!(
            again.tracks()[0]["notes"][0]["editSource"],
            edited.tracks()[0]["notes"][0]["editSource"]
        );
    }
    #[test]
    fn tail_lock_missing_and_wrong_kind_fail_without_mutating_source() {
        let (source, address) = fixture(Some(
            json!([{"type":"pressure","points":[{"tick":700,"value32":u32::MAX}]}]),
        ));
        let original = source.clone();
        assert!(
            source
                .update_note(
                    &address,
                    NoteValues {
                        duration_ticks: 699,
                        ..changed()
                    },
                    NoteTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
        assert!(
            source
                .update_note(
                    &address,
                    NoteValues {
                        duration_ticks: 700,
                        ..changed()
                    },
                    NoteTimelinePolicy::PreserveLoop
                )
                .is_ok()
        );
        assert!(
            source
                .restore_note(&address, NoteTimelinePolicy::PreserveLoop)
                .is_err()
        );
        for kind in ["notes", "audio"] {
            let mut value = source.value().clone();
            value["tracks"][0]["kind"] = kind.into();
            value["tracks"][0]["locked"] = (kind == "notes").into();
            let rejected = ProjectDocument::from_value(value).unwrap();
            assert!(rejected.delete_note(&address).is_err());
            assert!(
                rejected
                    .update_note(&address, changed(), NoteTimelinePolicy::PreserveLoop)
                    .is_err()
            );
        }
        assert!(
            source
                .delete_note(&DocumentNoteAddress {
                    note_id: "missing".into(),
                    ..address.clone()
                })
                .is_err()
        );
        assert_eq!(source, original);
    }
    #[test]
    fn explicit_extent_only_promotes_when_required_and_never_shrinks() {
        let (source, address) = fixture(None);
        let large = NoteValues {
            start_tick: 1_000_000_000,
            duration_ticks: 100_000_000,
            ..changed()
        };
        let kept = source
            .update_note(&address, large, NoteTimelinePolicy::PreserveLoop)
            .unwrap();
        assert_eq!(kept.schema_version(), 1);
        assert_eq!(kept.bars(), source.bars());
        assert!(
            kept.update_note(&address, large, NoteTimelinePolicy::PreserveLoop)
                .is_err()
        );
        assert!(
            source
                .update_note(
                    &address,
                    large,
                    NoteTimelinePolicy::ExtendLoop {
                        allow_native_schema_upgrade: false
                    }
                )
                .is_err()
        );
        let extended = source
            .update_note(
                &address,
                large,
                NoteTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: true,
                },
            )
            .unwrap();
        assert_eq!(extended.schema_version(), 2);
        assert_eq!(extended.bars(), 1_100_000_000u64.div_ceil(PPQ * 4));
        let restored = extended
            .restore_note(
                &address,
                NoteTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: false,
                },
            )
            .unwrap();
        assert_eq!(restored.bars(), extended.bars());
        assert_eq!(restored.schema_version(), 2);
        let only_extent = kept
            .update_note(
                &address,
                large,
                NoteTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: true,
                },
            )
            .unwrap();
        assert_eq!(only_extent.tracks(), kept.tracks());
        let near = source
            .update_note(
                &address,
                NoteValues {
                    start_tick: 5 * 3840,
                    ..changed()
                },
                NoteTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: true,
                },
            )
            .unwrap();
        assert_eq!(near.schema_version(), 1);
        assert_eq!(near.bars(), 6);
    }
    #[test]
    fn snapshot_size_cap_and_opaque_recording_notes_fail_without_changes() {
        let (source, address) = fixture(None);
        let mut value = source.value().clone();
        let base = source.to_json().unwrap().len();
        value["padding"] = "x".repeat(crate::MAX_PROJECT_BYTES - base - 64).into();
        let full = ProjectDocument::from_value(value).unwrap();
        let bytes = full.to_json().unwrap();
        assert!(
            full.update_note(&address, changed(), NoteTimelinePolicy::PreserveLoop)
                .is_err()
        );
        assert_eq!(full.to_json().unwrap(), bytes);
        let mut value = source.upgrade_native().unwrap().value().clone();
        value["tracks"][0]["id"] = "recording-track".into();
        value["tracks"][0]["kind"] = "recording".into();
        value["tracks"][0]["recording"] = json!({"version":1,"clips":[]});
        value["tracks"][0]["notes"] = json!({"foreign":"opaque rather than interpreted"});
        let recording = ProjectDocument::from_value(value).unwrap();
        let before = recording.clone();
        let address = DocumentNoteAddress {
            track_id: "recording-track".into(),
            note_id: String::new(),
        };
        assert!(recording.delete_note(&address).is_err());
        assert!(
            recording
                .add_note(
                    &DocumentNoteAddress {
                        note_id: "new-note".into(),
                        ..address
                    },
                    changed(),
                    NoteTimelinePolicy::PreserveLoop
                )
                .is_err()
        );
        assert_eq!(recording, before);
    }
    #[test]
    fn add_has_stable_strict_identity_and_delete_leaves_other_fields_exact() {
        let (source, address) = fixture(None);
        assert!(
            source
                .add_note(&address, changed(), NoteTimelinePolicy::PreserveLoop)
                .is_err()
        );
        let added_address = DocumentNoteAddress {
            note_id: "new-note".into(),
            ..address.clone()
        };
        let added = source
            .add_note(&added_address, changed(), NoteTimelinePolicy::PreserveLoop)
            .unwrap();
        assert_eq!(added.tracks()[0]["notes"][2]["velocity16"], 0);
        assert!(added.tracks()[0]["notes"][2].get("editSource").is_none());
        assert!(
            added
                .add_note(&added_address, changed(), NoteTimelinePolicy::PreserveLoop)
                .is_err()
        );
        assert_eq!(added.delete_note(&added_address).unwrap(), source);
    }
}

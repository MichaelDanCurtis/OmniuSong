//! Note-edit control DTOs. Document addresses are exact legacy JSON keys,
//! never endpoint identities, filesystem names or database object identities.
use crate::contracts::{ContractError, Validate};
use serde::{Deserialize, Serialize};

pub const MAX_NOTE_START_TICK: u64 = 1_000_000_000;
pub const MAX_NOTE_DURATION_TICKS: u64 = 100_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentNoteAddress {
    pub track_id: String,
    pub note_id: String,
}
impl Validate for DocumentNoteAddress {
    fn validate(&self) -> Result<(), ContractError> {
        // Empty, Unicode and control-bearing legacy keys are already legal in
        // ProjectDocument. Equality lookup must retain them verbatim. Global
        // TrackId/NoteId validation remains strict for its other consumers.
        if self.track_id.len() > 800
            || self.note_id.len() > 396
            || self.track_id.chars().take(201).count() > 200
            || self.note_id.chars().take(100).count() > 99
        {
            return Err(ContractError(
                "Document note address exceeds schema limits".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteValues {
    pub pitch: u8,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub velocity16: u16,
}
impl NoteValues {
    pub fn end_tick(&self) -> Result<u64, ContractError> {
        self.validate()?;
        self.start_tick
            .checked_add(self.duration_ticks)
            .ok_or_else(|| ContractError("Note end tick overflow".into()))
    }
}
impl Validate for NoteValues {
    fn validate(&self) -> Result<(), ContractError> {
        if self.pitch > 127
            || self.start_tick > MAX_NOTE_START_TICK
            || self.duration_ticks == 0
            || self.duration_ticks > MAX_NOTE_DURATION_TICKS
            || self.start_tick.checked_add(self.duration_ticks).is_none()
        {
            return Err(ContractError(
                "Note values exceed musical document limits".into(),
            ));
        }
        // Velocity zero is a valid high-resolution Note On value.
        Ok(())
    }
}

/// Loop policy is explicit in each durable request. No implicit quantization,
/// session shortening or schema promotion follows from changing a note.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoteTimelinePolicy {
    #[default]
    PreserveLoop,
    ExtendLoop {
        allow_native_schema_upgrade: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{NoteId, TrackId};
    #[test]
    fn document_keys_keep_legacy_boundaries_without_widening_transport_ids() {
        let address = DocumentNoteAddress {
            track_id: "🎹".repeat(200),
            note_id: "\0".into(),
        };
        address.validate().unwrap();
        assert!(TrackId(address.track_id.clone()).validate().is_err());
        assert!(NoteId(address.note_id.clone()).validate().is_err());
        let wire = serde_json::to_string(&address).unwrap();
        assert_eq!(
            serde_json::from_str::<DocumentNoteAddress>(&wire).unwrap(),
            address
        );
        DocumentNoteAddress {
            track_id: String::new(),
            note_id: String::new(),
        }
        .validate()
        .unwrap();
        assert!(
            DocumentNoteAddress {
                track_id: "x".repeat(201),
                note_id: "n".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            DocumentNoteAddress {
                track_id: "t".into(),
                note_id: "🎹".repeat(100)
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn values_keep_exact_zero_velocity_and_document_time_bounds() {
        let values = NoteValues {
            pitch: 127,
            start_tick: MAX_NOTE_START_TICK,
            duration_ticks: MAX_NOTE_DURATION_TICKS,
            velocity16: 0,
        };
        assert_eq!(values.end_tick().unwrap(), 1_100_000_000);
        assert_eq!(
            serde_json::from_str::<NoteValues>(&serde_json::to_string(&values).unwrap()).unwrap(),
            values
        );
        assert!(
            NoteValues {
                start_tick: u64::MAX,
                ..values
            }
            .validate()
            .is_err()
        );
        assert!(
            NoteValues {
                duration_ticks: 0,
                ..values
            }
            .validate()
            .is_err()
        );
        assert!(
            NoteValues {
                pitch: 128,
                ..values
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn typed_intents_require_exact_note_scope_explicit_extent_and_strict_new_ids() {
        use crate::contracts::{
            EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId,
        };
        let values = NoteValues {
            pitch: 60,
            start_tick: 17,
            duration_ticks: 963,
            velocity16: 0,
        };
        let address = DocumentNoteAddress {
            track_id: "演奏\0".into(),
            note_id: String::new(),
        };
        let mut intent = ProjectEditIntent {
            operation_id: OperationId("note-edit".into()),
            project_id: ProjectId("session".into()),
            expected_revision: 4,
            scope: EditScope::DocumentNote {
                address: address.clone(),
            },
            edit: EditOperation::UpdateNote {
                values,
                timeline: NoteTimelinePolicy::PreserveLoop,
            },
            extensions: Default::default(),
        };
        intent.validate().unwrap();
        let wire = serde_json::to_value(&intent).unwrap();
        assert_eq!(
            serde_json::from_value::<ProjectEditIntent>(wire.clone()).unwrap(),
            intent
        );
        let mut missing_policy = wire.clone();
        missing_policy["edit"]["data"]
            .as_object_mut()
            .unwrap()
            .remove("timeline");
        assert!(serde_json::from_value::<ProjectEditIntent>(missing_policy).is_err());
        let mut arbitrary_patch = wire;
        arbitrary_patch["edit"]["data"]["values"]["input"] =
            serde_json::json!({"sourceId":"forged"});
        assert!(serde_json::from_value::<ProjectEditIntent>(arbitrary_patch).is_err());
        intent.edit = EditOperation::AddNote {
            values,
            timeline: NoteTimelinePolicy::PreserveLoop,
        };
        assert!(intent.validate().is_err());
        intent.scope = EditScope::DocumentNote {
            address: DocumentNoteAddress {
                note_id: "new-note".into(),
                ..address
            },
        };
        intent.validate().unwrap();
        for operation in [
            EditOperation::DeleteNote,
            EditOperation::RestoreNote {
                timeline: NoteTimelinePolicy::PreserveLoop,
            },
            EditOperation::UpdateNote {
                values,
                timeline: NoteTimelinePolicy::PreserveLoop,
            },
            intent.edit.clone(),
        ] {
            intent.edit = operation;
            intent.scope = EditScope::Project;
            assert!(intent.validate().is_err());
        }
    }
}

//! Finite clip-setting changes. These are equality keys and typed settings,
//! never JSON paths, source replacement requests, or clock calibration.
use crate::contracts::{ContractError, Validate};
use crate::native::AudioFades;
use serde::{Deserialize, Serialize};
use serde_json::Number;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "representation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentClipAddress {
    LegacyAudio { track_id: String },
    NativeAudio { track_id: String, clip_id: String },
}
impl Validate for DocumentClipAddress {
    fn validate(&self) -> Result<(), ContractError> {
        let native_key = |value: &str| {
            !value.is_empty()
                && value.len() <= 96
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        };
        let valid = match self {
            Self::LegacyAudio { track_id } => {
                track_id.len() <= 800 && track_id.chars().take(201).count() <= 200
            }
            Self::NativeAudio { track_id, clip_id } => native_key(track_id) && native_key(clip_id),
        };
        if valid {
            Ok(())
        } else {
            Err(ContractError("Invalid document clip address".into()))
        }
    }
}

/// Keep preserves the exact current representation; Remove restores omission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum FieldChange<T> {
    Keep,
    Set { value: T },
    Remove,
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for FieldChange<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Serde's internally tagged unit variants ignore extra fields. Empty
        // struct variants enforce the same strict shape as Set on input.
        #[derive(Deserialize)]
        #[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire<T> {
            Keep {},
            Set { value: T },
            Remove {},
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Keep {} => Self::Keep,
            Wire::Set { value } => Self::Set { value },
            Wire::Remove {} => Self::Remove,
        })
    }
}

/// Required fields cannot be removed. Both forms are explicit on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueChange<T> {
    Keep,
    Set { value: T },
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for ValueChange<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire<T> {
            Keep {},
            Set { value: T },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Keep {} => Self::Keep,
            Wire::Set { value } => Self::Set { value },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyAudioClipPatch {
    pub start_tick: FieldChange<u64>,
    pub trim_start_seconds: FieldChange<Number>,
    pub trim_end_seconds: FieldChange<Number>,
    pub fade_in_seconds: FieldChange<Number>,
    pub fade_out_seconds: FieldChange<Number>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSourceWindow {
    pub start_frame: u64,
    pub frame_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAudioClipPatch {
    pub position_frame: ValueChange<i64>,
    pub source_window: ValueChange<AudioSourceWindow>,
    pub fades: FieldChange<AudioFades>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "representation",
    content = "fields",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ClipPatch {
    LegacyAudio(LegacyAudioClipPatch),
    NativeAudio(NativeAudioClipPatch),
}
impl ClipPatch {
    pub fn matches_address(&self, address: &DocumentClipAddress) -> bool {
        matches!(
            (self, address),
            (
                Self::LegacyAudio(_),
                DocumentClipAddress::LegacyAudio { .. }
            ) | (
                Self::NativeAudio(_),
                DocumentClipAddress::NativeAudio { .. }
            )
        )
    }
}
impl Validate for ClipPatch {
    fn validate(&self) -> Result<(), ContractError> {
        let invalid = || ContractError("Clip values exceed document limits".into());
        match self {
            Self::LegacyAudio(patch) => {
                if matches!(patch.start_tick, FieldChange::Set{value} if value > 1_000_000_000) {
                    return Err(invalid());
                }
                for change in [
                    &patch.trim_start_seconds,
                    &patch.trim_end_seconds,
                    &patch.fade_in_seconds,
                    &patch.fade_out_seconds,
                ] {
                    if let FieldChange::Set { value } = change
                        && !value.as_f64().is_some_and(|value| {
                            value.is_finite() && (0.0..=86_400.0).contains(&value)
                        })
                    {
                        return Err(invalid());
                    }
                }
            }
            Self::NativeAudio(patch) => {
                if let ValueChange::Set { value } = &patch.source_window {
                    crate::native::TakeRange::AudioFrames {
                        start: value.start_frame,
                        count: value.frame_count,
                    }
                    .end()
                    .map_err(|error| ContractError(error.to_string()))?;
                }
                if let FieldChange::Set { value } = &patch.fades
                    && value.in_frames.checked_add(value.out_frames).is_none()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipTimelinePolicy {
    #[default]
    PreserveLoop,
    ExtendLoop {
        allow_native_schema_upgrade: bool,
    },
}
impl<'de> Deserialize<'de> for ClipTimelinePolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            PreserveLoop {},
            ExtendLoop { allow_native_schema_upgrade: bool },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::PreserveLoop {} => Self::PreserveLoop,
            Wire::ExtendLoop {
                allow_native_schema_upgrade,
            } => Self::ExtendLoop {
                allow_native_schema_upgrade,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId};
    use serde_json::{Value, json};

    fn legacy() -> ClipPatch {
        ClipPatch::LegacyAudio(LegacyAudioClipPatch {
            start_tick: FieldChange::Keep,
            trim_start_seconds: FieldChange::Keep,
            trim_end_seconds: FieldChange::Keep,
            fade_in_seconds: FieldChange::Set {
                value: serde_json::from_str("0.12345678901234567890123456789").unwrap(),
            },
            fade_out_seconds: FieldChange::Remove,
        })
    }
    fn request() -> ProjectEditIntent {
        ProjectEditIntent {
            operation_id: OperationId("clip-operation".into()),
            project_id: ProjectId("clip-project".into()),
            expected_revision: 7,
            scope: EditScope::DocumentClip {
                address: DocumentClipAddress::LegacyAudio {
                    track_id: "演奏\0\n".into(),
                },
            },
            edit: EditOperation::UpdateClip {
                patch: legacy(),
                timeline: ClipTimelinePolicy::PreserveLoop,
            },
            extensions: Default::default(),
        }
    }
    #[test]
    fn clip_wire_retains_exact_decimal_and_explicit_keep_remove() {
        let request = request();
        request.validate().unwrap();
        let wire = serde_json::to_string(&request).unwrap();
        assert!(wire.contains("0.12345678901234567890123456789"));
        let decoded: ProjectEditIntent = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    }
    #[test]
    fn clip_wire_rejects_missing_policy_patch_fields_and_arbitrary_data() {
        let original = serde_json::to_value(request()).unwrap();
        for pointer in [
            "/edit/data/timeline",
            "/edit/data/patch/fields/fade_out_seconds",
        ] {
            let mut value = original.clone();
            let (parent, field) = pointer.rsplit_once('/').unwrap();
            value
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                serde_json::from_value::<ProjectEditIntent>(value).is_err(),
                "{pointer}"
            );
        }
        for (pointer, key, field) in [
            ("/edit/data/patch/fields", "sourceAssetId", json!("forged")),
            (
                "/edit/data/patch/fields/fade_out_seconds",
                "value",
                json!(1),
            ),
            ("/scope/selection/address", "path", json!("../source")),
            ("/edit/data/timeline", "calibration", json!("measured")),
        ] {
            let mut value = original.clone();
            value
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), field);
            assert!(serde_json::from_value::<ProjectEditIntent>(value).is_err());
        }
    }
    #[test]
    fn clip_scope_and_representation_cannot_be_reinterpreted() {
        let mut request = request();
        request.scope = EditScope::Project;
        assert!(request.validate().is_err());
        request.scope = EditScope::DocumentClip {
            address: DocumentClipAddress::NativeAudio {
                track_id: "track".into(),
                clip_id: "clip".into(),
            },
        };
        assert!(request.validate().is_err());
        request.edit = EditOperation::RestoreClip {
            timeline: ClipTimelinePolicy::PreserveLoop,
        };
        request.validate().unwrap();
    }
    #[test]
    fn clip_keys_preserve_legacy_limits_and_native_strictness() {
        for track_id in [String::new(), "🎹".repeat(200), "x\0\n".into()] {
            DocumentClipAddress::LegacyAudio { track_id }
                .validate()
                .unwrap();
        }
        assert!(
            DocumentClipAddress::LegacyAudio {
                track_id: "🎹".repeat(201)
            }
            .validate()
            .is_err()
        );
        for clip_id in [
            String::new(),
            "🎹".into(),
            "x".repeat(97),
            "../x".into(),
            "x\0".into(),
        ] {
            assert!(
                DocumentClipAddress::NativeAudio {
                    track_id: "track".into(),
                    clip_id
                }
                .validate()
                .is_err()
            );
        }
        DocumentClipAddress::NativeAudio {
            track_id: "t._:-1".into(),
            clip_id: "x".repeat(96),
        }
        .validate()
        .unwrap();
    }
    #[test]
    fn clip_patch_rejects_extreme_numbers_and_checked_frame_overflow() {
        for number in ["-0.001", "86400.001", "1e10000"] {
            let patch = ClipPatch::LegacyAudio(LegacyAudioClipPatch {
                start_tick: FieldChange::Keep,
                trim_start_seconds: FieldChange::Keep,
                trim_end_seconds: FieldChange::Set {
                    value: serde_json::from_str(number).unwrap(),
                },
                fade_in_seconds: FieldChange::Keep,
                fade_out_seconds: FieldChange::Keep,
            });
            assert!(patch.validate().is_err());
        }
        let mut patch = NativeAudioClipPatch {
            position_frame: ValueChange::Set { value: i64::MIN },
            source_window: ValueChange::Set {
                value: AudioSourceWindow {
                    start_frame: 0,
                    frame_count: i64::MAX as u64,
                },
            },
            fades: FieldChange::Keep,
        };
        ClipPatch::NativeAudio(patch.clone()).validate().unwrap();
        patch.source_window = ValueChange::Set {
            value: AudioSourceWindow {
                start_frame: 1,
                frame_count: i64::MAX as u64,
            },
        };
        assert!(ClipPatch::NativeAudio(patch).validate().is_err());
        assert!(serde_json::from_value::<ValueChange<u64>>(json!({"change":"remove"})).is_err());
        assert!(
            serde_json::from_value::<FieldChange<Number>>(
                json!({"change":"set","value":Value::Null})
            )
            .is_err()
        );
    }
}

//! Pure complete candidate construction. No retained review, source or caller
//! supplied binding acquires authentication/publication authority here.
use super::{
    MAX_PROJECTION_LANES, PerformanceProjectionBinding, ProjectionLaneChoice, native_identifier,
};
use crate::{DocumentError, NoteTimelinePolicy, PPQ, ProjectDocument, Result, require};
use serde_json::{Value, json};
use std::collections::BTreeSet;

impl ProjectDocument {
    /// Append one complete original binding and all initially muted ordinary
    /// note lanes. The store must derive/authenticate the binding itself and
    /// commit this candidate with history and its exact operation receipt.
    /// This method changes no revision and mutates no existing document.
    pub fn append_performance_projection(
        &self,
        binding: &PerformanceProjectionBinding,
        choices: &[ProjectionLaneChoice],
        timeline: NoteTimelinePolicy,
        allow_projection_schema_upgrade: bool,
    ) -> Result<Self> {
        binding.validate()?;
        require(
            !choices.is_empty()
                && choices.len() <= MAX_PROJECTION_LANES
                && choices.len() == binding.lanes.len(),
            "Projection publication requires every nonempty original lane",
        )?;
        require(
            choices.windows(2).all(|pair| {
                (pair[0].source, pair[0].instrument) < (pair[1].source, pair[1].instrument)
            }),
            "Projection lane choices must be sorted and unique",
        )?;
        let promoted = if self.schema_version() < 3 {
            require(
                allow_projection_schema_upgrade,
                "Performance note publication requires explicit musical schema-3 permission",
            )?;
            self.upgrade_performance_projection_schema()?
        } else {
            self.clone()
        };
        let mut registry = promoted
            .performance_projections()?
            .ok_or_else(|| DocumentError("Projection registry is missing".into()))?;
        require(
            !registry.entries.iter().any(|entry| entry.id == binding.id),
            "Performance projection identity already exists",
        )?;
        let reserved_tracks: BTreeSet<_> = registry
            .entries
            .iter()
            .flat_map(|entry| entry.lanes.iter().map(|lane| lane.track_id.as_str()))
            .collect();
        let reserved_notes: BTreeSet<_> = registry
            .entries
            .iter()
            .flat_map(|entry| &entry.lanes)
            .flat_map(|lane| {
                lane.original_notes
                    .iter()
                    .map(|note| note.note_id.0.as_str())
            })
            .collect();
        let current_tracks: BTreeSet<_> = self
            .tracks()
            .iter()
            .filter_map(|track| track["id"].as_str())
            .collect();
        let current_notes: BTreeSet<_> = self
            .tracks()
            .iter()
            .filter(|track| matches!(track["kind"].as_str(), Some("notes" | "audio")))
            .flat_map(|track| track["notes"].as_array().into_iter().flatten())
            .filter_map(|note| note["id"].as_str())
            .collect();
        require(
            self.tracks()
                .len()
                .checked_add(choices.len())
                .is_some_and(|n| n <= 64),
            "Projection lanes exceed the project track limit",
        )?;
        let mut end_tick = 0u64;
        let mut tracks = Vec::with_capacity(choices.len());
        for (choice, original) in choices.iter().zip(&binding.lanes) {
            native_identifier(&choice.track_id, "publication track identity")?;
            require(
                choice.source == original.source
                    && choice.instrument == original.instrument
                    && choice.track_id == original.track_id,
                "Projection lane destination differs from its complete original binding",
            )?;
            require(
                choice.track_name.len() <= 640
                    && !choice.track_name.trim().is_empty()
                    && choice.track_name.chars().count() <= 160
                    && !choice.track_name.chars().any(char::is_control),
                "Invalid projection lane name",
            )?;
            require(
                !current_tracks.contains(choice.track_id.as_str())
                    && !reserved_tracks.contains(choice.track_id.as_str()),
                "Projection publication requires a new unreserved track identity",
            )?;
            let mut notes = Vec::with_capacity(original.original_notes.len());
            for note in &original.original_notes {
                require(
                    !current_notes.contains(note.note_id.0.as_str())
                        && !reserved_notes.contains(note.note_id.0.as_str()),
                    "Projection publication cannot reuse an existing or reserved note identity",
                )?;
                end_tick = end_tick.max(
                    note.values
                        .end_tick()
                        .map_err(|e| DocumentError(e.to_string()))?,
                );
                notes.push(json!({
                    "id":note.note_id.0,"pitch":note.values.pitch,
                    "startTick":note.values.start_tick,"durationTicks":note.values.duration_ticks,
                    "velocity16":note.values.velocity16,"expression":[]
                }));
            }
            tracks.push(json!({
                "id":choice.track_id,"name":choice.track_name,"kind":"notes",
                "instrument":choice.instrument.as_str(),"gain":0.65,
                "muted":true,"locked":false,"armed":false,"notes":notes,
                "provenance":{"kind":"procedural-performance-projection","projectionId":binding.id}
            }));
        }
        drop(reserved_tracks);
        drop(reserved_notes);
        registry.entries.push(binding.clone());
        let mut value = promoted.value().clone();
        value["performanceProjections"] =
            serde_json::to_value(registry).map_err(|e| DocumentError(e.to_string()))?;
        value["tracks"]
            .as_array_mut()
            .expect("validated tracks")
            .extend(tracks);
        if matches!(timeline, NoteTimelinePolicy::ExtendLoop { .. }) {
            let bars = end_tick.div_ceil(PPQ * 4).max(self.bars());
            require(
                bars <= crate::native::MAX_NATIVE_BARS,
                "Projection exceeds native timeline bounds",
            )?;
            value["bars"] = Value::from(bars);
        }
        // Revalidate global identities, original ownership, all existing opaque
        // data and the unchanged complete metadata limit before returning.
        Self::from_value(value)
    }
}

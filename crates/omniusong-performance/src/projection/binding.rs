//! Portable retained encoding checks are deliberately separate from a held
//! original's derivation authority. Neither path publishes or adopts a document.
use super::*;
use omniusong_core::{
    contracts::NoteId,
    native::TakeReference,
    performance_projection::{
        PerformanceProjectionBinding, ProjectionInstrument, ProjectionLaneBinding,
        ProjectionLaneChoice, ProjectionNoteBinding, RetainedProjectionReview,
    },
};
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

const NOTE_ID_DOMAIN: &[u8] = b"OmniuSong.performance-projection-document-note.v1\0";
const REVIEW_DOMAIN: &[u8] = b"OmniuSong.performance-projection-review.v1\0";

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}
fn patch(patch: LivePatch) -> ProjectionInstrument {
    match patch {
        LivePatch::Keys => ProjectionInstrument::Keys,
        LivePatch::Bass => ProjectionInstrument::Bass,
        LivePatch::Drums => ProjectionInstrument::Drums,
        LivePatch::Pad => ProjectionInstrument::Pad,
    }
}
fn key(source: u8, instrument: ProjectionInstrument) -> (u8, ProjectionInstrument) {
    (source, instrument)
}
fn group_index(source: u8, instrument: ProjectionInstrument) -> usize {
    let instrument = match instrument {
        ProjectionInstrument::Keys => 0,
        ProjectionInstrument::Bass => 1,
        ProjectionInstrument::Drums => 2,
        ProjectionInstrument::Pad => 3,
    };
    usize::from(source - 1) * 4 + instrument
}
fn document_note_id(
    projection_id: &str,
    identity: &str,
    budget: &InspectionBudget,
) -> Result<NoteId> {
    require(
        identifier(projection_id) && sha(identity),
        "Invalid deterministic projected note identity inputs",
    )?;
    charge(
        budget,
        NOTE_ID_DOMAIN.len() + projection_id.len() + identity.len() + 16 + 160,
    )?;
    let mut hash = Sha256::new();
    hash.update(NOTE_ID_DOMAIN);
    for field in [projection_id.as_bytes(), identity.as_bytes()] {
        hash.update((field.len() as u64).to_le_bytes());
        hash.update(field);
    }
    Ok(NoteId(format!("pn-{:x}", hash.finalize())))
}

fn choices_shape(
    projection_id: &str,
    expected: &str,
    choices: &[ProjectionLaneChoice],
    budget: &InspectionBudget,
) -> Result<()> {
    budget.checkpoint()?;
    charge(budget, 4096)?;
    require(identifier(projection_id), "Invalid projection identity")?;
    require(
        sha(expected),
        "Projection publication requires an exact lowercase review SHA-256",
    )?;
    require(
        !choices.is_empty() && choices.len() <= 16,
        "Choose one through sixteen nonempty source/instrument lanes",
    )?;
    let mut ids = BTreeSet::new();
    let mut previous = None;
    for choice in choices {
        charge(budget, 2048)?;
        require(
            (1..=4).contains(&choice.source) && identifier(&choice.track_id),
            "Invalid projection lane source or track identity",
        )?;
        require(
            choice.track_name.len() <= 640
                && choice.track_name.chars().count() <= 160
                && !choice.track_name.trim().is_empty()
                && !choice.track_name.chars().any(char::is_control),
            "Projection lane names must be nonblank, without controls and at most 160 characters",
        )?;
        require(
            ids.insert(choice.track_id.as_str()),
            "Projection lane track identities must be unique",
        )?;
        let next = key(choice.source, choice.instrument);
        require(
            previous.is_none_or(|old| old < next),
            "Projection lanes must be sorted unique source/instrument groups",
        )?;
        previous = Some(next);
    }
    Ok(())
}

/// Writer credits allocation/reallocation and byte copies before extending its
/// bounded output. Serde never creates a second unbounded canonical JSON buffer.
struct CanonicalWriter<'a> {
    budget: &'a InspectionBudget,
    bytes: Vec<u8>,
}
impl Write for CanonicalWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let target = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= MAX_PROJECTION_REVIEW_BYTES)
            .ok_or_else(|| std::io::Error::other("Retained projection exceeds 8 MiB"))?;
        if target > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .max(1024)
                .saturating_mul(2)
                .max(target)
                .min(MAX_PROJECTION_REVIEW_BYTES);
            self.budget
                .charge_work((capacity + self.bytes.len()) as u64)
                .map_err(|_| {
                    std::io::Error::other("Projection governor stopped canonical allocation")
                })?;
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.budget
            .charge_work((bytes.len() as u64).saturating_mul(2))
            .map_err(|_| std::io::Error::other("Projection governor stopped canonical output"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn canonical_review(
    review: &PerformanceProjectionReview,
    budget: &InspectionBudget,
) -> Result<String> {
    let mut writer = CanonicalWriter {
        budget,
        bytes: Vec::new(),
    };
    let encoded = serde_json::to_writer(&mut writer, review);
    budget.checkpoint()?;
    encoded.map_err(invalid)?;
    // The typed serializer only emits UTF-8; conversion consumes the same Vec.
    charge(budget, writer.bytes.len())?;
    String::from_utf8(writer.bytes).map_err(invalid)
}
struct CompareWriter<'a> {
    budget: &'a InspectionBudget,
    expected: &'a [u8],
    used: usize,
}
impl Write for CompareWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.budget
            .charge_work((bytes.len() as u64).saturating_mul(2))
            .map_err(|_| {
                std::io::Error::other("Projection governor stopped canonical comparison")
            })?;
        let end = self
            .used
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("Canonical projection offset overflow"))?;
        if self.expected.get(self.used..end) != Some(bytes) {
            return Err(std::io::Error::other(
                "Retained projection is not the exact typed canonical encoding",
            ));
        }
        self.used = end;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn canonical_matches(
    review: &PerformanceProjectionReview,
    expected: &str,
    budget: &InspectionBudget,
) -> Result<()> {
    let mut writer = CompareWriter {
        budget,
        expected: expected.as_bytes(),
        used: 0,
    };
    let encoded = serde_json::to_writer(&mut writer, review);
    budget.checkpoint()?;
    encoded.map_err(invalid)?;
    require(
        writer.used == expected.len(),
        "Retained projection has extra noncanonical bytes",
    )
}

fn outer_credit(
    binding: &PerformanceProjectionBinding,
    budget: &InspectionBudget,
) -> Result<usize> {
    budget.checkpoint()?;
    charge(budget, 4096)?;
    let bytes = binding.review.canonical_json.len();
    require(
        bytes > 0 && bytes <= MAX_PROJECTION_REVIEW_BYTES,
        "Invalid retained projection review byte length",
    )?;
    require(
        !binding.lanes.is_empty() && binding.lanes.len() <= 16,
        "Invalid retained projection lane count",
    )?;
    let mut count = 0usize;
    for lane in &binding.lanes {
        count = count
            .checked_add(lane.original_notes.len())
            .ok_or_else(|| invalid("Retained projection note count overflow"))?;
        require(
            count <= MAX_PROJECTION_OBSERVATIONS,
            "Too many retained projection original notes",
        )?;
    }
    let credit = bytes
        .checked_mul(4)
        .and_then(|n| count.checked_mul(768).and_then(|rows| n.checked_add(rows)))
        .ok_or_else(|| invalid("Retained projection validation work overflow"))?;
    charge(budget, credit)?;
    Ok(bytes)
}
fn validate_identity(
    identity: &PerformanceVoiceIdentity,
    encoded_sha: &str,
    request: &PerformanceProjectionRequest,
    budget: &InspectionBudget,
) -> Result<()> {
    require(
        identity.take_id == request.source.take_id
            && identity.scope.is_valid()
            && (1..=4).contains(&identity.source)
            && request.sources.contains(&identity.source)
            && identity.connection_generation == 1
            && identity.note > 0
            && identity.voice > 0
            && sha(encoded_sha),
        "Invalid retained original voice identity",
    )?;
    require(
        digest(
            b"OmniuSong.performance-projected-note.v1\0",
            identity,
            4096,
            budget,
        )? == encoded_sha,
        "Retained original voice identity digest differs",
    )
}
fn review_shape(review: &PerformanceProjectionReview, budget: &InspectionBudget) -> Result<()> {
    review.request.validate()?;
    require(
        review.version == 1
            && matches!(review.profile_version, 1 | 2)
            && (8_000..=768_000).contains(&review.sample_rate)
            && review.gui_clock.is_valid(),
        "Invalid retained projection review version/profile/clock",
    )?;
    let rows = review
        .notes
        .len()
        .checked_add(review.omissions.len())
        .ok_or_else(|| invalid("Retained projection row count overflow"))?;
    require(
        rows <= MAX_PROJECTION_OBSERVATIONS && review.disclosures.len() <= 9,
        "Retained projection exceeds row/disclosure bounds",
    )?;
    charge(
        budget,
        rows.checked_mul(1024)
            .ok_or_else(|| invalid("Retained projection row work overflow"))?,
    )?;
    require(
        sha(&review.unselected_observations_sha256),
        "Invalid retained unselected observation digest",
    )?;
    let mut identities = BTreeSet::new();
    for note in &review.notes {
        budget.visit_records(1)?;
        validate_identity(
            &note.identity,
            &note.identity_sha256,
            &review.request,
            budget,
        )?;
        note.values.validate().map_err(invalid)?;
        require(
            identities.insert(note.identity_sha256.as_str()),
            "Duplicate retained original voice identity",
        )?;
    }
    let mut crossing = 0u32;
    let mut incomplete = 0u32;
    let mut collapsed = 0u32;
    for omission in &review.omissions {
        budget.visit_records(1)?;
        validate_identity(
            &omission.identity,
            &omission.identity_sha256,
            &review.request,
            budget,
        )?;
        require(
            identities.insert(omission.identity_sha256.as_str()),
            "Duplicate retained original voice identity",
        )?;
        match omission.reason {
            PerformanceProjectionOmissionReason::RegionCrossing => crossing += 1,
            PerformanceProjectionOmissionReason::Incomplete => incomplete += 1,
            PerformanceProjectionOmissionReason::CollapsedTicks => collapsed += 1,
            _ => {
                return Err(invalid(
                    "Unselected observations cannot occupy selected omission rows",
                ));
            }
        }
    }
    let total = (rows as u64)
        .checked_add(review.counts.excluded_source)
        .and_then(|n| n.checked_add(review.counts.outside_region))
        .ok_or_else(|| invalid("Retained projection observation count overflow"))?;
    require(
        review.counts.notes == review.notes.len() as u32
            && review.counts.observations == total
            && review.counts.region_crossing == crossing
            && review.counts.incomplete == incomplete
            && review.counts.collapsed == collapsed,
        "Retained projection counts differ from its complete result rows",
    )
}

fn group_counts(
    review: &PerformanceProjectionReview,
    budget: &InspectionBudget,
) -> Result<[usize; 16]> {
    charge(
        budget,
        review
            .notes
            .len()
            .checked_mul(64)
            .ok_or_else(|| invalid("Projection group traversal work overflow"))?,
    )?;
    let mut counts = [0usize; 16];
    for note in &review.notes {
        counts[group_index(note.identity.source, patch(note.patch))] += 1;
    }
    Ok(counts)
}
fn lane_bijection(
    binding: &PerformanceProjectionBinding,
    review: &PerformanceProjectionReview,
    budget: &InspectionBudget,
) -> Result<()> {
    let counts = group_counts(review, budget)?;
    let nonempty = counts.iter().filter(|n| **n > 0).count();
    require(
        nonempty > 0 && binding.lanes.len() == nonempty,
        "Retained lanes do not cover the exact nonempty projection groups",
    )?;
    let mut previous = None;
    let mut offsets = [0usize; 16];
    for lane in &binding.lanes {
        let next = key(lane.source, lane.instrument);
        require(
            previous.is_none_or(|old| old < next),
            "Retained lanes are not sorted unique source/instrument groups",
        )?;
        previous = Some(next);
        require(
            lane.original_notes.len() == counts[group_index(lane.source, lane.instrument)],
            "Retained original lane note count differs",
        )?;
    }
    for note in &review.notes {
        budget.visit_records(1)?;
        charge(budget, 1024)?;
        let group = key(note.identity.source, patch(note.patch));
        let lane = binding
            .lanes
            .iter()
            .find(|lane| key(lane.source, lane.instrument) == group)
            .ok_or_else(|| invalid("Projected source/patch group has no retained lane"))?;
        let index = group_index(group.0, group.1);
        let original = lane
            .original_notes
            .get(offsets[index])
            .ok_or_else(|| invalid("Retained lane omits a projected note"))?;
        offsets[index] += 1;
        require(
            original.identity_sha256 == note.identity_sha256
                && original.values == note.values
                && original.note_id
                    == document_note_id(&binding.id, &note.identity_sha256, budget)?,
            "Retained lane note identity, values or canonical order differs from its review",
        )?;
    }
    Ok(())
}

fn decode_binding(
    binding: &PerformanceProjectionBinding,
    budget: &InspectionBudget,
) -> Result<PerformanceProjectionReview> {
    let bytes = outer_credit(binding, budget)?;
    binding.validate().map_err(invalid)?;
    // Full typed decode (including any malicious string/array allocation) is
    // prepaid from the bounded canonical input, before serde allocates it.
    charge(
        budget,
        bytes
            .checked_mul(16)
            .ok_or_else(|| invalid("Retained projection decode work overflow"))?,
    )?;
    let mut review: PerformanceProjectionReview =
        serde_json::from_str(&binding.review.canonical_json).map_err(invalid)?;
    review_shape(&review, budget)?;
    require(
        review.review_sha256 == binding.review.review_sha256,
        "Retained review digest fields differ",
    )?;
    require(
        review.request.source.take_id == binding.source.take_id.0
            && review.request.source.manifest_sha256 == binding.source.manifest_sha256,
        "Retained full review source differs from its owned source",
    )?;
    canonical_matches(&review, &binding.review.canonical_json, budget)?;
    let saved = std::mem::take(&mut review.review_sha256);
    let computed = digest(
        REVIEW_DOMAIN,
        &review,
        MAX_PROJECTION_REVIEW_BYTES - 64,
        budget,
    );
    review.review_sha256 = saved;
    require(
        computed? == review.review_sha256,
        "Retained performance projection canonical review digest differs",
    )?;
    lane_bijection(binding, &review, budget)?;
    Ok(review)
}

/// Checks only the retained record's complete encoding and internal bindings.
/// It neither authenticates nor rederives an original, grants no playable
/// document authority, and returns no source-free authentication token.
pub fn check_retained_binding_encoding_bounded(
    binding: &PerformanceProjectionBinding,
    budget: &InspectionBudget,
) -> Result<()> {
    let result = decode_binding(binding, budget).map(|_| ());
    budget.checkpoint()?;
    result
}

impl AuthenticatedPerformance<'_> {
    pub fn build_binding(
        &self,
        projection_id: &str,
        request: &PerformanceProjectionRequest,
        expected_review_sha256: &str,
        choices: &[ProjectionLaneChoice],
    ) -> Result<PerformanceProjectionBinding> {
        let budget = self.budget();
        let result = (|| {
            choices_shape(projection_id, expected_review_sha256, choices, budget)?;
            let review = self.project(request)?;
            require(
                review.review_sha256 == expected_review_sha256,
                "Reviewed performance projection SHA-256 differs from the held derivation",
            )?;
            let counts = group_counts(&review, budget)?;
            require(
                choices.len() == counts.iter().filter(|n| **n > 0).count(),
                "Choose exactly the nonempty derived source/instrument groups",
            )?;
            charge(
                budget,
                choices.len() * 1024
                    + review.notes.len() * (std::mem::size_of::<ProjectionNoteBinding>() + 256)
                    + 4096,
            )?;
            let mut lanes = Vec::with_capacity(choices.len());
            for choice in choices {
                let count = counts[group_index(choice.source, choice.instrument)];
                require(count > 0, "A chosen projection lane has no derived notes")?;
                lanes.push(ProjectionLaneBinding {
                    source: choice.source,
                    track_id: choice.track_id.clone(),
                    instrument: choice.instrument,
                    original_notes: Vec::with_capacity(count),
                });
            }
            for note in &review.notes {
                budget.visit_records(1)?;
                charge(budget, 1024)?;
                let lane = lanes
                    .iter_mut()
                    .find(|lane| {
                        key(lane.source, lane.instrument)
                            == key(note.identity.source, patch(note.patch))
                    })
                    .ok_or_else(|| invalid("A derived source/patch group was not chosen"))?;
                lane.original_notes.push(ProjectionNoteBinding {
                    note_id: document_note_id(projection_id, &note.identity_sha256, budget)?,
                    identity_sha256: note.identity_sha256.clone(),
                    values: note.values,
                });
            }
            let canonical_json = canonical_review(&review, budget)?;
            charge(budget, 4096)?;
            let binding = PerformanceProjectionBinding {
                id: projection_id.into(),
                version: 1,
                source: TakeReference {
                    take_id: TakeId(self.source.take_id.clone()),
                    manifest_sha256: self.source.manifest_sha256.clone(),
                    stream_id: "audio".into(),
                },
                review: RetainedProjectionReview {
                    version: 1,
                    canonical_json,
                    review_sha256: review.review_sha256.clone(),
                },
                lanes,
            };
            outer_credit(&binding, budget)?;
            binding.validate().map_err(invalid)?;
            self.verify_unchanged()?;
            Ok(binding)
        })();
        budget.checkpoint()?;
        result
    }

    /// Validates encoding, then recomputes the complete requested interpretation
    /// from this held original. A self-consistent forged record is insufficient.
    pub fn validate_binding(&self, binding: &PerformanceProjectionBinding) -> Result<()> {
        let budget = self.budget();
        let result = (|| {
            let review = decode_binding(binding, budget)?;
            let actual = self.project(&review.request)?;
            charge(
                budget,
                binding
                    .review
                    .canonical_json
                    .len()
                    .checked_mul(2)
                    .ok_or_else(|| invalid("Projection review comparison work overflow"))?,
            )?;
            require(
                actual == review,
                "Retained performance projection differs from the held original derivation",
            )?;
            self.verify_unchanged()
        })();
        budget.checkpoint()?;
        result
    }
}

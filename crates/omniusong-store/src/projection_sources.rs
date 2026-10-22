//! Registered original ownership beside a borrowed performance authentication.
//! No catalog connection, writer lease or caller-supplied inspection is retained.
use crate::{Result, StoreError, TakeObject, recovered_takes, takes};
use omniusong_core::{ProjectDocument, contracts::TakeId, native::TakeReference};
use omniusong_performance::projection::AuthenticatedPerformance;
use omniusong_recording::{InspectionBudget, SealedJournal};
use rusqlite::{Connection, OptionalExtension};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_REGISTERED_MANIFEST_BYTES: usize = 512 * 1024;

fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(StoreError::Invalid(message.into()))
    }
}
fn charge(budget: &InspectionBudget, bytes: usize) -> Result<()> {
    budget.charge_work(
        u64::try_from(bytes)
            .map_err(|_| StoreError::Invalid("Projection source work overflow".into()))?,
    )?;
    Ok(())
}
fn finish<T>(budget: &InspectionBudget, result: Result<T>) -> Result<T> {
    // Only this module's registration queries use StoreError::Database; source
    // SQLite remains wrapped in RecordingError. A later cancellation cannot
    // hide a fatal failure of the actual live catalog from worker retirement.
    if matches!(&result,Err(StoreError::Database(error)) if error.sqlite_error().is_some_and(|code| matches!(code.extended_code & 0xff,10|11|26)))
    {
        return result;
    }
    budget.checkpoint()?;
    result
}

struct DocumentMeter<'a> {
    budget: &'a InspectionBudget,
    bytes: usize,
    maximum: usize,
}
impl Write for DocumentMeter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .bytes
            .checked_add(bytes.len())
            .filter(|size| *size <= self.maximum)
            .ok_or_else(|| {
                std::io::Error::other("Projection source document exceeds metadata bounds")
            })?;
        self.budget
            .charge_work((bytes.len() as u64).saturating_mul(2))
            .map_err(|_| {
                std::io::Error::other("Projection source governor stopped document traversal")
            })?;
        self.bytes = next;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn prepay_document(document: &ProjectDocument, budget: &InspectionBudget) -> Result<()> {
    budget.checkpoint()?;
    let mut meter = DocumentMeter {
        budget,
        bytes: 0,
        maximum: document.metadata_limit(),
    };
    let encoded = serde_json::to_writer(&mut meter, document.value());
    budget.checkpoint()?;
    encoded.map_err(|error| {
        StoreError::Invalid(format!(
            "Invalid projection source document traversal: {error}"
        ))
    })?;
    // Covers both typed owned views, source-reference/clip copies (including
    // original snapshot extensions), their bounded validation and map storage.
    charge(
        budget,
        meter
            .bytes
            .checked_mul(32)
            .ok_or_else(|| StoreError::Invalid("Projection document copy work overflow".into()))?,
    )
}

#[derive(Default)]
struct Group {
    references: BTreeSet<TakeReference>,
    performance: bool,
}
fn add_reference(
    groups: &mut BTreeMap<TakeId, Group>,
    reference: TakeReference,
    performance: bool,
    budget: &InspectionBudget,
) -> Result<()> {
    budget.visit_records(1)?;
    charge(budget, 2048)?;
    reference.validate()?;
    let group = groups.entry(reference.take_id.clone()).or_default();
    group.references.insert(reference);
    group.performance |= performance;
    Ok(())
}
fn group_document(
    groups: &mut BTreeMap<TakeId, Group>,
    document: &ProjectDocument,
    budget: &InspectionBudget,
) -> Result<()> {
    prepay_document(document, budget)?;
    // These are exactly the two schema-owned paths in referenced_takes().
    // Enumerating them here avoids the legacy guarded store visitor and keeps
    // every original clip's immutable reference without interpreting windows.
    let clips = document.take_clips()?;
    let registry = document.performance_projections()?;
    for clip in clips {
        charge(budget, 1024)?;
        add_reference(groups, clip.source.clone(), false, budget)?;
    }
    if let Some(registry) = registry {
        for entry in registry.entries {
            add_reference(groups, entry.source, true, budget)?;
        }
    }
    budget.checkpoint()?;
    Ok(())
}

/// Streamed reference/role collection only; it retains no document or source
/// proof. Every addition and the eventual open use this original governor.
pub(crate) struct RegisteredSourceSet<'b> {
    budget: &'b InspectionBudget,
    grouped: BTreeMap<TakeId, Group>,
    failed: bool,
}
impl<'b> RegisteredSourceSet<'b> {
    pub(crate) fn new(budget: &'b InspectionBudget) -> Self {
        Self {
            budget,
            grouped: BTreeMap::new(),
            failed: false,
        }
    }
    pub(crate) fn add_document(&mut self, document: &ProjectDocument) -> Result<()> {
        let result = (|| {
            require(!self.failed, "Registered source collection already failed")?;
            group_document(&mut self.grouped, document, self.budget)
        })();
        let result = finish(self.budget, result);
        self.failed |= result.is_err();
        result
    }
    pub(crate) fn add_performance_reference(&mut self, reference: &TakeReference) -> Result<()> {
        let result = (|| {
            self.budget.checkpoint()?;
            require(!self.failed, "Registered source collection already failed")?;
            charge(self.budget, 1024)?;
            reference.validate()?;
            add_reference(&mut self.grouped, reference.clone(), true, self.budget)
        })();
        let result = finish(self.budget, result);
        self.failed |= result.is_err();
        result
    }
}

fn load_registered(
    connection: &Connection,
    take_id: &TakeId,
    budget: &InspectionBudget,
) -> Result<(TakeObject, usize)> {
    budget.checkpoint()?;
    budget.visit_rows(2)?;
    charge(budget, 4096)?;
    let length: Option<i64> = connection
        .query_row(
            "SELECT octet_length(manifest_json) FROM native_takes WHERE take_id=?1",
            [&take_id.0],
            |row| row.get(0),
        )
        .optional()?;
    let length = length
        .and_then(|size| usize::try_from(size).ok())
        .filter(|size| *size <= MAX_REGISTERED_MANIFEST_BYTES)
        .ok_or_else(|| {
            StoreError::Invalid(
                "Registered projection source is absent or its manifest is oversized".into(),
            )
        })?;
    charge(
        budget,
        length
            .checked_mul(64)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(|| StoreError::Invalid("Registered manifest work overflow".into()))?,
    )?;
    let object = takes::load_take(connection, take_id)?;
    budget.checkpoint()?;
    Ok((object, length))
}
fn path_credit(path: &Path, budget: &InspectionBudget) -> Result<()> {
    charge(budget, path.as_os_str().len())?;
    charge(
        budget,
        path.components()
            .count()
            .checked_mul(1024)
            .and_then(|n| {
                path.as_os_str()
                    .len()
                    .checked_mul(8)
                    .and_then(|bytes| n.checked_add(bytes))
            })
            .and_then(|n| n.checked_add(8192))
            .ok_or_else(|| StoreError::Invalid("Projection source path work overflow".into()))?,
    )
}
fn reference_credit(object: &TakeObject, budget: &InspectionBudget) -> Result<()> {
    charge(
        budget,
        object
            .manifest
            .body
            .descriptor
            .streams
            .len()
            .saturating_mul(128)
            .saturating_add(2048),
    )
}
fn clip_credit(object: &TakeObject, budget: &InspectionBudget) -> Result<()> {
    charge(
        budget,
        object
            .manifest
            .body
            .descriptor
            .streams
            .len()
            .saturating_mul(2048)
            .saturating_add(8192),
    )
}
fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> Result<()> {
    require(
        before.is_file() && after.is_file(),
        "Registered projection source ceased to be an ordinary file",
    )?;
    takes::same_file(before, after)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            before.mode() == after.mode()
                && before.nlink() == after.nlink()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "Registered projection source changed during authentication",
        )?;
    }
    Ok(())
}
struct RegisteredSource {
    object: TakeObject,
    references: BTreeSet<TakeReference>,
    reader: SealedJournal,
    path: PathBuf,
    opening: fs::Metadata,
    performance: bool,
}

/// Owns immutable registered objects/readers; it borrows no catalog connection.
/// This holder alone grants no performance-derivation authority.
pub(crate) struct RegisteredProjectionSources<'b> {
    budget: &'b InspectionBudget,
    root: PathBuf,
    sources: BTreeMap<TakeId, RegisteredSource>,
}
/// The proofs borrow their stable reader owner instead of forming a self-
/// referential owner. All operations retain the opening readers' same governor.
pub(crate) struct ProjectionProofs<'a> {
    sources: &'a RegisteredProjectionSources<'a>,
    budget: &'a InspectionBudget,
    performance: BTreeMap<TakeId, AuthenticatedPerformance<'a>>,
}

impl<'b> RegisteredProjectionSources<'b> {
    pub(crate) fn open(
        root: &Path,
        connection: &Connection,
        documents: &[&ProjectDocument],
        extra: &[TakeReference],
        budget: &'b InspectionBudget,
    ) -> Result<Self> {
        let result = (|| {
            budget.checkpoint()?;
            let mut collected = RegisteredSourceSet::new(budget);
            for document in documents {
                collected.add_document(document)?;
            }
            for reference in extra {
                collected.add_performance_reference(reference)?;
            }
            Self::open_collected(root, connection, collected)
        })();
        finish(budget, result)
    }

    pub(crate) fn open_collected(
        root: &Path,
        connection: &Connection,
        collected: RegisteredSourceSet<'b>,
    ) -> Result<Self> {
        let RegisteredSourceSet {
            budget,
            grouped,
            failed,
        } = collected;
        let result = (|| {
            budget.checkpoint()?;
            require(!failed, "Registered source collection already failed")?;
            path_credit(root, budget)?;
            let mut sources = BTreeMap::new();
            for (take_id, group) in grouped {
                budget.checkpoint()?;
                let (object, manifest_length) = load_registered(connection, &take_id, budget)?;
                for reference in &group.references {
                    reference_credit(&object, budget)?;
                    takes::check_reference(&object, reference)?;
                }
                // Performance sources must not compose the existing raw-file
                // hash/full-inspection helper with another complete proof.
                charge(budget, manifest_length.saturating_mul(8))?;
                takes::validate_object(&object)?;
                path_credit(root, budget)?;
                let path = takes::object_path(root, &object.object_sha256)?;
                let opening = takes::ordinary_object(&path)?;
                require(
                    opening.len() == object.byte_length,
                    "Registered source length differs from its catalog object",
                )?;
                let reader = if group.performance {
                    SealedJournal::open_bounded(&path, budget)?
                } else {
                    charge(budget, manifest_length.saturating_mul(8))?;
                    recovered_takes::verify_owned_with_budget(root, &object, budget)?
                };
                require(
                    reader.manifest() == &object.manifest
                        && reader.descriptor().take_id == object.take_id,
                    "Held source manifest or take differs from its registered object",
                )?;
                charge(
                    budget,
                    path.as_os_str()
                        .len()
                        .saturating_mul(2)
                        .saturating_add(1024),
                )?;
                unchanged(&opening, &takes::ordinary_object(&path)?)?;
                charge(
                    budget,
                    std::mem::size_of::<RegisteredSource>().saturating_add(512),
                )?;
                sources.insert(
                    take_id,
                    RegisteredSource {
                        object,
                        references: group.references,
                        reader,
                        path,
                        opening,
                        performance: group.performance,
                    },
                );
            }
            charge(budget, root.as_os_str().len())?;
            let sources = Self {
                budget,
                root: root.to_owned(),
                sources,
            };
            sources.verify_with_budget(budget)?;
            Ok(sources)
        })();
        finish(budget, result)
    }

    fn verify_with_budget(&self, budget: &InspectionBudget) -> Result<()> {
        let result = (|| {
            budget.checkpoint()?;
            require(
                self.budget.shares_governor(budget),
                "Projection source verification cannot change the cumulative inspection governor",
            )?;
            for source in self.sources.values() {
                let opening_budget = source
                    .reader
                    .inspection_budget()
                    .expect("all registered projection readers are bounded");
                opening_budget.checkpoint()?;
                require(
                    opening_budget.shares_governor(budget),
                    "Projection source verification cannot change the cumulative inspection governor",
                )?;
                path_credit(&self.root, budget)?;
                let current_path = takes::object_path(&self.root, &source.object.object_sha256)?;
                require(
                    current_path.as_os_str() == source.path.as_os_str(),
                    "Registered projection source directory was redirected",
                )?;
                charge(
                    budget,
                    source
                        .path
                        .as_os_str()
                        .len()
                        .saturating_mul(2)
                        .saturating_add(1024),
                )?;
                let metadata = takes::ordinary_object(&source.path)?;
                require(
                    metadata.len() == source.object.byte_length,
                    "Registered source length changed",
                )?;
                unchanged(&source.opening, &metadata)?;
                source.reader.verify_source_identity()?;
                charge(
                    budget,
                    source
                        .path
                        .as_os_str()
                        .len()
                        .saturating_mul(2)
                        .saturating_add(1024),
                )?;
                unchanged(&source.opening, &takes::ordinary_object(&source.path)?)?;
            }
            Ok(())
        })();
        finish(budget, result)
    }

    pub(crate) fn authenticate<'a>(
        &'a self,
        budget: &'a InspectionBudget,
    ) -> Result<ProjectionProofs<'a>> {
        let result = (|| {
            self.verify_with_budget(budget)?;
            let mut performance = BTreeMap::new();
            for source in self.sources.values().filter(|source| source.performance) {
                charge(
                    budget,
                    std::mem::size_of::<AuthenticatedPerformance<'_>>().saturating_add(1024),
                )?;
                let proof = AuthenticatedPerformance::open(&source.reader)?;
                let pin = proof.source_pin();
                require(
                    pin.take_id == source.object.take_id.0
                        && pin.manifest_sha256 == source.object.manifest_sha256
                        && pin.descriptor_sha256 == source.object.manifest.body.descriptor_sha256
                        && pin.journal_sha256_at_review == source.object.object_sha256
                        && pin.selection.chunk_count == source.object.manifest.body.chunk_count
                        && pin.selection.chunk_index_sha256
                            == source.object.manifest.body.chunk_index_sha256,
                    "Authenticated performance differs from its registered physical original or manifest",
                )?;
                performance.insert(source.object.take_id.clone(), proof);
            }
            let proofs = ProjectionProofs {
                sources: self,
                budget,
                performance,
            };
            proofs.verify_unchanged()?;
            Ok(proofs)
        })();
        finish(budget, result)
    }
}

impl<'a> ProjectionProofs<'a> {
    pub(crate) fn verify_registrations(&self, connection: &Connection) -> Result<()> {
        let result = (|| {
            for source in self.sources.sources.values() {
                let (current, _) =
                    load_registered(connection, &source.object.take_id, self.budget)?;
                require(
                    current == source.object,
                    "Registered source changed during the project transaction",
                )?;
            }
            Ok(())
        })();
        finish(self.budget, result)
    }
    fn registered(&self, reference: &TakeReference) -> Result<&RegisteredSource> {
        self.budget.checkpoint()?;
        charge(self.budget, 4096)?;
        reference.validate()?;
        let source = self
            .sources
            .sources
            .get(&reference.take_id)
            .ok_or_else(|| {
                StoreError::Invalid(
                    "Document references a take outside its held registered source set".into(),
                )
            })?;
        require(
            source.references.contains(reference),
            "Document references a stream or manifest outside the exact opened reference set",
        )?;
        reference_credit(&source.object, self.budget)?;
        takes::check_reference(&source.object, reference)?;
        Ok(source)
    }
    pub(crate) fn performance(
        &self,
        reference: &TakeReference,
    ) -> Result<&AuthenticatedPerformance<'a>> {
        let result = (|| {
            self.registered(reference)?;
            self.performance.get(&reference.take_id).ok_or_else(|| {
                StoreError::Invalid("Take has no held performance authentication".into())
            })
        })();
        finish(self.budget, result)
    }
    pub(crate) fn validate_document(&self, document: &ProjectDocument) -> Result<()> {
        self.validate_document_parts(document, true)
    }
    /// Reference/geometry admission only. Callers separately prove exact
    /// registry equality/subset with a document whose bindings were validated.
    pub(crate) fn validate_document_geometry(&self, document: &ProjectDocument) -> Result<()> {
        self.validate_document_parts(document, false)
    }
    fn validate_document_parts(
        &self,
        document: &ProjectDocument,
        validate_bindings: bool,
    ) -> Result<()> {
        let result = (|| {
            prepay_document(document, self.budget)?;
            let clips = document.take_clips()?;
            let registry = document.performance_projections()?;
            // Resolve all exact references before interpreting either geometry
            // or retained note bindings. Opening their union was not admission
            // of a caller's new window or document.
            for clip in &clips {
                self.registered(&clip.source)?;
            }
            if let Some(registry) = &registry {
                for entry in &registry.entries {
                    self.registered(&entry.source)?;
                }
            }
            for clip in &clips {
                let source = self.registered(&clip.source)?;
                clip_credit(&source.object, self.budget)?;
                takes::check_clip(&source.object, clip)?;
            }
            if let Some(registry) = registry.filter(|_| validate_bindings) {
                for entry in registry.entries {
                    self.performance(&entry.source)?.validate_binding(&entry)?;
                }
            }
            self.verify_unchanged()
        })();
        finish(self.budget, result)
    }
    pub(crate) fn verify_unchanged(&self) -> Result<()> {
        self.sources.verify_with_budget(self.budget)
    }
}

#[cfg(test)]
mod collector_tests {
    use super::*;
    use omniusong_core::native::{TakeClip, TakeRange, TimelinePosition};
    use omniusong_recording::{InspectionLimits, RecordingError};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn budget(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        (
            InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
            cancelled,
        )
    }
    fn reference(take: &str, stream: &str, digest: char) -> TakeReference {
        TakeReference {
            take_id: TakeId(take.into()),
            manifest_sha256: digest.to_string().repeat(64),
            stream_id: stream.into(),
        }
    }
    fn document(clips: &[TakeReference], performance: &[TakeReference]) -> ProjectDocument {
        let mut document = ProjectDocument::new("Streamed registered sources")
            .unwrap()
            .upgrade_native()
            .unwrap();
        for (index, source) in clips.iter().enumerate() {
            document
                .append_take_clip(
                    "original-recordings",
                    "Original recordings",
                    TakeClip {
                        id: format!("clip-{index}"),
                        source: source.clone(),
                        range: TakeRange::AudioFrames { start: 0, count: 1 },
                        position: TimelinePosition {
                            frame: 0,
                            sample_rate: 48_000,
                        },
                        fades: None,
                        edit_source: None,
                    },
                )
                .unwrap();
        }
        if performance.is_empty() {
            return document;
        }
        let mut value = document
            .upgrade_performance_projection_schema()
            .unwrap()
            .value()
            .clone();
        value["performanceProjections"]["entries"] = performance.iter().enumerate().map(|(index, source)| {
            let digest = "d".repeat(64);
            let review = json!({"version":1,"review_sha256":digest,"request":{"source":{
                "take_id":source.take_id.0,"manifest_sha256":source.manifest_sha256
            }}}).to_string();
            json!({"id":format!("projection-{index}"),"version":1,"source":source,
                "review":{"version":1,"canonicalJson":review,"reviewSha256":digest},
                "lanes":[{"source":1,"trackId":format!("reserved-lane-{index}"),"instrument":"keys",
                    "originalNotes":[{"noteId":format!("reserved-note-{index}"),"identitySha256":"e".repeat(64),
                        "values":{"pitch":60,"start_tick":0,"duration_ticks":1,"velocity16":1}}]}]})
        }).collect::<Vec<_>>().into();
        ProjectDocument::from_value(value).unwrap()
    }
    #[test]
    fn streamed_documents_union_roles_and_deduplicate_exact_references() {
        let (budget, _) = budget(InspectionLimits::default());
        let audio = reference("take-one", "audio", 'a');
        let control = reference("take-one", "control", 'a');
        let other = reference("take-two", "audio", 'b');
        let ordinary = document(&[audio.clone(), audio.clone(), other.clone()], &[]);
        let projected = document(std::slice::from_ref(&control), std::slice::from_ref(&audio));
        let mut collected = RegisteredSourceSet::new(&budget);
        collected.add_document(&ordinary).unwrap();
        collected.add_document(&ordinary).unwrap();
        assert!(!collected.grouped[&audio.take_id].performance);
        collected.add_document(&projected).unwrap();
        assert_eq!(collected.grouped.len(), 2);
        let group = &collected.grouped[&audio.take_id];
        assert!(group.performance);
        assert_eq!(group.references, BTreeSet::from([audio, control]));
        assert!(!collected.grouped[&other.take_id].performance);
        assert_eq!(
            collected.grouped[&other.take_id].references,
            BTreeSet::from([other])
        );
        // Deduplication saves retained state, never traversal work.
        assert_eq!(budget.usage().record_visits, 8);
    }
    #[test]
    fn empty_collected_sources_cannot_rebind_prepaid_work_to_a_fresh_governor() {
        let connection = Connection::open_in_memory().unwrap();
        let (opening, _) = budget(InspectionLimits::default());
        let (other, _) = budget(InspectionLimits::default());
        let mut collected = RegisteredSourceSet::new(&opening);
        collected.add_document(&document(&[], &[])).unwrap();
        let sources = RegisteredProjectionSources::open_collected(
            Path::new("unused-empty-source-root"),
            &connection,
            collected,
        )
        .unwrap();
        assert!(opening.usage().work_bytes > 0);
        assert!(
            matches!(sources.authenticate(&other), Err(StoreError::Invalid(message))
            if message == "Projection source verification cannot change the cumulative inspection governor")
        );
        assert_eq!(other.usage(), Default::default());
        sources
            .authenticate(&opening)
            .unwrap()
            .verify_unchanged()
            .unwrap();
        assert_eq!(opening.usage().file_bytes, 0);
    }

    #[test]
    fn schema_one_opaque_lookalikes_do_not_claim_sources_or_performance_roles() {
        let (budget, _) = budget(InspectionLimits::default());
        let source = reference("take-opaque", "audio", 'a');
        let mut value = ProjectDocument::new("Opaque legacy metadata")
            .unwrap()
            .value()
            .clone();
        value["performanceProjections"] = json!({"version":999,"entries":[{"source":source}]});
        value["tracks"][0]["recording"] = json!({"version":1,"clips":[{"source":source}]});
        let document = ProjectDocument::from_value(value).unwrap();
        let mut collected = RegisteredSourceSet::new(&budget);
        collected.add_document(&document).unwrap();
        assert!(collected.grouped.is_empty());
        assert_eq!(budget.usage().record_visits, 0);
        assert!(budget.usage().work_bytes > 0);
        collected.add_performance_reference(&source).unwrap();
        assert!(collected.grouped[&source.take_id].performance);
        assert_eq!(
            collected.grouped[&source.take_id].references,
            BTreeSet::from([source])
        );
    }
    #[test]
    fn repeated_streamed_additions_share_record_limits_and_cancellation() {
        let (budget, _) = budget(InspectionLimits {
            max_record_visits: 2,
            ..Default::default()
        });
        let source = reference("take-one", "audio", 'a');
        let mut collected = RegisteredSourceSet::new(&budget);
        collected.add_performance_reference(&source).unwrap();
        collected.add_performance_reference(&source).unwrap();
        assert!(matches!(
            collected.add_performance_reference(&source),
            Err(StoreError::Recording(RecordingError::InspectionLimit {
                resource: omniusong_recording::InspectionResource::RecordVisits,
                limit: 2,
                attempted: 3
            }))
        ));
        assert_eq!(collected.grouped.len(), 1);
        assert_eq!(budget.usage().record_visits, 2);
        assert!(collected.failed);
        assert!(collected.add_document(&document(&[], &[])).is_err());

        let (budget, cancelled) = self::budget(InspectionLimits::default());
        let mut collected = RegisteredSourceSet::new(&budget);
        collected.add_performance_reference(&source).unwrap();
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            collected.add_document(&document(&[], &[])),
            Err(StoreError::Recording(RecordingError::InspectionCancelled))
        ));
        cancelled.store(false, Ordering::Release);
        assert!(matches!(
            collected.add_performance_reference(&source),
            Err(StoreError::Recording(RecordingError::InspectionCancelled))
        ));
        assert_eq!(budget.usage().record_visits, 1);
    }
    #[test]
    fn streamed_documents_charge_cumulative_work_and_refused_addition_cannot_be_reused() {
        let document = document(&[reference("take-one", "audio", 'a')], &[]);
        let (measurement, _) = budget(InspectionLimits::default());
        RegisteredSourceSet::new(&measurement)
            .add_document(&document)
            .unwrap();
        let first_work = measurement.usage().work_bytes;
        assert!(first_work > 0);
        let (limited, _) = budget(InspectionLimits {
            max_work_bytes: first_work,
            ..Default::default()
        });
        let mut collected = RegisteredSourceSet::new(&limited);
        collected.add_document(&document).unwrap();
        assert!(matches!(
            collected.add_document(&document),
            Err(StoreError::Recording(RecordingError::InspectionLimit {
                resource: omniusong_recording::InspectionResource::WorkBytes,
                ..
            }))
        ));
        assert!(collected.failed);

        let (healthy, _) = budget(InspectionLimits::default());
        let mut collected = RegisteredSourceSet::new(&healthy);
        let mut invalid = reference("take-invalid", "audio", 'a');
        invalid.manifest_sha256 = "INVALID".into();
        assert!(collected.add_performance_reference(&invalid).is_err());
        assert!(
            collected
                .add_performance_reference(&reference("take-valid", "audio", 'a'))
                .is_err()
        );
        assert!(collected.grouped.is_empty());
        healthy.checkpoint().unwrap();
    }
    #[test]
    fn conflicting_same_take_manifests_survive_collection_and_refuse_registration_before_paths() {
        use sha2::{Digest, Sha256};
        let descriptor: omniusong_recording::TakeDescriptor = serde_json::from_value(json!({
            "version":1,"take_id":"take-registered","streams":[{
                "stream_id":"audio","segment_id":"segment","source_id":"source","source_name":"Synthetic",
                "source_connection_generation":1,"host_clock":{"domain":"host","epoch":"epoch",
                    "ticks_per_second_numerator":1000,"ticks_per_second_denominator":1},
                "kind":{"kind":"raw_events","format":"length_prefixed_capture_v1"},"initial_source_position":0
            }],"provenance":{}
        })).unwrap();
        let body = omniusong_recording::ManifestBody {
            version: 1,
            manifest_id: "synthetic-manifest".into(),
            operation_id: "synthetic-operation".into(),
            descriptor_sha256: descriptor.sha256().unwrap(),
            descriptor,
            streams: vec![],
            chunk_count: 0,
            payload_bytes: 0,
            chunk_index_sha256: "a".repeat(64),
            interrupted: None,
        };
        let mut hash = Sha256::new();
        hash.update(b"OmniuSong.sealed-take.v1\0");
        hash.update(serde_json::to_vec(&body).unwrap());
        let manifest = omniusong_recording::SealedManifest {
            body,
            sha256: format!("{:x}", hash.finalize()),
        };
        manifest.verify_hash().unwrap();
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE native_takes(take_id TEXT, manifest_sha256 TEXT, object_sha256 TEXT, byte_length INTEGER, manifest_json TEXT)").unwrap();
        connection
            .execute(
                "INSERT INTO native_takes VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params![
                    "take-registered",
                    manifest.sha256,
                    "c".repeat(64),
                    1,
                    String::from_utf8(manifest.canonical_bytes().unwrap()).unwrap()
                ],
            )
            .unwrap();
        let valid = TakeReference {
            take_id: TakeId("take-registered".into()),
            manifest_sha256: manifest.sha256.clone(),
            stream_id: "audio".into(),
        };
        let mut conflicting = valid.clone();
        conflicting.manifest_sha256 = if valid.manifest_sha256 == "b".repeat(64) {
            "a".repeat(64)
        } else {
            "b".repeat(64)
        };
        let (budget, _) = budget(InspectionLimits::default());
        let mut collected = RegisteredSourceSet::new(&budget);
        collected
            .add_document(&document(std::slice::from_ref(&valid), &[]))
            .unwrap();
        collected.add_performance_reference(&conflicting).unwrap();
        assert_eq!(collected.grouped[&valid.take_id].references.len(), 2);
        let result = RegisteredProjectionSources::open_collected(
            Path::new("never-opened-synthetic-root"),
            &connection,
            collected,
        );
        assert!(matches!(result, Err(StoreError::Invalid(message))
            if message == "Take reference does not match its registered sealed identity"));
        assert_eq!(budget.usage().file_bytes, 0);
    }
}

#[cfg(test)]
mod error_domain_tests {
    use super::*;
    use omniusong_recording::{InspectionLimits, RecordingError};
    use std::sync::{Arc, atomic::AtomicBool};
    fn sqlite(code: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some("synthetic fault domain".into()),
        )
    }
    #[test]
    fn late_cancellation_cannot_erase_actual_catalog_fatal_codes() {
        for code in [10, 11, 26, 10 | (33 << 8)] {
            let budget =
                InspectionBudget::new(InspectionLimits::default(), Arc::new(AtomicBool::new(true)))
                    .unwrap();
            let result: Result<()> = finish(&budget, Err(StoreError::Database(sqlite(code))));
            match result {
                Err(StoreError::Database(error)) => {
                    assert_eq!(error.sqlite_error().unwrap().extended_code, code)
                }
                other => panic!("Lost main-catalog failure: {other:?}"),
            }
            assert!(matches!(
                budget.checkpoint(),
                Err(RecordingError::InspectionCancelled)
            ));
        }
    }
    #[test]
    fn source_sqlite_does_not_acquire_live_catalog_failure_authority() {
        let budget = InspectionBudget::new(
            InspectionLimits::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let result: Result<()> = finish(
            &budget,
            Err(StoreError::Recording(RecordingError::Database(sqlite(11)))),
        );
        assert!(matches!(
            result,
            Err(StoreError::Recording(RecordingError::Database(_)))
        ));
        let stopped =
            InspectionBudget::new(InspectionLimits::default(), Arc::new(AtomicBool::new(true)))
                .unwrap();
        let result: Result<()> = finish(
            &stopped,
            Err(StoreError::Recording(RecordingError::Database(sqlite(11)))),
        );
        assert!(matches!(
            result,
            Err(StoreError::Recording(RecordingError::InspectionCancelled))
        ));
    }
    #[test]
    fn actual_catalog_fault_survives_exhausted_work_but_other_results_remain_stopped() {
        let budget = InspectionBudget::new(
            InspectionLimits {
                max_work_bytes: 0,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(budget.charge_work(1).is_err());
        let result: Result<()> = finish(&budget, Err(StoreError::Database(sqlite(26))));
        assert!(matches!(result, Err(StoreError::Database(_))));
        let result: Result<()> = finish(&budget, Ok(()));
        assert!(matches!(
            result,
            Err(StoreError::Recording(
                RecordingError::InspectionLimit { .. }
            ))
        ));
        let result: Result<()> = finish(
            &budget,
            Err(StoreError::Invalid("ordinary metadata refusal".into())),
        );
        assert!(matches!(
            result,
            Err(StoreError::Recording(
                RecordingError::InspectionLimit { .. }
            ))
        ));
    }
}

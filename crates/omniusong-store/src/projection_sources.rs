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
pub(crate) struct RegisteredProjectionSources {
    root: PathBuf,
    sources: BTreeMap<TakeId, RegisteredSource>,
}
/// The proofs borrow their stable reader owner instead of forming a self-
/// referential owner. All operations retain the opening readers' same governor.
pub(crate) struct ProjectionProofs<'a> {
    sources: &'a RegisteredProjectionSources,
    budget: &'a InspectionBudget,
    performance: BTreeMap<TakeId, AuthenticatedPerformance<'a>>,
}

impl RegisteredProjectionSources {
    pub(crate) fn open(
        root: &Path,
        connection: &Connection,
        documents: &[&ProjectDocument],
        extra: &[TakeReference],
        budget: &InspectionBudget,
    ) -> Result<Self> {
        let result = (|| {
            budget.checkpoint()?;
            path_credit(root, budget)?;
            let mut grouped = BTreeMap::new();
            for document in documents {
                group_document(&mut grouped, document, budget)?;
            }
            for reference in extra {
                charge(budget, 1024)?;
                reference.validate()?;
                add_reference(&mut grouped, reference.clone(), true, budget)?;
            }
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

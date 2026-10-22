//! Explicit bounded ingestion for an authenticated recovered working copy.
use crate::{NativeStore, Result, StoreError, TakeObject, takes};
use omniusong_core::native::TakeReference;
use omniusong_recording::{InspectionBudget, RecoveredCopyView, SealedJournal};
use rusqlite::{TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(StoreError::Invalid(message.into()))
    }
}
fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> Result<()> {
    takes::same_file(before, after)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "Owned object changed during bounded proof",
        )?;
    }
    Ok(())
}
pub(crate) fn verify_owned_with_budget(
    root: &Path,
    object: &TakeObject,
    budget: &InspectionBudget,
) -> Result<SealedJournal> {
    budget.checkpoint()?;
    takes::validate_object(object)?;
    let path = takes::object_path(root, &object.object_sha256)?;
    let before = takes::ordinary_object(&path)?;
    require(
        before.len() == object.byte_length,
        "Owned object length differs from registration",
    )?;
    let mut file = File::open(&path)?;
    unchanged(&before, &file.metadata()?)?;
    let sealed = SealedJournal::open_bounded(&path, budget)?;
    require(
        sealed.manifest() == &object.manifest && sealed.descriptor().take_id == object.take_id,
        "Owned object manifest differs from registration",
    )?;
    let inspection = sealed.inspect_snapshot()?;
    require(
        inspection.manifest.as_ref() == Some(&object.manifest),
        "Owned full-original inspection differs from registration",
    )?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        budget.checkpoint()?;
        let size = object
            .byte_length
            .saturating_sub(total)
            .saturating_add(1)
            .min(buffer.len() as u64) as usize;
        budget.charge_work(size as u64)?;
        let n = file.read(&mut buffer[..size])?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| StoreError::Invalid("Owned byte count overflow".into()))?;
        require(
            total <= object.byte_length,
            "Owned object grew during bounded proof",
        )?;
        budget.charge_work(n as u64)?;
        hash.update(&buffer[..n]);
    }
    require(
        total == object.byte_length && format!("{:x}", hash.finalize()) == object.object_sha256,
        "Owned object full-file SHA differs from registration",
    )?;
    unchanged(&before, &file.metadata()?)?;
    unchanged(&before, &takes::ordinary_object(&path)?)?;
    budget.checkpoint()?;
    Ok(sealed)
}
fn matches_view(sealed: &SealedJournal, view: &RecoveredCopyView<'_>) -> Result<()> {
    let source = view.working_reader().inspection();
    sealed.validate_recovered_copy_schema()?;
    let inspection = sealed.inspect_snapshot()?;
    require(
        inspection.descriptor == source.descriptor
            && inspection.descriptor_sha256 == source.descriptor_sha256
            && inspection.original_rows_sha256 == source.original_rows_sha256
            && inspection.manifest == source.manifest
            && inspection.report.unknown_tail,
        "Registered recovered object differs from full original or selected manifest",
    )
}
impl NativeStore {
    pub fn load_with_budget(
        &self,
        id: &str,
        budget: &InspectionBudget,
    ) -> Result<omniusong_core::ProjectDocument> {
        budget.checkpoint()?;
        crate::catalog_transaction::CatalogTransaction::deferred(&self.connection, budget)?.run(
            |tx| {
                if crate::projection_metadata::project(tx, id, budget)? {
                    let document = crate::read_document_metadata_with_budget(tx, id, budget)?;
                    return crate::active_projection_edits::load(tx, &self.root, document, budget);
                }
                tx.release_governor_for_legacy()?;
                let document = crate::load_document(tx, id)?;
                tx.rollback()?;
                crate::check_references(&self.connection, &document)?;
                takes::verify_document_files_with_budget(
                    &self.root,
                    &self.connection,
                    &document,
                    budget,
                )?;
                budget.checkpoint()?;
                Ok(document)
            },
        )
    }
    pub fn open_take_with_budget(
        &self,
        reference: &TakeReference,
        budget: &InspectionBudget,
    ) -> Result<SealedJournal> {
        let object = takes::load_take(&self.connection, &reference.take_id)?;
        takes::check_reference(&object, reference)?;
        verify_owned_with_budget(&self.root, &object, budget)
    }
    /// Retains any exclusive partial/orphan on failure. The caller holds the
    /// action lease and recovered view through the later project transaction.
    pub fn ingest_recovered_copy(&mut self, view: &RecoveredCopyView<'_>) -> Result<TakeObject> {
        let _writer = crate::retained_file_coordination::writer(&self.root)?;
        view.verify_unchanged()?;
        let source = view.working_reader().inspection();
        let take_id = &source.descriptor.take_id;
        if takes::has_take(&self.connection, take_id)? {
            let object = takes::load_take(&self.connection, take_id)?;
            let sealed = verify_owned_with_budget(&self.root, &object, view.budget())?;
            matches_view(&sealed, view)?;
            view.verify_unchanged()?;
            return Ok(object);
        }
        let directory = takes::takes_directory(&self.root, true)?;
        let staging = directory.join(format!(
            ".recovered-ingest-{}.partial",
            uuid::Uuid::new_v4()
        ));
        let backup = view.export_working_copy(&staging)?;
        let object = TakeObject {
            take_id: backup.take_id,
            manifest_sha256: backup.manifest_sha256,
            object_sha256: backup.sha256,
            byte_length: backup.byte_len,
            manifest: source
                .manifest
                .clone()
                .ok_or_else(|| StoreError::Invalid("Recovered view lacks manifest".into()))?,
        };
        takes::validate_object(&object)?;
        let destination = takes::object_path(&self.root, &object.object_sha256)?;
        match fs::hard_link(&staging, &destination) {
            Ok(()) => fs::remove_file(&staging)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let sealed = verify_owned_with_budget(&self.root, &object, view.budget())?;
                matches_view(&sealed, view)?;
                fs::remove_file(&staging)?;
            }
            Err(error) => return Err(error.into()),
        }
        File::open(&directory)?.sync_all()?;
        let sealed = verify_owned_with_budget(&self.root, &object, view.budget())?;
        matches_view(&sealed, view)?;
        view.verify_unchanged()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if takes::has_take(&tx, take_id)? {
            let existing = takes::load_take(&tx, take_id)?;
            let owned = verify_owned_with_budget(&self.root, &existing, view.budget())?;
            matches_view(&owned, view)?;
            view.verify_unchanged()?;
            tx.commit()?;
            return Ok(existing);
        }
        let bytes = i64::try_from(object.byte_length)
            .map_err(|_| StoreError::Invalid("Owned size out of range".into()))?;
        let manifest = String::from_utf8(object.manifest.canonical_bytes()?)
            .map_err(|_| StoreError::Invalid("Manifest UTF8".into()))?;
        view.budget().checkpoint()?;
        tx.execute("INSERT INTO native_takes(take_id,manifest_sha256,object_sha256,byte_length,manifest_json) VALUES(?1,?2,?3,?4,?5)",params![object.take_id.0,object.manifest_sha256,object.object_sha256,bytes,manifest])?;
        view.verify_unchanged()?;
        tx.commit()?;
        Ok(object)
    }
}

//! Explicit raw working-copy export; read-only reconciliation never calls this.
use super::*;
use crate::BackupReceipt;

fn no_destination_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal", ".owner.lock"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        require(
            fs::symlink_metadata(PathBuf::from(name))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "Recovered export refuses an existing destination sidecar or owner",
        )?;
    }
    Ok(())
}

fn same_created_object(before: &Stamp, after: &Stamp) -> bool {
    #[cfg(unix)]
    {
        before.dev == after.dev && before.ino == after.ino
    }
    #[cfg(not(unix))]
    {
        let _ = (before, after);
        false
    }
}

fn check_destination(file: &File, frozen: &Stamp, path: &Path) -> Result<()> {
    directory(
        path.parent()
            .ok_or_else(|| RecordingError::Invalid("Missing export parent".into()))?,
    )?;
    ordinary(path)?;
    no_destination_sidecars(path)?;
    require(
        stamp(&file.metadata()?)? == *frozen
            && stamp(&fs::symlink_metadata(path)?)? == *frozen
            && fs::canonicalize(path)? == path,
        "Recovered export destination changed after exclusive creation",
    )
}

impl RecoveredCopyView<'_> {
    /// Explicitly copy every byte of the authenticated sealed working file to
    /// a new ordinary destination. This does not export the separate evidence
    /// image/history, register a take, attach a clip or mint a new manifest.
    ///
    /// The absolute destination must have an existing canonical trusted parent
    /// outside this action's evidence/working namespace. All read/hash/write
    /// passes share this view's governor. Errors retain the destination, even
    /// when cancellation occurs after all bytes have been written and synced;
    /// callers must retain its known path and must not assume rollback.
    pub fn export_working_copy(&self, destination: impl AsRef<Path>) -> Result<BackupReceipt> {
        let result = (|| {
            self.budget.checkpoint()?;
            let destination = destination.as_ref();
            let parent = destination
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .ok_or_else(|| RecordingError::Invalid("Export needs an absolute parent".into()))?;
            directory(parent)?;
            require(
                destination.file_name().is_some()
                    && !destination.starts_with(&self.owner.paths.action),
                "Export destination must be outside the retained recovery action",
            )?;
            no_destination_sidecars(destination)?;
            self.verify_unchanged()?;
            self.budget.checkpoint()?;
            // The source proof may be lengthy. Revalidate the destination
            // namespace immediately before its first filesystem mutation.
            directory(parent)?;
            no_destination_sidecars(destination)?;
            let mut target = exclusive_file(destination)?;
            let created = stamp(&target.metadata()?)?;
            let anchors = self.working.anchors.as_ref().ok_or_else(|| {
                RecordingError::Invalid("Missing held recovered working identity".into())
            })?;
            // copy_file charges bounded reads, writes and hashing separately,
            // and verifies exact length/digest against this held postimage.
            copy_file(
                &anchors.data,
                &mut target,
                &self.working.inspection,
                &self.budget,
            )?;
            let frozen = stamp(&target.metadata()?)?;
            require(
                same_created_object(&created, &frozen),
                "Recovered export lost its exclusively created file",
            )?;
            check_destination(&target, &frozen, destination)?;
            target.sync_all()?;
            sync_directory(parent)?;
            verify_image(
                &target,
                &frozen,
                destination,
                &self.working.inspection,
                &self.budget,
            )?;
            self.verify_unchanged()?;
            // Recheck the destination after the potentially lengthy source
            // proof; never acknowledge a changed pathname or newly hot sidecar.
            check_destination(&target, &frozen, destination)?;
            self.budget.checkpoint()?;
            let manifest = self.working.inspection.manifest.as_ref().ok_or_else(|| {
                RecordingError::Invalid("Recovered working manifest is missing".into())
            })?;
            Ok(BackupReceipt {
                path: destination.to_owned(),
                byte_len: self.working.inspection.byte_length,
                sha256: self.working.inspection.journal_sha256.clone(),
                manifest_sha256: manifest.sha256.clone(),
                take_id: self.working.inspection.descriptor.take_id.clone(),
            })
        })();
        self.budget.finish(result)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn destination_anchor_rejects_same_byte_replacement_and_added_sidecar() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime/recovered-export-anchor-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let path = root.join("export.sqlite3");
        fs::write(&path, b"retained synthetic bytes").unwrap();
        let file = File::open(&path).unwrap();
        let frozen = stamp(&file.metadata().unwrap()).unwrap();
        check_destination(&file, &frozen, &path).unwrap();
        let sidecar = root.join("export.sqlite3-journal");
        fs::write(&sidecar, b"retained sidecar").unwrap();
        assert!(check_destination(&file, &frozen, &path).is_err());
        fs::rename(&sidecar, root.join("saved-sidecar")).unwrap();
        fs::rename(&path, root.join("saved-export")).unwrap();
        fs::write(&path, b"retained synthetic bytes").unwrap();
        assert!(check_destination(&file, &frozen, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"retained synthetic bytes");
        drop(file);
        fs::remove_dir_all(root).unwrap();
    }
}

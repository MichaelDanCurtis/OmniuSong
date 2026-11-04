//! Read-only authentication of a completed inert OSRE extraction. The recorded
//! archive path is provenance only and is never opened or required to exist.
use super::*;
use serde::de::DeserializeOwned;

const MAX_COMPLETION_BYTES: u64 = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InertInspectionBoundary {
    AfterManifestRead,
    AfterObjectRead(usize),
    BeforeFinalAnchors,
}

fn absolute_provenance(path: &Path) -> Result<()> {
    let text = path
        .to_str()
        .ok_or_else(|| StoreError::Invalid("Inert evidence path provenance is not UTF8".into()))?;
    require(
        text.len() <= 4096
            && path.is_absolute()
            && path.file_name().is_some()
            && !text.chars().any(char::is_control),
        "Inert evidence path provenance is not bounded absolute text",
    )
}

// Conservative credit covers the ancestor walks/canonicalization and repeated
// stat observations inside the shared ordinary-file/directory anchor helpers.
fn anchor_credit(path: &Path, budget: &InspectionBudget) -> Result<()> {
    for ancestor in path.ancestors() {
        budget.charge_work((ancestor.as_os_str().len() as u64 + 160) * 8)?;
        budget.visit_rows(8)?;
    }
    Ok(())
}

fn private_directory(path: &Path, budget: &InspectionBudget) -> Result<fs::Metadata> {
    anchor_credit(path, budget)?;
    ordinary_directory(path)?;
    require(
        fs::canonicalize(path)?.as_os_str() == path.as_os_str(),
        "Inert evidence directory is not its exact canonical path",
    )?;
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.mode() & 0o077 == 0,
            "Inert evidence directory is not private",
        )?;
    }
    require(
        metadata.is_dir(),
        "Inert evidence member is not a directory",
    )?;
    Ok(metadata)
}

fn directory_unchanged(
    path: &Path,
    before: &fs::Metadata,
    budget: &InspectionBudget,
) -> Result<()> {
    let after = private_directory(path, budget)?;
    require(
        before.is_dir() && after.is_dir(),
        "Inert evidence directory became nonordinary",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.len() == after.len()
                && before.mode() == after.mode()
                && before.nlink() == after.nlink()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "Inert evidence directory changed during inspection",
        )?;
    }
    Ok(())
}

fn top_members(directory: &Path, budget: &InspectionBudget) -> Result<()> {
    anchor_credit(directory, budget)?;
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        budget.checkpoint()?;
        require(names.len() < 3, "Surplus inert evidence top-level member")?;
        budget.charge_work(4096 + 160)?;
        budget.visit_rows(1)?;
        let name = entry?.file_name().into_string().map_err(|_| {
            StoreError::Invalid("Inert evidence top-level member is not UTF8".into())
        })?;
        require(names.insert(name), "Duplicate inert evidence member")?;
    }
    require(
        names
            == BTreeSet::from([
                "complete.json".into(),
                "manifest.json".into(),
                "objects".into(),
            ]),
        "Inert evidence lacks exact completion/manifest/object membership",
    )
}

fn anchors_credit(anchors: &RestoreAnchors, budget: &InspectionBudget) -> Result<()> {
    anchor_credit(&anchors.directory, budget)?;
    anchor_credit(&anchors.directory.join("objects"), budget)
}

fn members(
    anchors: &RestoreAnchors,
    manifest: &EvidenceManifest,
    budget: &InspectionBudget,
) -> Result<()> {
    anchors_credit(anchors, budget)?;
    anchors.verify()?;
    top_members(&anchors.directory, budget)?;
    fn object_names(path: &Path, budget: &InspectionBudget) -> Result<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(path)? {
            budget.checkpoint()?;
            require(
                names.len() < MAX_EVIDENCE_ENTRIES,
                "Surplus inert evidence objects",
            )?;
            // Filename conversion and set allocation are prepaid per entry;
            // no attacker-controlled name is allocated before this credit.
            budget.charge_work(4096 + 160)?;
            budget.visit_rows(1)?;
            let name = entry?.file_name().into_string().map_err(|_| {
                StoreError::Invalid("Inert evidence object name is not UTF8".into())
            })?;
            require(names.insert(name), "Duplicate inert evidence object name")?;
        }
        Ok(names)
    }
    budget.charge_work(manifest.entries.len() as u64 * 160)?;
    let expected = manifest
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.kind == EvidenceEntryKind::File)
        .map(|(index, _)| format!("{index:04}.bin"))
        .collect::<BTreeSet<_>>();
    require(
        object_names(&anchors.directory.join("objects"), budget)? == expected,
        "Inert evidence numbered object membership differs from the manifest",
    )?;
    anchors_credit(anchors, budget)?;
    anchors.verify()
}

struct ObservedFile {
    path: PathBuf,
    stamp: fs::Metadata,
}

fn metadata_json<T: DeserializeOwned + Serialize>(
    path: &Path,
    maximum: u64,
    budget: &InspectionBudget,
) -> Result<(T, Vec<u8>, ObservedFile)> {
    anchor_credit(path, budget)?;
    let (mut file, before) = open_locked(path)?;
    let size = before.len();
    budget.observe_file_bytes(size)?;
    require(
        size > 0 && size <= maximum,
        "Inert evidence JSON exceeds its bound",
    )?;
    // Prepay and reserve input/canonical buffers before decoding. Maximum-size
    // additional credit covers the bounded decoded metadata allocations.
    budget.charge_work(size + maximum * 2)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size as usize).map_err(|_| {
        StoreError::Invalid("Cannot reserve bounded inert evidence JSON memory".into())
    })?;
    let mut canonical = Vec::new();
    canonical.try_reserve_exact(maximum as usize).map_err(|_| {
        StoreError::Invalid("Cannot reserve bounded inert canonical JSON memory".into())
    })?;
    bytes.resize(size as usize, 0);
    read_exact(&mut file, &mut bytes, budget)?;
    budget.charge_work(1)?;
    let mut extra = [0u8; 1];
    require(
        file.read(&mut extra)? == 0,
        "Inert evidence JSON grew while reading",
    )?;
    let value: T =
        serde_json::from_slice(&bytes).map_err(|error| StoreError::Invalid(error.to_string()))?;
    struct Canonical<'a> {
        bytes: &'a mut Vec<u8>,
        maximum: usize,
        budget: &'a InspectionBudget,
    }
    impl Write for Canonical<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.budget
                .charge_work(bytes.len() as u64)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other(
                    "Inert canonical JSON exceeds its bound",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(
        Canonical {
            bytes: &mut canonical,
            maximum: maximum as usize,
            budget,
        },
        &value,
    )
    .map_err(|error| StoreError::Invalid(error.to_string()))?;
    require(
        canonical == bytes,
        "Inert evidence JSON is not exact canonical JSON",
    )?;
    anchor_credit(path, budget)?;
    verify_archive_anchor(path, &file, &before)?;
    Ok((
        value,
        bytes,
        ObservedFile {
            path: path.to_owned(),
            stamp: before,
        },
    ))
}

/// Authenticates an existing completed inert restore without accessing the
/// original archive or opening a live NativeStore. This grants no adoption,
/// recovery, import, repair or execution authority and writes no bytes.
pub fn inspect_restored_evidence(
    directory: impl AsRef<Path>,
    expected_archive_path: &Path,
    expected_archive_sha256: &str,
    budget: &InspectionBudget,
) -> Result<EvidenceRestoreReceipt> {
    inspect_restored_evidence_using(
        directory.as_ref(),
        expected_archive_path,
        expected_archive_sha256,
        budget,
        |_, _| Ok(()),
    )
}

pub(super) fn inspect_restored_evidence_using(
    directory: &Path,
    expected_archive_path: &Path,
    expected_archive_sha256: &str,
    budget: &InspectionBudget,
    mut observe: impl FnMut(InertInspectionBoundary, &Path) -> Result<()>,
) -> Result<EvidenceRestoreReceipt> {
    let result = (|| {
        budget.checkpoint()?;
        require(
            crate::archive::digest_valid(expected_archive_sha256),
            "Inert evidence inspection needs a strict lowercase64hex archive SHA256",
        )?;
        absolute_provenance(directory)?;
        absolute_provenance(expected_archive_path)?;
        // One bounded prepayment covers request text and the expected receipt's
        // path/SHA/export-identity clones before any inspection lease or I/O.
        budget.charge_work(
            directory.as_os_str().len() as u64
                + expected_archive_path.as_os_str().len() as u64
                + expected_archive_sha256.len() as u64
                + 32
                + 320,
        )?;
        anchor_credit(directory, budget)?;
        // This lease outlives every directory/data descriptor. The metadata-only
        // checks refuse a wrong objects type before RestoreAnchors opens it.
        let _input = crate::retained_file_coordination::evidence_input(directory)?;
        let root_stamp = private_directory(directory, budget)?;
        let objects_stamp = private_directory(&directory.join("objects"), budget)?;
        top_members(directory, budget)?;
        anchor_credit(directory, budget)?;
        anchor_credit(&directory.join("objects"), budget)?;
        let anchors = RestoreAnchors::new(directory)?;
        let (receipt, _, completion_stamp): (EvidenceRestoreReceipt, _, _) = metadata_json(
            &directory.join("complete.json"),
            MAX_COMPLETION_BYTES,
            budget,
        )?;
        absolute_provenance(&receipt.directory)?;
        absolute_provenance(&receipt.archive.path)?;
        let (manifest, manifest_bytes, manifest_stamp): (EvidenceManifest, _, _) = metadata_json(
            &directory.join("manifest.json"),
            MAX_EVIDENCE_MANIFEST_BYTES as u64,
            budget,
        )?;
        validate_manifest(&manifest, budget)?;
        let mut length = 56u64 + manifest_bytes.len() as u64;
        let mut files = 0u64;
        let mut directories = 0u64;
        let mut payload = 0u64;
        for entry in &manifest.entries {
            budget.checkpoint()?;
            if entry.kind == EvidenceEntryKind::File {
                files += 1;
                length = length.checked_add(entry.byte_length).ok_or_else(|| {
                    StoreError::Invalid("Inert evidence container length overflow".into())
                })?;
                payload = payload.checked_add(entry.byte_length).ok_or_else(|| {
                    StoreError::Invalid("Inert evidence payload length overflow".into())
                })?;
            } else {
                directories += 1;
            }
        }
        budget.observe_file_bytes(length)?;
        let expected = EvidenceRestoreReceipt {
            directory: directory.to_owned(),
            archive: EvidenceArchiveReceipt {
                path: expected_archive_path.to_owned(),
                export_id: manifest.export_id.clone(),
                sha256: expected_archive_sha256.to_owned(),
                byte_length: length,
                file_count: files,
                directory_count: directories,
                payload_bytes: payload,
            },
        };
        require(
            receipt.directory.as_os_str() == directory.as_os_str()
                && receipt.archive.path.as_os_str() == expected_archive_path.as_os_str(),
            "Inert evidence completion paths differ from the exact recorded paths",
        )?;
        require(
            receipt == expected,
            "Inert evidence completion differs from the recorded identity or manifest",
        )?;
        members(&anchors, &manifest, budget)?;
        observe(InertInspectionBoundary::AfterManifestRead, directory)?;
        budget.charge_work((manifest.entries.len() as u64 + 2) * 320 + 65_536)?;
        let mut stamps = Vec::new();
        stamps
            .try_reserve_exact(manifest.entries.len() + 2)
            .map_err(|_| {
                StoreError::Invalid("Cannot reserve bounded inert evidence anchors".into())
            })?;
        stamps.push(completion_stamp);
        stamps.push(manifest_stamp);
        let mut content = Sha256::new();
        budget.charge_work(16 + manifest_bytes.len() as u64)?;
        content.update(MAGIC);
        content.update((manifest_bytes.len() as u64).to_le_bytes());
        content.update(&manifest_bytes);
        let mut catalog = Vec::new();
        let mut buffer = [0u8; 65_536];
        for (index, entry) in manifest.entries.iter().enumerate() {
            if entry.kind != EvidenceEntryKind::File {
                continue;
            }
            budget.checkpoint()?;
            anchors_credit(&anchors, budget)?;
            anchors.verify()?;
            budget.charge_work(directory.as_os_str().len() as u64 + 160)?;
            let path = directory.join("objects").join(format!("{index:04}.bin"));
            anchor_credit(&path, budget)?;
            let (mut file, before) = open_locked(&path)?;
            budget.observe_file_bytes(before.len())?;
            require(
                before.len() == entry.byte_length,
                "Inert evidence object size differs from its manifest",
            )?;
            if entry.path == "catalog.sqlite3" {
                budget.charge_work(entry.byte_length.checked_mul(2).ok_or_else(|| {
                    StoreError::Invalid("Inert catalog allocation credit overflow".into())
                })?)?;
                catalog
                    .try_reserve_exact(entry.byte_length as usize)
                    .map_err(|_| {
                        StoreError::Invalid("Cannot reserve bounded inert catalog memory".into())
                    })?;
            }
            let mut remaining = entry.byte_length;
            let mut hash = Sha256::new();
            while remaining > 0 {
                let size = remaining.min(buffer.len() as u64) as usize;
                budget.charge_work(size as u64 * 2)?;
                read_exact(&mut file, &mut buffer[..size], budget)?;
                hash.update(&buffer[..size]);
                content.update(&buffer[..size]);
                if entry.path == "catalog.sqlite3" {
                    budget.charge_work(size as u64)?;
                    catalog.extend_from_slice(&buffer[..size]);
                }
                remaining -= size as u64;
            }
            budget.charge_work(1)?;
            require(
                file.read(&mut buffer[..1])? == 0,
                "Inert evidence object grew while reading",
            )?;
            require(
                Some(format!("{:x}", hash.finalize())) == entry.sha256,
                "Inert evidence object differs from its manifest SHA256",
            )?;
            anchor_credit(&path, budget)?;
            verify_archive_anchor(&path, &file, &before)?;
            anchors_credit(&anchors, budget)?;
            anchors.verify()?;
            stamps.push(ObservedFile {
                path,
                stamp: before,
            });
            drop(file);
            observe(InertInspectionBoundary::AfterObjectRead(index), directory)?;
        }
        budget.charge_work(40)?;
        let content_digest = content.clone().finalize();
        content.update(content_digest);
        content.update(FOOTER);
        require(
            format!("{:x}", content.finalize()) == expected_archive_sha256,
            "Inert evidence reconstructed archive differs from the reviewed SHA256",
        )?;
        validate_catalog_closure(&catalog, &manifest, budget)?;
        observe(InertInspectionBoundary::BeforeFinalAnchors, directory)?;
        members(&anchors, &manifest, budget)?;
        for observed in stamps {
            anchor_credit(&observed.path, budget)?;
            same(&observed.stamp, &fs::symlink_metadata(&observed.path)?)?;
        }
        members(&anchors, &manifest, budget)?;
        directory_unchanged(directory, &root_stamp, budget)?;
        directory_unchanged(&directory.join("objects"), &objects_stamp, budget)?;
        Ok(receipt)
    })();
    budget.checkpoint()?;
    result
}

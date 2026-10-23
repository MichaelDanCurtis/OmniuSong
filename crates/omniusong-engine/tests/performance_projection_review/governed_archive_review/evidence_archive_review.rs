//! Genuine schema-3 catalog claims are inert evidence, never adoption authority.
//! The SQL and byte oracles below inspect only each test's isolated fixture.
use super::*;
use omniusong_store::{
    EVIDENCE_SCOPE, EvidenceArchiveReceipt, EvidenceEntryKind, EvidenceManifest,
    EvidenceRestoreReceipt, inspect_evidence_archive, inspect_restored_evidence,
};
use rusqlite::OpenFlags;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
enum EvidenceItem {
    Directory,
    File(Vec<u8>),
}
type EvidenceTree = BTreeMap<String, EvidenceItem>;

fn evidence_tree(root: &Path) -> EvidenceTree {
    fn visit(root: &Path, path: &Path, result: &mut EvidenceTree) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace(std::path::MAIN_SEPARATOR, "/");
            if metadata.is_dir() {
                assert!(result.insert(key, EvidenceItem::Directory).is_none());
                visit(root, &path, result);
            } else {
                assert!(metadata.is_file());
                assert!(
                    result
                        .insert(key, EvidenceItem::File(fs::read(path).unwrap()))
                        .is_none()
                );
            }
        }
    }
    let mut result = EvidenceTree::new();
    visit(root, root, &mut result);
    result
}

fn retained_evidence_tree(store: &NativeStore) -> EvidenceTree {
    let mut result = EvidenceTree::new();
    // Never open a raw FD on the live catalog as a side effect of inventory.
    for name in ["captures", "source-actions", "takes"] {
        let root = store.root().join(name);
        if root.exists() {
            let metadata = fs::symlink_metadata(&root).unwrap();
            assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
            result.insert(name.into(), EvidenceItem::Directory);
            result.extend(
                evidence_tree(&root)
                    .into_iter()
                    .map(|(path, item)| (format!("{name}/{path}"), item)),
            );
        }
    }
    result
}

// Independent storage-cell oracle for the private numbered catalog object.
// Opening this read-only SQLite copy never installs it as a NativeStore.
fn inert_catalog(path: &Path) -> Catalog {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut names = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names.push("sqlite_schema".into());
    let mut tables = BTreeMap::new();
    for name in names {
        let mut statement = connection
            .prepare(&format!(
                "SELECT * FROM \"{}\" NOT INDEXED",
                name.replace('"', "\"\"")
            ))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            values.push(
                (0..columns)
                    .map(|column| match row.get_ref(column).unwrap() {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(value) => Cell::Integer(value),
                        ValueRef::Real(value) => Cell::Real(value.to_bits()),
                        ValueRef::Text(value) => Cell::Text(value.to_vec()),
                        ValueRef::Blob(value) => Cell::Blob(value.to_vec()),
                    })
                    .collect::<Vec<_>>(),
            );
        }
        values.sort();
        tables.insert(name, values);
    }
    Catalog {
        application: connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap(),
        version: connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap(),
        schema: tables.remove("sqlite_schema").unwrap(),
        tables,
    }
}

struct ParsedEvidence {
    manifest: EvidenceManifest,
    manifest_bytes: Vec<u8>,
    payload: BTreeMap<String, Vec<u8>>,
}

// Independently frame the actual container, including every original byte and
// both the content digest and whole-file digest (the latter is the reviewed pin).
fn parse_evidence(receipt: &EvidenceArchiveReceipt) -> ParsedEvidence {
    let bytes = fs::read(&receipt.path).unwrap();
    assert_eq!(sha(&bytes), receipt.sha256);
    assert_eq!(bytes.len() as u64, receipt.byte_length);
    assert_eq!(&bytes[..8], b"OSRE\x01\r\n\0");
    let length = usize::try_from(u64::from_le_bytes(bytes[8..16].try_into().unwrap())).unwrap();
    assert!(length <= 2 * 1024 * 1024);
    let manifest_bytes = bytes[16..16 + length].to_vec();
    let manifest: EvidenceManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest_bytes, serde_json::to_vec(&manifest).unwrap());
    let mut offset = 16 + length;
    let mut payload = BTreeMap::new();
    let mut directories = 0;
    for entry in &manifest.entries {
        if entry.kind == EvidenceEntryKind::File {
            let size = usize::try_from(entry.byte_length).unwrap();
            let body = bytes[offset..offset + size].to_vec();
            assert_eq!(entry.sha256.as_deref(), Some(sha(&body).as_str()));
            assert!(payload.insert(entry.path.clone(), body).is_none());
            offset += size;
        } else {
            assert_eq!(entry.byte_length, 0);
            assert!(entry.sha256.is_none());
            directories += 1;
        }
    }
    assert_eq!(bytes.len(), offset + 40);
    assert_eq!(
        &bytes[offset..offset + 32],
        Sha256::digest(&bytes[..offset]).as_slice()
    );
    assert_eq!(&bytes[offset + 32..], b"OSREEND1");
    assert_eq!(receipt.file_count, payload.len() as u64);
    assert_eq!(receipt.directory_count, directories);
    assert_eq!(
        receipt.payload_bytes,
        payload
            .values()
            .map(|bytes| bytes.len() as u64)
            .sum::<u64>()
    );
    ParsedEvidence {
        manifest,
        manifest_bytes,
        payload,
    }
}

fn assert_inert_only(
    restored: &EvidenceRestoreReceipt,
    parsed: &ParsedEvidence,
    expected_catalog: &Catalog,
    expected_raw: &EvidenceTree,
) {
    let directory = &restored.directory;
    assert_eq!(
        fs::read(directory.join("manifest.json")).unwrap(),
        parsed.manifest_bytes
    );
    assert_eq!(
        serde_json::from_slice::<EvidenceRestoreReceipt>(
            &fs::read(directory.join("complete.json")).unwrap()
        )
        .unwrap(),
        *restored
    );
    let before = evidence_tree(directory);
    let mut expected = BTreeSet::from([
        "manifest.json".to_owned(),
        "complete.json".to_owned(),
        "objects".to_owned(),
    ]);
    let mut raw = EvidenceTree::new();
    for (index, entry) in parsed.manifest.entries.iter().enumerate() {
        if entry.kind == EvidenceEntryKind::File {
            let name = format!("objects/{index:04}.bin");
            let path = directory.join(&name);
            assert_eq!(fs::read(&path).unwrap(), parsed.payload[&entry.path]);
            expected.insert(name);
            if entry.path == "catalog.sqlite3" {
                assert_eq!(
                    inert_catalog(&path),
                    *expected_catalog,
                    "all current/history/operation/proof cells, SQL types and schema are preserved"
                );
            } else {
                raw.insert(
                    entry.path.clone(),
                    EvidenceItem::File(parsed.payload[&entry.path].clone()),
                );
            }
        } else {
            raw.insert(entry.path.clone(), EvidenceItem::Directory);
        }
    }
    assert_eq!(raw, *expected_raw);
    assert_eq!(before.keys().cloned().collect::<BTreeSet<_>>(), expected);
    assert_eq!(
        evidence_tree(directory),
        before,
        "the read-only catalog oracle creates no sidecars"
    );
    for active in ["studio.sqlite3", "captures", "source-actions", "takes"] {
        assert!(
            !directory.join(active).exists(),
            "no active native namespace is installed"
        );
    }
}

fn evidence_roundtrip(store: &mut NativeStore, output: &Path, destination: &Path, export_id: &str) {
    let before = catalog(store);
    let raw = retained_evidence_tree(store);
    let owned = originals(store);
    let (governor, _) = budget(InspectionLimits::default());
    let receipt = store
        .export_retained_evidence_full_root_typed(output, export_id, &governor)
        .unwrap();
    let parsed = parse_evidence(&receipt);
    assert_eq!(parsed.manifest.version, 1);
    assert_eq!(parsed.manifest.scope, EVIDENCE_SCOPE);
    assert_eq!(parsed.manifest.catalog_schema, 6);
    assert_eq!(parsed.manifest.origin_root, store.root().to_str().unwrap());
    assert_eq!(parsed.manifest.export_id, export_id);
    let paths = parsed
        .manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
    for original in owned.values() {
        assert_eq!(
            parsed.payload[&format!("takes/{}.sqlite3", original.object_sha)],
            original.bytes
        );
    }
    let (governor, _) = budget(InspectionLimits::default());
    let inspection = inspect_evidence_archive(&receipt.path, &governor).unwrap();
    assert_eq!(inspection.receipt, receipt);
    assert_eq!(inspection.manifest, parsed.manifest);
    let (governor, _) = budget(InspectionLimits::default());
    let restored = store
        .restore_retained_evidence_reviewed(&receipt.path, destination, &receipt.sha256, &governor)
        .unwrap();
    assert_eq!(restored.archive, receipt);
    assert_inert_only(&restored, &parsed, &before, &raw);
    let inert_before = evidence_tree(&restored.directory);
    let parked = receipt.path.with_extension("parked-osre");
    fs::rename(&receipt.path, &parked).unwrap();
    assert!(!receipt.path.exists());
    let (governor, _) = budget(InspectionLimits::default());
    assert_eq!(
        inspect_restored_evidence(
            &restored.directory,
            &receipt.path,
            &receipt.sha256,
            &governor,
        )
        .unwrap(),
        restored
    );
    assert_eq!(evidence_tree(&restored.directory), inert_before);
    assert_eq!(sha(&fs::read(&parked).unwrap()), receipt.sha256);
    assert_eq!(catalog(store), before);
    assert_eq!(retained_evidence_tree(store), raw);
    assert_eq!(originals(store), owned);
}

#[test]
fn genuine_v1_v2_schema3_publications_and_two_copy_generations_restore_only_numbered_evidence() {
    for v2 in [false, true] {
        let mut f = ArchiveFixture::new(v2);
        f.add_ordinary_audio();
        let source_catalog = catalog(&f.store);
        let source_originals = originals(&f.store);
        let staged = f.original.bytes();
        assert_eq!(
            source_originals.len(),
            2,
            "ordinary audio and performance are separate roles"
        );
        assert_eq!(f.project.schema_version(), 3);
        evidence_roundtrip(
            &mut f.store,
            &f.root.join("published.osre"),
            &f.root.join("published-inert"),
            "11111111111111111111111111111111",
        );
        let (archive, receipt) = f.export("copy-source.osar");
        let mut first = NativeStore::open(f.root.join("first-copy")).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        let copied = first
            .restore_archive_as_copy_bounded_outcome(
                &archive,
                &copy_intent(&receipt, "evidence-first-copy"),
                &governor,
            )
            .unwrap()
            .value
            .document;
        let (next_archive, next_receipt) =
            export(&first, copied.id(), &f.root.join("copy-one.osar"));
        let mut leaf = NativeStore::open(f.root.join("second-copy")).unwrap();
        let (governor, _) = budget(InspectionLimits::default());
        let copied = leaf
            .restore_archive_as_copy_bounded_outcome(
                &next_archive,
                &copy_intent(&next_receipt, "evidence-second-copy"),
                &governor,
            )
            .unwrap()
            .value
            .document;
        assert_copy_document(&f.project, &copied);
        assert_live_mappings(&leaf, copied.id(), &f.intent);
        assert_eq!(originals(&leaf), source_originals);
        let leaf_catalog = catalog(&leaf);
        assert_eq!(leaf_catalog.tables["archive_copy_proofs"].len(), 2);
        assert!(!leaf_catalog.tables["archive_copy_proof_operations"].is_empty());
        // Full-root evidence also contains the unrelated ordinary project and
        // retained bytes that no active clip or binding claims.
        leaf.create(&ProjectDocument::new("Unrelated ordinary project").unwrap())
            .unwrap();
        fs::create_dir_all(leaf.root().join("takes/empty-retained-directory")).unwrap();
        fs::write(
            leaf.root().join("takes/future-opaque.bin"),
            b"unclaimed bytes\0\xff",
        )
        .unwrap();
        evidence_roundtrip(
            &mut leaf,
            &f.root.join("copied.osre"),
            &f.root.join("copied-inert"),
            "22222222222222222222222222222222",
        );
        assert_eq!(catalog(&f.store), source_catalog);
        assert_eq!(originals(&f.store), source_originals);
        assert_eq!(f.original.bytes(), staged);
    }
}

#[test]
fn schema3_reviewed_restore_refuses_another_valid_whole_container_before_completion() {
    let mut f = ArchiveFixture::new(true);
    let (governor, _) = budget(InspectionLimits::default());
    let reviewed = f
        .store
        .export_retained_evidence_full_root(
            f.root.join("reviewed.osre"),
            "33333333333333333333333333333333",
            &governor,
        )
        .unwrap();
    let inspected_bytes = fs::read(&reviewed.path).unwrap();
    f.project = f
        .store
        .apply_edit(&edit(&f.project, "later-name", "Later human name"))
        .unwrap()
        .document;
    let (governor, _) = budget(InspectionLimits::default());
    let replacement = f
        .store
        .export_retained_evidence_full_root(
            f.root.join("replacement.osre"),
            "44444444444444444444444444444444",
            &governor,
        )
        .unwrap();
    assert_ne!(reviewed.sha256, replacement.sha256);
    let (governor, _) = budget(InspectionLimits::default());
    inspect_evidence_archive(&replacement.path, &governor).unwrap();
    fs::copy(&replacement.path, &reviewed.path).unwrap();
    let before = catalog(&f.store);
    let raw = retained_evidence_tree(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let refused = f.root.join("mismatched-inert");
    let (governor, _) = budget(InspectionLimits::default());
    assert!(matches!(f.store.restore_retained_evidence_reviewed(
        &reviewed.path, &refused, &reviewed.sha256, &governor,
    ), Err(StoreError::Invalid(message)) if message ==
        "Evidence archive differs from the reviewed SHA256; inert partial evidence was retained"));
    assert!(refused.join("manifest.json").exists());
    assert!(refused.join("objects").is_dir());
    assert!(!refused.join("complete.json").exists());
    for active in ["studio.sqlite3", "takes", "captures", "source-actions"] {
        assert!(!refused.join(active).exists());
    }
    assert_eq!(sha(&inspected_bytes), reviewed.sha256);
    assert_eq!(sha(&fs::read(&reviewed.path).unwrap()), replacement.sha256);
    assert_eq!(catalog(&f.store), before);
    assert_eq!(retained_evidence_tree(&f.store), raw);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn coherent_false_operation_only_music_is_preserved_as_inert_evidence_but_never_authenticated() {
    let mut f = ArchiveFixture::new(true);
    let published = branch_after_undo(&mut f);
    let forged = false_pitch(&published);
    let registry = forged.performance_projections().unwrap().unwrap();
    let binding = &registry.entries[0];
    let (governor, _) = budget(InspectionLimits::default());
    check_retained_binding_encoding_bounded(binding, &governor).unwrap();
    let (governor, _) = budget(InspectionLimits::default());
    let reader = f
        .store
        .open_take_with_budget(&binding.source, &governor)
        .unwrap();
    let held = AuthenticatedPerformance::open(&reader).unwrap();
    assert!(
        matches!(held.validate_binding(binding), Err(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation")
    );
    drop(held);
    drop(reader);
    let mut intent = f.intent.clone();
    intent.expected_review_sha256 = binding.review.review_sha256.clone();
    let request = intent.canonical_json().unwrap();
    let result = String::from_utf8(forged.to_json().unwrap()).unwrap();
    let connection = db(&f.store);
    remove_triggers(&connection, "operations");
    assert_eq!(connection.execute(
        "UPDATE operations SET request_json=?1,request_sha256=?2,result_json=?3,result_sha256=?4 WHERE operation_id=?5",
        params![request, sha(request.as_bytes()), result, sha(result.as_bytes()), intent.operation_id.0],
    ).unwrap(), 1);
    drop(connection);
    let before = catalog(&f.store);
    let raw = retained_evidence_tree(&f.store);
    let owned = originals(&f.store);
    let staged = f.original.bytes();
    let (governor, _) = budget(InspectionLimits::default());
    let failure = f
        .store
        .inspect_retained_projection_closure(f.project.id(), &governor)
        .unwrap_err();
    assert!(
        matches!(primary(&failure), StoreError::Recording(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation")
    );
    evidence_roundtrip(
        &mut f.store,
        &f.root.join("false-claim-evidence.osre"),
        &f.root.join("false-claim-inert"),
        "55555555555555555555555555555555",
    );
    // Forensic preservation cannot turn the same false claim into an active
    // source-derived project or a semantically admitted projection archive.
    let (governor, _) = budget(InspectionLimits::default());
    let failure = f
        .store
        .export_archive_bounded_outcome(
            f.project.id(),
            &f.root.join("false-claim-active.osar"),
            &governor,
        )
        .unwrap_err();
    assert!(
        matches!(primary(failure.operation_error()), StoreError::Recording(RecordingError::Invalid(message))
        if message == "Retained performance projection differs from the held original derivation")
    );
    assert_eq!(catalog(&f.store), before);
    assert_eq!(retained_evidence_tree(&f.store), raw);
    assert_eq!(originals(&f.store), owned);
    assert_eq!(f.original.bytes(), staged);
}

#[test]
fn schema3_evidence_still_requires_every_registered_original_without_adopting_a_substitute() {
    let mut f = ArchiveFixture::new(false);
    let owned = originals(&f.store);
    let original = owned.values().next().unwrap();
    let missing_relative = format!("takes/{}.sqlite3", original.object_sha);
    let original_path = f.store.root().join(&missing_relative);
    let parked = f.root.join("private-parked-original.bin");
    fs::rename(&original_path, &parked).unwrap();
    let before = catalog(&f.store);
    let raw = retained_evidence_tree(&f.store);
    let staged = f.original.bytes();
    let output = f.root.join("missing-original.osre");
    let (governor, _) = budget(InspectionLimits::default());
    assert!(matches!(f.store.export_retained_evidence_full_root(
        &output, "66666666666666666666666666666666", &governor,
    ), Err(StoreError::Invalid(message)) if message ==
        format!("Complete evidence export is missing required bytes: {missing_relative}")));
    assert!(!output.exists());
    assert!(!original_path.exists());
    assert_eq!(fs::read(&parked).unwrap(), original.bytes);
    assert_eq!(catalog(&f.store), before);
    assert_eq!(retained_evidence_tree(&f.store), raw);
    assert_eq!(f.original.bytes(), staged);
}

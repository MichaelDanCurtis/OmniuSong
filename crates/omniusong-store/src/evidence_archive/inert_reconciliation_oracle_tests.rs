//! An independently encoded, coherently rehashed extraction must still prove
//! the actual archived catalog's required-reference closure.
use super::*;

#[test]
fn reconstructed_sha_and_forged_completion_cannot_hide_a_missing_required_source() {
    let mut fixture = Fixture::new();
    let receipt = fixture.export("closure-oracle-source.osre");
    let mut parsed = parse_independent(&receipt.path);
    let omitted = format!("captures/{STAGE}/take.sqlite3");
    assert!(parsed.payload.remove(&omitted).is_some());
    parsed
        .manifest
        .entries
        .retain(|entry| entry.path != omitted);
    let rebuilt = bundle(&parsed.manifest, &parsed.payload, None);
    assert_authenticated_container(&rebuilt);
    let expected_sha = sha(&rebuilt);
    let directory = fixture.root.join("independently-forged-inert");
    let mut builder = fs::DirBuilder::new();
    use std::os::unix::fs::DirBuilderExt;
    builder.mode(0o700);
    builder.create(&directory).unwrap();
    builder.create(directory.join("objects")).unwrap();
    let manifest_bytes = serde_json::to_vec(&parsed.manifest).unwrap();
    fs::write(directory.join("manifest.json"), &manifest_bytes).unwrap();
    let mut file_count = 0;
    let mut directory_count = 0;
    let mut payload_bytes = 0;
    for (index, entry) in parsed.manifest.entries.iter().enumerate() {
        if entry.kind == EvidenceEntryKind::Directory {
            directory_count += 1;
        } else {
            file_count += 1;
            let bytes = &parsed.payload[&entry.path];
            assert_eq!(bytes.len() as u64, entry.byte_length);
            payload_bytes += bytes.len() as u64;
            fs::write(
                directory.join("objects").join(format!("{index:04}.bin")),
                bytes,
            )
            .unwrap();
        }
    }
    let absent_archive = fixture.archive("never-created-claimed-original.osre");
    assert!(!absent_archive.exists());
    // All JSON, counts, per-object hashes and the complete reconstructed SHA
    // agree. Only the catalog's actual required-link proof can reject it.
    let marker = serde_json::json!({
        "directory": directory,
        "archive": {
            "path": absent_archive,
            "export_id": EXPORT,
            "sha256": expected_sha,
            "byte_length": rebuilt.len(),
            "file_count": file_count,
            "directory_count": directory_count,
            "payload_bytes": payload_bytes
        }
    });
    // Serialize through the public receipt's declared canonical field order,
    // while every value above was derived independently from raw payloads.
    let typed: EvidenceRestoreReceipt = serde_json::from_value(marker).unwrap();
    fs::write(
        directory.join("complete.json"),
        serde_json::to_vec(&typed).unwrap(),
    )
    .unwrap();
    let before = tree(&fixture.root);
    let catalog_before = catalog(&fixture.store.connection);
    let error = inspect_restored_evidence(&directory, &absent_archive, &expected_sha, &budget())
        .unwrap_err();
    assert!(
        matches!(error, StoreError::Invalid(ref message) if message.contains("missing required bytes")),
        "{error:?}"
    );
    assert_eq!(tree(&fixture.root), before);
    assert_eq!(catalog(&fixture.store.connection), catalog_before);
    assert!(!fixture.store.list().unwrap().is_empty());
    assert!(!absent_archive.exists());
}

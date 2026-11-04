use super::*;
use omniusong_core::{DocumentError, ProjectDocument, contracts::{TakeId, TrackId}, native::{TakeClip, TakeRange, TakeReference, TimelinePosition}};

fn request() -> CaptureAttachmentRequest {
    let root = std::env::temp_dir().join("omniusong-capture-dto-only");
    CaptureAttachmentRequest {
        version: 1, origin_root: root.join("store"), journal_path: root.join("old-stage.sqlite3"),
        publication: TakePublication {
            operation_id: OperationId("capture-operation".into()), project_id: ProjectId("capture-project".into()), expected_revision: 7,
            track_id: TrackId("capture-track".into()), track_name: "Synthetic original".into(),
            clip: TakeClip {
                id: "capture-clip".into(), edit_source: None,
                source: TakeReference { take_id: TakeId("capture-take".into()), manifest_sha256: "a".repeat(64), stream_id: "audio".into() },
                range: TakeRange::AudioFrames { start: 0, count: 4 }, position: TimelinePosition { frame: 0, sample_rate: 48_000 }, fades: None,
            },
        },
    }
}
fn committed(request: &CaptureAttachmentRequest) -> CaptureAttachmentReply {
    *PreparedReply::new(request, request.fingerprint().unwrap()).unwrap().reply
}
fn sqlite(code: i32) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), Some("Synthetic SQLite diagnostic".into()))
}

#[test]
fn exact_request_covers_origin_locator_and_all_publication_bytes_without_filesystem_access() {
    let original = request();
    original.validate().unwrap();
    let bytes = original.canonical_bytes().unwrap();
    assert_eq!(CaptureAttachmentRequest::from_json(&bytes).unwrap(), original);
    let fingerprint = original.fingerprint().unwrap();
    for changed in [
        { let mut value = original.clone(); value.origin_root.push("another"); value },
        { let mut value = original.clone(); value.journal_path.set_file_name("missing-old-stage.sqlite3"); value },
        { let mut value = original.clone(); value.publication.track_name.push('!'); value },
        { let mut value = original.clone(); value.publication.expected_revision += 1; value },
    ] {
        assert_ne!(changed.fingerprint().unwrap(), fingerprint);
    }
    assert!(!original.journal_path.exists(), "DTO validation unexpectedly created a journal");
}

#[test]
fn request_and_reply_reject_unknown_fields_and_nonstring_enum_encodings() {
    let request = request();
    let mut value = serde_json::to_value(&request).unwrap();
    value["unknown"] = true.into();
    assert!(CaptureAttachmentRequest::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(committed(&request)).unwrap();
    value["outcome"]["commitment"]["completion"] = serde_json::json!({"fresh_commit":null});
    assert!(CaptureAttachmentReply::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(committed(&request)).unwrap();
    value["outcome"]["unknown"] = true.into();
    assert!(CaptureAttachmentReply::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn lexical_paths_identity_and_checked_revisions_fail_closed() {
    let original = request();
    for path in [PathBuf::from("relative.sqlite3"), original.origin_root.join("../old.sqlite3"), original.origin_root.join("bad\n.sqlite3")] {
        let mut value = original.clone(); value.journal_path = path; assert!(value.validate().is_err());
    }
    for suffix in ["/./stage.sqlite3", "//stage.sqlite3", "/stage.sqlite3/"] {
        let mut value = original.clone();
        value.journal_path = PathBuf::from(format!("{}{suffix}", original.origin_root.display()));
        assert!(value.validate().is_err());
    }
    let mut value = original.clone(); value.version = 2; assert!(value.validate().is_err());
    let mut value = original.clone(); value.publication.operation_id.0 = "a".repeat(97); assert!(value.validate().is_err());
    let mut value = original.clone(); value.publication.project_id.0 = "a".repeat(97); assert!(value.validate().is_err());
    let mut value = original.clone(); value.publication.expected_revision = i64::MAX as u64; assert!(value.validate().is_err());
    let mut value = original; value.publication.expected_revision = u64::MAX; assert!(value.validate().is_err());
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_are_not_admitted() {
    use std::os::unix::ffi::OsStringExt;
    let mut value = request();
    value.journal_path = PathBuf::from(std::ffi::OsString::from_vec(b"/synthetic/\xff".to_vec()));
    assert!(value.validate().is_err());
}

#[test]
fn exact_byte_ceilings_include_the_entire_canonical_request_and_wire_reply() {
    let mut value = request();
    let prefix = std::env::temp_dir();
    let base = prefix.to_str().unwrap().len() + 1;
    value.journal_path = prefix.join("a".repeat(MAX_CAPTURE_ATTACHMENT_PATH_BYTES - base));
    assert_eq!(value.journal_path.to_str().unwrap().len(), MAX_CAPTURE_ATTACHMENT_PATH_BYTES);
    value.validate().unwrap();
    value.journal_path.as_mut_os_string().push("a");
    assert!(value.validate().is_err());
    let request = request();
    let mut bytes = request.canonical_bytes().unwrap();
    bytes.resize(MAX_CAPTURE_ATTACHMENT_REQUEST_BYTES, b' ');
    assert!(CaptureAttachmentRequest::from_json(&bytes).is_ok());
    bytes.push(b' ');
    assert!(CaptureAttachmentRequest::from_json(&bytes).is_err());
    let mut bytes = serde_json::to_vec(&committed(&request)).unwrap();
    bytes.resize(MAX_CAPTURE_ATTACHMENT_REPLY_BYTES, b' ');
    assert!(CaptureAttachmentReply::from_json(&bytes).is_ok());
    bytes.push(b' ');
    assert!(CaptureAttachmentReply::from_json(&bytes).is_err());
}

#[test]
fn matching_unknown_version_is_consumed_as_invalid_body_not_foreign_reference() {
    let request = request();
    let mut reply = committed(&request);
    reply.store_invoked = true;
    reply.version = 2;
    assert!(reply.matches_request_reference(&request));
    assert!(reply.validate_for(&request).is_err());
    reply.request_sha256 = "b".repeat(64);
    assert!(!reply.matches_request_reference(&request));
}

#[test]
fn invalid_matching_commitment_cannot_hide_an_independent_live_catalog_retirement() {
    let request = request();
    let mut reply = committed(&request);
    reply.store_invoked = true;
    let CaptureAttachmentOutcome::Committed { commitment } = &mut reply.outcome else { unreachable!() };
    commitment.committed_revision = u64::MAX;
    reply.catalog_retirement = Some(CaptureAttachmentCatalogRetirement {
        sqlite_code: Some(11), connection_unusable: false, detail: bounded_message(&"Synthetic fatal catalog"),
    });
    assert!(reply.matches_request_reference(&request));
    assert!(reply.validate_for(&request).is_err());
    let error = reply.retirement_error().unwrap();
    assert_eq!(error.kind, FailureKind::StorageUnavailable);
    assert!(error.outcome_unknown);
}

#[test]
fn success_metadata_and_disposition_are_checked_without_serializing_document_content() {
    let mut request = request();
    let document = ProjectDocument::new("Synthetic receipt").unwrap();
    request.publication.project_id = ProjectId(document.id().into());
    request.publication.expected_revision = 0;
    let reply = PreparedReply::new(&request, request.fingerprint().unwrap()).unwrap().success(EditReceipt {
        operation_id: request.publication.operation_id.0.clone(), previous_revision: 0, document, replayed: false,
    });
    assert!(matches!(reply.outcome, CaptureAttachmentOutcome::Uncertain { .. }), "Revision-zero document must not be presented as an adjacent committed result");
    assert_eq!(reply.failure_error().unwrap().kind, FailureKind::Protocol);
    assert!(reply.failure_error().unwrap().outcome_unknown);
    reply.validate_for(&request).unwrap();
}

#[test]
fn only_exact_conflict_and_edit_rejected_errors_prove_no_project_commit() {
    let request = request();
    for error in [StoreError::Conflict, StoreError::EditRejected(DocumentError("Synthetic semantic refusal".into()))] {
        let reply = PreparedReply::new(&request, request.fingerprint().unwrap()).unwrap().failure(error);
        assert!(reply.store_invoked);
        assert!(matches!(reply.outcome, CaptureAttachmentOutcome::Rejected { .. }));
        assert!(!reply.failure_error().unwrap().outcome_unknown);
        reply.validate_for(&request).unwrap();
    }
    for error in [
        StoreError::Invalid("Synthetic integrity refusal".into()),
        StoreError::Io(std::io::Error::other("Synthetic I/O failure")),
        StoreError::Archive(Box::new(omniusong_store::ArchiveFailure::new(
            omniusong_store::ArchiveContext::new(omniusong_store::ArchiveAction::Inspect, &request.origin_root), StoreError::Conflict,
        ))),
    ] {
        let reply = PreparedReply::new(&request, request.fingerprint().unwrap()).unwrap().failure(error);
        assert!(matches!(reply.outcome, CaptureAttachmentOutcome::Uncertain { .. }));
        assert!(reply.failure_error().unwrap().outcome_unknown);
        reply.validate_for(&request).unwrap();
    }
}

#[test]
fn source_corrupt_is_distinct_from_actual_live_catalog_corrupt_and_stops_remain_typed() {
    let request = request();
    let source = PreparedReply::new(&request, request.fingerprint().unwrap()).unwrap().failure(StoreError::Recording(RecordingError::Database(sqlite(11))));
    assert!(source.catalog_retirement.is_none());
    assert!(source.retirement_error().is_none());
    let CaptureAttachmentOutcome::Uncertain { problem } = &source.outcome else { panic!("source failure must remain uncertain") };
    assert_eq!(problem.sqlite.as_ref().unwrap().origin, CaptureAttachmentSqlOrigin::OwnedSource);
    source.validate_for(&request).unwrap();
    let catalog = PreparedReply::new(&request, request.fingerprint().unwrap()).unwrap().failure(StoreError::Database(sqlite(11)));
    assert!(catalog.retirement_error().is_some());
    catalog.validate_for(&request).unwrap();
    for error in [RecordingError::InspectionCancelled, RecordingError::InspectionLimit { resource: InspectionResource::WorkBytes, limit: 0, attempted: 1 }] {
        let reply = PreparedReply::new(&request, request.fingerprint().unwrap()).unwrap().failure(StoreError::Recording(error));
        let CaptureAttachmentOutcome::Uncertain { problem } = &reply.outcome else { panic!("invoked stop must remain uncertain") };
        assert!(problem.governor_stop.is_some());
        assert!(reply.catalog_retirement.is_none());
        reply.validate_for(&request).unwrap();
    }
}

#[test]
fn diagnostics_are_utf8_bounded_and_worst_case_wire_shape_fits_reserved_storage() {
    let request = request();
    let reply = PreparedReply::new(&request, request.fingerprint().unwrap()).unwrap().failure(StoreError::Invalid("東京\0".repeat(1024)));
    let CaptureAttachmentOutcome::Uncertain { problem } = &reply.outcome else { panic!("invalid store result must remain uncertain") };
    assert!(problem.detail.truncated);
    assert!(problem.detail.text.len() <= MAX_CAPTURE_ATTACHMENT_MESSAGE_BYTES);
    assert!(serde_json::to_vec(reply.as_ref()).unwrap().len() < MAX_CAPTURE_ATTACHMENT_REPLY_BYTES);
    reply.validate_for(&request).unwrap();
}

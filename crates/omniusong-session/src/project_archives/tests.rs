//! Pure DTO/conversion controls. Paths are inert text; no files or SQLite are
//! opened. Constructed SQLite codes establish transport domains, not I/O faults.
use super::*;
use crate::FailureKind;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn request(operation: ProjectArchiveOperation) -> ProjectArchiveRequest {
    ProjectArchiveRequest {
        version: 1,
        origin_root: "/archive-dto/catalog".into(),
        operation,
    }
}
fn export_request() -> ProjectArchiveRequest {
    request(ProjectArchiveOperation::Export {
        project_id: ProjectId("project-one".into()),
        destination: "/archive-dto/output/song.osar".into(),
    })
}
fn identity_request() -> ProjectArchiveRequest {
    request(ProjectArchiveOperation::RestoreIdentity {
        archive_path: "/archive-dto/input.osar".into(),
    })
}
fn copy_request() -> ProjectArchiveRequest {
    request(ProjectArchiveOperation::RestoreCopy {
        archive_path: "/archive-dto/input.osar".into(),
        intent: RestoreCopyIntent {
            operation_id: OperationId("copy-one".into()),
            expected_archive_sha256: "a".repeat(64),
        },
    })
}
fn receipt(request: &ProjectArchiveRequest) -> ArchiveReceipt {
    let ProjectArchiveOperation::Export {
        project_id,
        destination,
    } = &request.operation
    else {
        unreachable!()
    };
    ArchiveReceipt {
        path: destination.clone(),
        project_id: project_id.0.clone(),
        sha256: "b".repeat(64),
        byte_length: 8192,
        history_rows: 7,
        operation_rows: 11,
        asset_count: 2,
        take_count: 3,
    }
}
fn stage(request: &ProjectArchiveRequest) -> PathBuf {
    let (base, prefix) = stage_base(request);
    base.join(format!(
        "{prefix}01234567-89ab-4cde-8123-456789abcdef.partial"
    ))
}
fn successful_export(request: &ProjectArchiveRequest) -> ProjectArchiveReply {
    from_export(
        &request.fingerprint().unwrap(),
        ArchiveOutcome::new(receipt(request), ArchiveCompletion::ExportDurable),
    )
}
fn sqlite(code: i32) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(code),
        Some("explicit DTO error-code control".into()),
    )
}
fn tagged(origin: ArchiveSqlOrigin, code: i32) -> StoreError {
    StoreError::ArchiveSql(Box::new(omniusong_store::ArchiveSqlFailure::new(
        origin,
        sqlite(code),
    )))
}

#[test]
fn canonical_request_bytes_and_fingerprints_bind_every_operation_and_origin() {
    for request in [export_request(), identity_request(), copy_request()] {
        let bytes = request.canonical_bytes().unwrap();
        assert_eq!(ProjectArchiveRequest::from_json(&bytes).unwrap(), request);
        let mut independent = Sha256::new();
        independent.update(b"OmniuSong.ProjectArchiveRequest.v1\0");
        independent.update(&bytes);
        assert_eq!(
            request.fingerprint().unwrap(),
            format!("{:x}", independent.finalize())
        );
        let mut changed = request.clone();
        changed.origin_root = "/archive-dto/another-catalog".into();
        assert_ne!(
            request.fingerprint().unwrap(),
            changed.fingerprint().unwrap()
        );
        let mut changed = request.clone();
        match &mut changed.operation {
            ProjectArchiveOperation::Export {
                project_id,
                destination,
            } => {
                project_id.0.push_str("-changed");
                destination.push("changed");
            }
            ProjectArchiveOperation::RestoreIdentity { archive_path } => {
                archive_path.push("changed")
            }
            ProjectArchiveOperation::RestoreCopy {
                archive_path,
                intent,
            } => {
                archive_path.push("changed");
                intent.operation_id.0.push_str("-changed");
                intent.expected_archive_sha256 = "c".repeat(64);
            }
        }
        assert_ne!(
            request.fingerprint().unwrap(),
            changed.fingerprint().unwrap()
        );
    }
}

#[test]
fn strict_request_and_reply_json_refuse_unknown_fields_and_map_shaped_units() {
    let request = copy_request();
    for pointer in ["", "/operation", "/operation/intent"] {
        let mut value = serde_json::to_value(&request).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), Value::Bool(true));
        assert!(serde_json::from_value::<ProjectArchiveRequest>(value).is_err());
    }
    let request = export_request();
    let reply = successful_export(&request);
    for pointer in ["", "/outcome", "/outcome/receipt", "/outcome/cleanup"] {
        let mut value = serde_json::to_value(&reply).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), Value::Bool(true));
        assert!(serde_json::from_value::<ProjectArchiveReply>(value).is_err());
    }
    assert!(serde_json::from_value::<ProjectArchiveAction>(json!({"export":null})).is_err());
    assert!(
        serde_json::from_value::<ProjectArchivePhase>(json!({"not_started":{"unknown":true}}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<ProjectArchiveCopyCompletion>(json!({"recorded_replay":null}))
            .is_err()
    );
    assert!(serde_json::from_value::<ProjectArchiveSqlOrigin>(json!({"input":null})).is_err());
    assert!(serde_json::from_value::<ProjectArchiveProblemKind>(json!({"rejected":null})).is_err());
    assert!(
        serde_json::from_value::<ProjectArchiveBudgetStop>(
            json!({"kind":"cancelled","unknown":true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ProjectArchiveBudgetStop>(
            json!({"kind":"limit","resource":{"WorkBytes":null},"limit":1,"attempted":2})
        )
        .is_err()
    );
    let stop = ProjectArchiveBudgetStop::Limit {
        resource: InspectionResource::WorkBytes,
        limit: 1,
        attempted: 2,
    };
    assert_eq!(
        serde_json::from_slice::<ProjectArchiveBudgetStop>(&serde_json::to_vec(&stop).unwrap())
            .unwrap(),
        stop
    );
}

#[test]
fn request_paths_identifiers_versions_and_raw_envelopes_are_bounded() {
    for path in [
        "relative",
        "/archive-dto/../catalog",
        "/archive-dto/./catalog",
        "/archive-dto//catalog",
        "/archive-dto/catalog/",
        "/archive-dto/\ncontrol",
    ] {
        let mut request = export_request();
        request.origin_root = path.into();
        assert!(
            request.validate().is_err(),
            "Accepted alias/control path {path:?}"
        );
    }
    let mut request = export_request();
    request.origin_root = format!("/{}", "x".repeat(MAX_PROJECT_ARCHIVE_PATH_BYTES)).into();
    assert!(request.validate().is_err());
    let mut request = copy_request();
    let ProjectArchiveOperation::RestoreCopy { intent, .. } = &mut request.operation else {
        unreachable!()
    };
    intent.expected_archive_sha256 = "A".repeat(64);
    assert!(request.validate().is_err());
    let mut request = export_request();
    request.version = 2;
    assert!(request.validate().is_err());
    let mut request = export_request();
    let ProjectArchiveOperation::Export { project_id, .. } = &mut request.operation else {
        unreachable!()
    };
    project_id.0 = "x".repeat(97);
    assert!(request.validate().is_err());
    assert!(
        ProjectArchiveRequest::from_json(&vec![b' '; MAX_PROJECT_ARCHIVE_REQUEST_BYTES + 1])
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_are_refused_without_lossy_replacement() {
    use std::os::unix::ffi::OsStringExt;
    let mut request = export_request();
    request.origin_root = std::ffi::OsString::from_vec(b"/archive-dto/\xff".to_vec()).into();
    assert!(request.validate().is_err());
    let request = export_request();
    let mut reply = successful_export(&request);
    let ProjectArchiveOutcome::Exported { receipt, .. } = &mut reply.outcome else {
        unreachable!()
    };
    receipt.path = std::ffi::OsString::from_vec(b"/archive-dto/\xff".to_vec()).into();
    assert!(reply.matches_request_reference(&request));
    assert!(reply.validate_for(&request).is_err());
}

#[test]
fn successful_export_keeps_exact_receipt_and_bounded_cleanup_warnings() {
    let request = export_request();
    let expected = receipt(&request);
    let mut outcome = ArchiveOutcome::new(expected.clone(), ArchiveCompletion::ExportDurable);
    for index in 0..6 {
        outcome.cleanup.push(ArchiveCleanupIssue {
            path: stage(&request).join(format!("private-{index}")),
            error: std::io::Error::other("π".repeat(2048)),
        });
    }
    let reply = from_export(&request.fingerprint().unwrap(), outcome);
    reply.validate_for(&request).unwrap();
    let ProjectArchiveOutcome::Exported { receipt, cleanup } = &reply.outcome else {
        panic!("Cleanup warning erased successful export");
    };
    assert_eq!(
        (
            receipt.project_id.0.as_str(),
            receipt.path.as_path(),
            receipt.sha256.as_str(),
            receipt.byte_length,
            receipt.history_rows,
            receipt.operation_rows,
            receipt.asset_count,
            receipt.take_count
        ),
        (
            expected.project_id.as_str(),
            expected.path.as_path(),
            expected.sha256.as_str(),
            8192,
            7,
            11,
            2,
            3
        )
    );
    assert_eq!((cleanup.issues.len(), cleanup.omitted), (4, 2));
    assert!(cleanup.issues.iter().all(|issue| issue.detail.text.len()
        == MAX_PROJECT_ARCHIVE_MESSAGE_BYTES
        && issue.detail.truncated));
    assert!(
        encode(&reply, MAX_PROJECT_ARCHIVE_REPLY_BYTES)
            .unwrap()
            .len()
            < MAX_PROJECT_ARCHIVE_REPLY_BYTES
    );
}

#[test]
fn identity_and_copy_return_only_compact_project_references_and_exact_completion() {
    let mut value = ProjectDocument::new("Compact original")
        .unwrap()
        .value()
        .clone();
    value["id"] = "destination-project".into();
    value["revision"] = 7.into();
    value["vendor:large"] = Value::String("opaque".repeat(50_000));
    let document = ProjectDocument::from_value(value).unwrap();
    let request = identity_request();
    let reply = from_identity(
        &request.fingerprint().unwrap(),
        ArchiveOutcome::new(document.clone(), ArchiveCompletion::CatalogCommitted),
    );
    reply.validate_for(&request).unwrap();
    let encoded =
        String::from_utf8(encode(&reply, MAX_PROJECT_ARCHIVE_REPLY_BYTES).unwrap()).unwrap();
    assert!(
        encoded.len() < 1024
            && !encoded.contains("vendor:large")
            && !encoded.contains("tracks")
            && !encoded.contains("result_sha256")
    );
    let request = copy_request();
    let ProjectArchiveOperation::RestoreCopy { intent, .. } = &request.operation else {
        unreachable!()
    };
    for replayed in [false, true] {
        let completion = if replayed {
            ArchiveCompletion::RecordedReplay
        } else {
            ArchiveCompletion::CatalogCommitted
        };
        let reply = from_copy(
            &request.fingerprint().unwrap(),
            ArchiveOutcome::new(
                RestoreCopyReceipt {
                    operation_id: intent.operation_id.clone(),
                    source_project_id: "original-project".into(),
                    source_archive_sha256: intent.expected_archive_sha256.clone(),
                    document: document.clone(),
                    replayed,
                },
                completion,
            ),
        );
        reply.validate_for(&request).unwrap();
        let ProjectArchiveOutcome::RestoredCopy {
            project,
            completion,
            ..
        } = reply.outcome
        else {
            unreachable!()
        };
        assert_eq!(
            (project.project_id.0.as_str(), project.revision),
            ("destination-project", 7)
        );
        assert_eq!(
            completion == ProjectArchiveCopyCompletion::RecordedReplay,
            replayed
        );
    }
}

#[test]
fn matching_reference_is_independent_of_outcome_shape_and_retirement_core() {
    let request = export_request();
    let mut reply = successful_export(&request);
    let ProjectArchiveOutcome::Exported { receipt, .. } = &mut reply.outcome else {
        unreachable!()
    };
    receipt.project_id = ProjectId("wrong-project".into());
    assert!(reply.matches_request_reference(&request));
    assert!(reply.validate_for(&request).is_err());
    reply.catalog_retirement = Some(ProjectArchiveCatalogRetirement {
        sqlite_code: Some(10 | (3 << 8)),
        connection_unusable: false,
        detail: ProjectArchiveMessage {
            text: "real independent main retirement core".repeat(2048),
            truncated: false,
        },
    });
    let retirement = reply.retirement_error().unwrap();
    assert_eq!(retirement.kind, FailureKind::StorageUnavailable);
    assert!(
        retirement.message.len() <= MAX_PROJECT_ARCHIVE_MESSAGE_BYTES && retirement.outcome_unknown
    );
    assert!(reply.validate_for(&request).is_err());
    reply.request_sha256 = "c".repeat(64);
    assert!(!reply.matches_request_reference(&request));
    assert!(
        reply.retirement_error().is_some(),
        "Correlation caller decides whether this independent core belongs to its credited request"
    );
}

#[test]
fn constructed_sql_domains_preserve_primary_kind_without_false_private_retirement() {
    let request = identity_request();
    for origin in [
        ArchiveSqlOrigin::Input,
        ArchiveSqlOrigin::PrivateOutput,
        ArchiveSqlOrigin::LiveCatalog,
    ] {
        let error = tagged(origin, 26);
        let nested = ArchiveFailure::new(request.preflight_context(), error);
        let failure = ArchiveFailure::new(
            request.preflight_context(),
            StoreError::Archive(Box::new(nested)),
        );
        let reply = from_failure(&request.fingerprint().unwrap(), failure, true);
        reply.validate_for(&request).unwrap();
        let ProjectArchiveOutcome::Failed { problem, .. } = &reply.outcome else {
            unreachable!()
        };
        assert_eq!(problem.kind, ProjectArchiveProblemKind::Sqlite);
        assert_eq!(problem.sqlite.as_ref().unwrap().extended_code, 26);
        assert_eq!(
            reply.retirement_error().is_some(),
            origin == ArchiveSqlOrigin::LiveCatalog
        );
    }
    let failure = ArchiveFailure::new(
        request.preflight_context(),
        StoreError::Recording(RecordingError::Database(sqlite(26))),
    );
    let reply = from_failure(&request.fingerprint().unwrap(), failure, true);
    reply.validate_for(&request).unwrap();
    let ProjectArchiveOutcome::Failed { problem, .. } = &reply.outcome else {
        unreachable!()
    };
    assert_eq!(
        problem.sqlite.as_ref().unwrap().origin,
        ProjectArchiveSqlOrigin::OwnedSource
    );
    assert!(reply.catalog_retirement.is_none() && reply.retirement_error().is_none());
}

#[test]
fn preflight_primary_recording_stops_are_inferred_without_claiming_invocation() {
    for request in [export_request(), identity_request(), copy_request()] {
        for error in [
            RecordingError::InspectionCancelled,
            RecordingError::InspectionLimit {
                resource: InspectionResource::WorkBytes,
                limit: 7,
                attempted: 8,
            },
        ] {
            let failure =
                ArchiveFailure::new(request.preflight_context(), StoreError::Recording(error));
            let reply = from_failure(&request.fingerprint().unwrap(), failure, false);
            reply.validate_for(&request).unwrap();
            assert!(!reply.invoked);
            let ProjectArchiveOutcome::Failed { context, problem } = reply.outcome else {
                unreachable!()
            };
            assert_eq!(context.phase, ProjectArchivePhase::NotStarted);
            assert!(problem.governor_stop.is_some());
        }
    }
}

#[test]
fn actual_export_catalog_source_and_all_initial_partial_contexts_are_represented_exactly() {
    let export = export_request();
    let mut context = export.preflight_context();
    context.source = Some(export.origin_root.join("studio.sqlite3"));
    context.staging = Some(stage(&export));
    let reply = from_failure(
        &export.fingerprint().unwrap(),
        ArchiveFailure::new(
            context,
            StoreError::Invalid("original export refusal".into()),
        ),
        true,
    );
    reply.validate_for(&export).unwrap();
    for request in [export_request(), identity_request(), copy_request()] {
        let action = match request.operation {
            ProjectArchiveOperation::Export { .. } => ArchiveAction::Export,
            ProjectArchiveOperation::RestoreIdentity { .. } => ArchiveAction::Restore,
            ProjectArchiveOperation::RestoreCopy { .. } => ArchiveAction::Copy,
        };
        let original = ArchiveContext::new(action, &request.origin_root);
        let reply = from_failure(
            &request.fingerprint().unwrap(),
            ArchiveFailure::new(
                original.clone(),
                StoreError::Recording(RecordingError::InspectionCancelled),
            ),
            true,
        );
        reply.validate_for(&request).unwrap();
        let ProjectArchiveOutcome::Failed { context, .. } = reply.outcome else {
            unreachable!()
        };
        assert_eq!(context.destination, original.destination);
        assert!(
            context.source.is_none()
                && context.operation_id.is_none()
                && context.expected_archive_sha256.is_none()
        );
    }
}

#[test]
fn later_phase_missing_or_unrelated_context_paths_cannot_borrow_request_authority() {
    let request = copy_request();
    let mut context = request.preflight_context();
    context.phase = ArchivePhase::CatalogCommitAttempted;
    context.staging = Some(stage(&request));
    context.archive_sha256 = Some("a".repeat(64));
    context.published_objects = 2;
    let reply = from_failure(
        &request.fingerprint().unwrap(),
        ArchiveFailure::new(context, StoreError::Invalid("commit refusal".into())),
        true,
    );
    reply.validate_for(&request).unwrap();
    for slot in ["source", "operation", "stage", "destination", "pin"] {
        let mut changed = reply.clone();
        let ProjectArchiveOutcome::Failed { context, .. } = &mut changed.outcome else {
            unreachable!()
        };
        match slot {
            "source" => context.source = None,
            "operation" => context.operation_id = None,
            "stage" => context.staging = Some("/unrelated/private.partial".into()),
            "destination" => context.destination = "/unrelated/catalog".into(),
            _ => context.expected_archive_sha256 = Some("b".repeat(64)),
        }
        assert!(changed.matches_request_reference(&request));
        assert!(changed.validate_for(&request).is_err());
    }
}

#[test]
fn private_primary_stop_and_retirement_are_separate_transport_facts() {
    // Explicit DTO construction models a private primary plus an independently
    // retained main cleanup fatal; it is not physical mixed-fault execution.
    let request = identity_request();
    let failure = ArchiveFailure::new(
        request.preflight_context(),
        tagged(ArchiveSqlOrigin::Input, 26),
    );
    let mut reply = from_failure(&request.fingerprint().unwrap(), failure, true);
    let ProjectArchiveOutcome::Failed { problem, .. } = &mut reply.outcome else {
        unreachable!()
    };
    problem.governor_stop = Some(ProjectArchiveBudgetStop::Cancelled {});
    reply.catalog_retirement = Some(ProjectArchiveCatalogRetirement {
        sqlite_code: Some(10),
        connection_unusable: true,
        detail: ProjectArchiveMessage::from_display(&"main rollback failed"),
    });
    reply.validate_for(&request).unwrap();
    let ProjectArchiveOutcome::Failed { problem, .. } = reply.outcome else {
        unreachable!()
    };
    assert_eq!(problem.kind, ProjectArchiveProblemKind::Sqlite);
    assert_eq!(
        problem.sqlite.unwrap().origin,
        ProjectArchiveSqlOrigin::Input
    );
    assert!(matches!(
        problem.governor_stop,
        Some(ProjectArchiveBudgetStop::Cancelled {})
    ));
}

#[test]
fn nonfatal_or_private_sql_cannot_retire_a_catalog_by_message_text() {
    let request = identity_request();
    let mut reply = from_failure(
        &request.fingerprint().unwrap(),
        ArchiveFailure::new(
            request.preflight_context(),
            StoreError::Invalid("SQLITE_CORRUPT catalog connection unavailable".into()),
        ),
        true,
    );
    assert!(reply.retirement_error().is_none());
    reply.catalog_retirement = Some(ProjectArchiveCatalogRetirement {
        sqlite_code: Some(5),
        connection_unusable: false,
        detail: ProjectArchiveMessage::from_display(&"fatal looking text"),
    });
    assert!(reply.retirement_error().is_none() && reply.validate_for(&request).is_err());
    reply
        .catalog_retirement
        .as_mut()
        .unwrap()
        .connection_unusable = true;
    reply.catalog_retirement.as_mut().unwrap().sqlite_code = None;
    assert!(reply.retirement_error().is_some());
    reply.validate_for(&request).unwrap();
}

#[test]
fn completed_conversion_cannot_be_erased_by_a_later_governor_stop() {
    let request = export_request();
    let reference = request.fingerprint().unwrap();
    let token = Arc::new(AtomicBool::new(false));
    let governor =
        omniusong_recording::InspectionBudget::new(Default::default(), Arc::clone(&token)).unwrap();
    let mut outcome = ArchiveOutcome::new(receipt(&request), ArchiveCompletion::ExportDurable);
    outcome.cleanup.push(ArchiveCleanupIssue {
        path: stage(&request),
        error: std::io::Error::from_raw_os_error(13),
    });
    token.store(true, Ordering::Release);
    let reply = from_export(&reference, outcome);
    assert!(matches!(
        governor.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    reply.validate_for(&request).unwrap();
    let ProjectArchiveOutcome::Exported { cleanup, .. } = reply.outcome else {
        panic!("Late stop erased durable completion");
    };
    assert_eq!(cleanup.issues[0].raw_os_error, Some(13));
}

#[test]
fn cleanup_report_paths_and_message_wire_size_are_independently_validated() {
    let request = export_request();
    let mut reply = successful_export(&request);
    let ProjectArchiveOutcome::Exported { cleanup, .. } = &mut reply.outcome else {
        unreachable!()
    };
    cleanup.issues.push(ProjectArchiveCleanupIssue {
        path: "/unrelated/path".into(),
        detail: ProjectArchiveMessage::from_display(&"not authority"),
        raw_os_error: None,
    });
    assert!(reply.validate_for(&request).is_err());
    let ProjectArchiveOutcome::Exported { cleanup, .. } = &mut reply.outcome else {
        unreachable!()
    };
    cleanup.issues[0].path = stage(&request);
    cleanup.issues[0].detail.text = "x".repeat(MAX_PROJECT_ARCHIVE_MESSAGE_BYTES + 1);
    assert!(reply.validate_for(&request).is_err());
}

#[test]
fn bounded_reply_decode_preserves_invalid_body_retirement_for_ordered_handling() {
    let request = export_request();
    let mut reply = successful_export(&request);
    reply.invoked = false;
    reply.catalog_retirement = Some(ProjectArchiveCatalogRetirement {
        sqlite_code: Some(26),
        connection_unusable: true,
        detail: ProjectArchiveMessage::from_display(&"independent main fatal"),
    });
    let bytes = encode(&reply, MAX_PROJECT_ARCHIVE_REPLY_BYTES).unwrap();
    let decoded = ProjectArchiveReply::from_json(&bytes).unwrap();
    assert!(decoded.matches_request_reference(&request));
    assert_eq!(
        decoded.retirement_error().unwrap().kind,
        FailureKind::StorageUnavailable
    );
    assert!(decoded.validate_for(&request).is_err());
    assert!(
        ProjectArchiveReply::from_json(&vec![b' '; MAX_PROJECT_ARCHIVE_REPLY_BYTES + 1]).is_err()
    );
}

#[test]
fn placement_and_publication_phase_facts_require_their_observed_prerequisites() {
    for request in [export_request(), identity_request(), copy_request()] {
        let mut reply = from_failure(
            &request.fingerprint().unwrap(),
            ArchiveFailure::new(
                request.preflight_context(),
                StoreError::Invalid("shape control".into()),
            ),
            false,
        );
        let ProjectArchiveOutcome::Failed { context, .. } = &mut reply.outcome else {
            unreachable!()
        };
        context.confirmed_objects = 1;
        assert!(reply.validate_for(&request).is_err());
        let ProjectArchiveOutcome::Failed { context, .. } = &mut reply.outcome else {
            unreachable!()
        };
        context.confirmed_objects = 0;
        context.phase = match request.operation {
            ProjectArchiveOperation::Export { .. } => ProjectArchivePhase::OutputLinked,
            _ => ProjectArchivePhase::CatalogCommitAttempted,
        };
        reply.invoked = true;
        assert!(reply.validate_for(&request).is_err());
    }
    let request = copy_request();
    let mut context = request.preflight_context();
    context.phase = ArchivePhase::RecordedReplay;
    let mut reply = from_failure(
        &request.fingerprint().unwrap(),
        ArchiveFailure::new(
            context,
            StoreError::Recording(RecordingError::InspectionCancelled),
        ),
        true,
    );
    reply.validate_for(&request).unwrap();
    let ProjectArchiveOutcome::Failed { context, .. } = &mut reply.outcome else {
        unreachable!()
    };
    context.confirmed_objects = 1;
    assert!(reply.validate_for(&request).is_err());
}

#[test]
fn missing_retirement_metadata_cannot_hide_an_explicit_live_primary_fatal() {
    let request = identity_request();
    for origin in [
        ArchiveSqlOrigin::Input,
        ArchiveSqlOrigin::PrivateOutput,
        ArchiveSqlOrigin::LiveCatalog,
    ] {
        let mut reply = from_failure(
            &request.fingerprint().unwrap(),
            ArchiveFailure::new(request.preflight_context(), tagged(origin, 10 | (3 << 8))),
            true,
        );
        reply.catalog_retirement = None;
        assert!(reply.matches_request_reference(&request));
        assert_eq!(
            reply.retirement_error().is_some(),
            origin == ArchiveSqlOrigin::LiveCatalog
        );
        assert_eq!(
            reply.validate_for(&request).is_err(),
            origin == ArchiveSqlOrigin::LiveCatalog
        );
    }
    let mut reply = from_failure(
        &request.fingerprint().unwrap(),
        ArchiveFailure::new(
            request.preflight_context(),
            StoreError::Recording(RecordingError::Database(sqlite(26))),
        ),
        true,
    );
    reply.catalog_retirement = None;
    assert!(reply.retirement_error().is_none());
}

#[test]
fn acquired_second_check_recorded_replay_keeps_its_observed_stage_and_fingerprint() {
    // Constructor representation of the real second-check path, not an
    // execution claim for the concurrent copy-admission race.
    let request = copy_request();
    let mut context = request.preflight_context();
    context.phase = ArchivePhase::RecordedReplay;
    context.staging = Some(stage(&request));
    context.archive_sha256 = Some("a".repeat(64));
    let mut reply = from_failure(
        &request.fingerprint().unwrap(),
        ArchiveFailure::new(
            context,
            StoreError::Recording(RecordingError::InspectionCancelled),
        ),
        true,
    );
    reply.validate_for(&request).unwrap();
    let ProjectArchiveOutcome::Failed { context, .. } = &mut reply.outcome else {
        unreachable!()
    };
    assert_eq!(context.staging.as_ref(), Some(&stage(&request)));
    context.archive_sha256 = None;
    assert!(reply.matches_request_reference(&request));
    assert!(reply.validate_for(&request).is_err());
}

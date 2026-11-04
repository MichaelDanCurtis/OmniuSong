//! Source-only regressions for metadata traversal. Fixtures are in-memory and
//! never open a retained original, a default catalog, or an installed binary.
use super::*;
use crate::StoreError;
use omniusong_core::{
    contracts::{EditOperation, EditScope, OperationId, ProjectEditIntent},
    native::TakeReference,
};
use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
use rusqlite::types::Value as SqlValue;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn budget(max_work_bytes: u64) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(
            InspectionLimits {
                max_work_bytes,
                ..Default::default()
            },
            Arc::clone(&cancelled),
        )
        .unwrap(),
        cancelled,
    )
}
fn ordinary_budget() -> InspectionBudget {
    budget(InspectionLimits::default().max_work_bytes).0
}
fn catalog() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,payload TEXT,revision INTEGER,cursor INTEGER);
        CREATE TABLE edits(project_id TEXT,sequence INTEGER,before_json TEXT,after_json TEXT,PRIMARY KEY(project_id,sequence));
        CREATE TABLE operations(operation_id TEXT PRIMARY KEY,project_id TEXT,request_json TEXT,request_sha256 TEXT,previous_revision INTEGER,committed_revision INTEGER,result_json TEXT,result_sha256 TEXT);
        CREATE TABLE imports(project_id TEXT PRIMARY KEY,source_sha256 TEXT,original_json BLOB);
        CREATE TABLE assets(id TEXT PRIMARY KEY,payload BLOB);
        CREATE TABLE native_takes(take_id TEXT PRIMARY KEY,manifest_sha256 TEXT,object_sha256 TEXT,byte_length INTEGER,manifest_json TEXT);").unwrap();
    connection
}
fn changed(base: &ProjectDocument, revision: u64, name: &str) -> ProjectDocument {
    let mut value = base.value().clone();
    value["revision"] = revision.into();
    value["name"] = name.into();
    // The exact JSON spelling/whitespace and these lookalikes remain opaque.
    value["vendor:archive"] = json!({"project_id":"opaque-project","operation_id":"opaque-operation","source":{"take_id":"opaque-take"},"assetId":"opaque-asset","text":"line\nquote\"slash\\é\u{0001}"});
    value["performanceProjections"] =
        json!({"version":999,"entries":[{"source":{"takeId":"opaque-take"}}]});
    ProjectDocument::from_value(value).unwrap()
}
fn raw(document: &ProjectDocument) -> String {
    format!(" \n{}\n ", serde_json::to_string(document.value()).unwrap())
}
fn put_project(connection: &Connection, document: &ProjectDocument, cursor: i64) {
    connection
        .execute(
            "INSERT INTO projects VALUES(?1,?2,?3,?4)",
            params![
                document.id(),
                raw(document),
                document.revision() as i64,
                cursor
            ],
        )
        .unwrap();
}
fn put_operation(
    connection: &Connection,
    document: &ProjectDocument,
    operation_id: &str,
    previous: u64,
) {
    let intent = ProjectEditIntent {
        operation_id: OperationId(operation_id.into()),
        project_id: ProjectId(document.id().into()),
        expected_revision: previous,
        scope: EditScope::Project,
        edit: EditOperation::Rename {
            name: document.name().into(),
        },
        extensions: std::collections::BTreeMap::from([(
            "vendor".into(),
            json!({"project_id":"opaque-project","operation_id":operation_id,"source":{"take_id":"opaque-take"},"lanes":[{"noteId":"opaque-note"}]}),
        )]),
    };
    let request = serde_json::to_string(&intent).unwrap();
    let result = raw(document);
    connection
        .execute(
            "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                operation_id,
                document.id(),
                request,
                checksum(request.as_bytes()),
                previous as i64,
                document.revision() as i64,
                result,
                checksum(result.as_bytes())
            ],
        )
        .unwrap();
}
fn fixture() -> (Connection, String) {
    let connection = catalog();
    let base = ProjectDocument::new("Archive metadata").unwrap();
    let before = changed(&base, 0, "Before");
    let after = changed(&base, 1, "After");
    let current = changed(&base, 2, "Current");
    put_project(&connection, &current, 1);
    connection
        .execute(
            "INSERT INTO edits VALUES(?1,1,?2,?3)",
            params![base.id(), raw(&before), raw(&after)],
        )
        .unwrap();
    put_operation(&connection, &current, "ordinary-rename", 1);
    let original = raw(&before).into_bytes();
    connection
        .execute(
            "INSERT INTO imports VALUES(?1,?2,?3)",
            params![base.id(), checksum(&original), original],
        )
        .unwrap();
    (connection, base.id().into())
}
fn rows(connection: &Connection, table: &str) -> Vec<Vec<SqlValue>> {
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
        .unwrap();
    let count = statement.column_count();
    let mut cursor = statement.query([]).unwrap();
    let mut output = Vec::new();
    while let Some(row) = cursor.next().unwrap() {
        output.push((0..count).map(|index| row.get(index).unwrap()).collect());
    }
    output
}
fn assert_work_limit<T>(result: Result<T>) {
    assert!(matches!(
        result,
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }))
    ));
}

#[test]
fn exact_legacy_logical_index_totals_and_opaque_rows_match() {
    let (source, project) = fixture();
    let legacy_destination = catalog();
    let metadata_destination = catalog();
    let legacy = archive::copy_metadata(
        &source,
        Some(Destination::Store(&legacy_destination)),
        &project,
    )
    .unwrap();
    let governed = copy_metadata(
        &source,
        Some(Destination::PlanCatalog(&metadata_destination)),
        &project,
        ArchiveSqlOrigin::Input,
        &ordinary_budget(),
    )
    .unwrap();
    assert_eq!(governed.project, legacy.project);
    assert_eq!(governed.totals, legacy.totals);
    assert_eq!(governed.assets, legacy.assets);
    assert_eq!(governed.takes, legacy.takes);
    assert_eq!(governed.index_digest(), legacy.index_digest());
    for table in ["projects", "edits", "operations", "imports"] {
        assert_eq!(rows(&source, table), rows(&metadata_destination, table));
        assert_eq!(
            rows(&legacy_destination, table),
            rows(&metadata_destination, table)
        );
    }
    assert!(
        governed.assets.is_empty() && governed.takes.is_empty(),
        "Opaque lookalikes acquired owned references"
    );
    assert_eq!(
        (
            governed.totals.edits,
            governed.totals.operations,
            governed.totals.imports
        ),
        (1, 1, 1)
    );
}

fn untrusted_schema_three(base: &ProjectDocument) -> (ProjectDocument, TakeReference) {
    let source = TakeReference {
        take_id: TakeId("unopened-metadata-source".into()),
        manifest_sha256: "a".repeat(64),
        stream_id: "audio".into(),
    };
    let digest = "b".repeat(64);
    let review = json!({"version":1,"review_sha256":digest,"request":{"source":{"take_id":source.take_id.0,"manifest_sha256":source.manifest_sha256}}}).to_string();
    let mut value = base.value().clone();
    value["schemaVersion"] = 3.into();
    value["performanceProjections"] = json!({"version":1,"entries":[{"id":"claimed-projection","version":1,"source":source,
        "review":{"version":1,"canonicalJson":review,"reviewSha256":digest},
        "lanes":[{"source":1,"trackId":base.tracks()[0]["id"],"instrument":"keys","originalNotes":[{"noteId":"claimed-original-note","identitySha256":"c".repeat(64),"values":{"pitch":60,"start_tick":0,"duration_ticks":1,"velocity16":65535}}]}]}]});
    (ProjectDocument::from_value(value).unwrap(), source)
}

#[test]
fn schema_three_rows_are_untrusted_while_legacy_guard_remains_closed() {
    let source = catalog();
    let base = ProjectDocument::new("Untrusted registry").unwrap();
    let (before, reference) = untrusted_schema_three(&base);
    let revised = |revision: u64, name: &str| {
        let mut value = before.value().clone();
        value["revision"] = revision.into();
        value["name"] = name.into();
        ProjectDocument::from_value(value).unwrap()
    };
    let after = revised(1, "After");
    let document = revised(2, "Current");
    put_project(&source, &document, 1);
    source
        .execute(
            "INSERT INTO edits VALUES(?1,1,?2,?3)",
            params![document.id(), raw(&before), raw(&after)],
        )
        .unwrap();
    put_operation(&source, &document, "untrusted-registry-rename", 1);
    let destination = catalog();
    let result = copy_metadata(
        &source,
        Some(Destination::PlanCatalog(&destination)),
        document.id(),
        ArchiveSqlOrigin::Input,
        &ordinary_budget(),
    )
    .unwrap();
    assert_eq!(result.project, document);
    assert_eq!(result.takes, BTreeSet::from([reference]));
    for table in ["projects", "edits", "operations"] {
        assert_eq!(rows(&source, table), rows(&destination, table));
    }
    assert!(
        matches!(crate::load_document(&source, document.id()), Err(StoreError::Invalid(message)) if message.contains("projection"))
    );
    assert!(
        matches!(archive::copy_metadata(&source, None, document.id()), Err(StoreError::Invalid(message)) if message.contains("projection"))
    );
    // There is intentionally no native_takes row and no original file. The
    // successful traversal establishes neither availability nor source truth.
    assert!(rows(&source, "native_takes").is_empty());
}

#[test]
fn bounded_prepayment_precedes_damaged_large_current_json() {
    let source = catalog();
    let project = "metadata-project";
    source
        .execute(
            "INSERT INTO projects VALUES(?1,?2,0,0)",
            params![project, "{damaged JSON".repeat(16_384)],
        )
        .unwrap();
    let (budget, _) = budget(100_000);
    assert_work_limit(copy_metadata(
        &source,
        None,
        project,
        ArchiveSqlOrigin::Input,
        &budget,
    ));
    assert_work_limit(copy_metadata(
        &source,
        None,
        project,
        ArchiveSqlOrigin::Input,
        &budget,
    ));
}

#[test]
fn bounded_prepayment_precedes_damaged_large_history_json() {
    let source = catalog();
    let base = ProjectDocument::new("History prepay").unwrap();
    let document = changed(&base, 1, "Current");
    put_project(&source, &document, 1);
    source
        .execute(
            "INSERT INTO edits VALUES(?1,1,?2,?3)",
            params![base.id(), "{damaged JSON".repeat(16_384), raw(&document)],
        )
        .unwrap();
    assert_work_limit(copy_metadata(
        &source,
        None,
        base.id(),
        ArchiveSqlOrigin::Input,
        &budget(1_000_000).0,
    ));
}

#[test]
fn bounded_prepayment_precedes_damaged_large_operation_json() {
    let (source, project) = fixture();
    let malformed = "{damaged result".repeat(16_384);
    source
        .execute(
            "UPDATE operations SET result_json=?1,result_sha256=?2",
            params![malformed, checksum(malformed.as_bytes())],
        )
        .unwrap();
    assert_work_limit(copy_metadata(
        &source,
        None,
        &project,
        ArchiveSqlOrigin::Input,
        &budget(1_000_000).0,
    ));
}

#[test]
fn count_and_cursor_preflight_precede_any_current_document_parse() {
    for bad_cursor in [false, true] {
        let source = catalog();
        source
            .execute(
                "INSERT INTO projects VALUES('metadata-project','{damaged',0,?1)",
                [if bad_cursor { 1 } else { 0 }],
            )
            .unwrap();
        if !bad_cursor {
            source.execute_batch("WITH RECURSIVE n(v) AS (VALUES(1) UNION ALL SELECT v+1 FROM n WHERE v<10001) INSERT INTO edits SELECT 'metadata-project',v,'{damaged','{damaged' FROM n;").unwrap();
        }
        let result = copy_metadata(
            &source,
            None,
            "metadata-project",
            ArchiveSqlOrigin::Input,
            &ordinary_budget(),
        );
        assert!(
            matches!(result, Err(StoreError::Invalid(message)) if message.contains(if bad_cursor { "cursor" } else { "row count" }))
        );
    }
}

#[test]
fn original_imports_still_require_musical_schema_one() {
    let source = catalog();
    let document = ProjectDocument::new("Legacy import").unwrap();
    put_project(&source, &document, 0);
    let native_original = document.upgrade_native().unwrap().to_json().unwrap();
    source
        .execute(
            "INSERT INTO imports VALUES(?1,?2,?3)",
            params![document.id(), checksum(&native_original), native_original],
        )
        .unwrap();
    assert!(
        matches!(copy_metadata(&source, None, document.id(), ArchiveSqlOrigin::Input, &ordinary_budget()), Err(StoreError::Invalid(message)) if message == "Original legacy import must use musical schema 1")
    );
}

#[test]
fn source_sql_errors_keep_the_explicit_handle_origin() {
    for origin in [
        ArchiveSqlOrigin::Input,
        ArchiveSqlOrigin::PrivateOutput,
        ArchiveSqlOrigin::LiveCatalog,
    ] {
        let source = Connection::open_in_memory().unwrap();
        let error = match copy_metadata(
            &source,
            None,
            "metadata-project",
            origin,
            &ordinary_budget(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("Missing source table was admitted"),
        };
        assert_eq!(
            error.catalog_sqlite_error_code().is_some(),
            origin == ArchiveSqlOrigin::LiveCatalog
        );
        let StoreError::ArchiveSql(error) = error else {
            panic!("Source SQL error lost provenance");
        };
        assert_eq!(error.origin(), origin);
    }
}

#[test]
fn private_destination_sql_is_never_retagged_as_live_source_sql() {
    let (source, project) = fixture();
    let destination = Connection::open_in_memory().unwrap();
    let error = match copy_metadata(
        &source,
        Some(Destination::PlanCatalog(&destination)),
        &project,
        ArchiveSqlOrigin::LiveCatalog,
        &ordinary_budget(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("Missing private destination table was admitted"),
    };
    assert_eq!(error.catalog_sqlite_error_code(), None);
    let StoreError::ArchiveSql(error) = error else {
        panic!("Private SQL error lost provenance");
    };
    assert_eq!(error.origin(), ArchiveSqlOrigin::PrivateOutput);
}

#[test]
fn private_notadb_survives_late_cancellation_and_live_fatal_keeps_main_domain() {
    for origin in [
        ArchiveSqlOrigin::Input,
        ArchiveSqlOrigin::PrivateOutput,
        ArchiveSqlOrigin::LiveCatalog,
    ] {
        let (budget, cancelled) = budget(100_000);
        let primary = sql_error(
            origin,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
                None,
            ),
        );
        cancelled.store(true, Ordering::Release);
        let error = finish::<()>(&budget, Err(primary)).unwrap_err();
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionCancelled)
        ));
        assert_eq!(
            error.catalog_connection_unusable(),
            origin == ArchiveSqlOrigin::LiveCatalog
        );
        let StoreError::ArchiveSql(error) = error else {
            panic!("Late cancellation replaced primary SQL failure");
        };
        assert_eq!(error.origin(), origin);
        assert!(
            matches!(error.operation_error(), StoreError::Database(error) if error.sqlite_error().is_some_and(|code| code.extended_code == rusqlite::ffi::SQLITE_NOTADB))
        );
    }
}

#[test]
fn bounded_document_encoding_matches_core_pretty_bytes_with_opaque_escapes() {
    let base = ProjectDocument::new("Encoding").unwrap();
    let document = changed(&base, 0, "Encoded");
    assert_eq!(
        document_json(&document, &ordinary_budget())
            .unwrap()
            .as_bytes(),
        document.to_json().unwrap()
    );
    let mut value = document.value().clone();
    value["vendor:large"] = Value::String("x".repeat(100_000));
    let document = ProjectDocument::from_value(value).unwrap();
    assert_work_limit(document_json(&document, &budget(500_000).0));
}

#[test]
fn source_attachment_reserved_operations_stay_forbidden_in_store_and_plan_rows() {
    let source = catalog();
    let base = ProjectDocument::new("Reserved").unwrap();
    let document = changed(&base, 1, "Current");
    put_project(&source, &document, 0);
    put_operation(&source, &document, "os-src-attach:old-receipt", 0);
    assert!(
        copy_metadata(
            &source,
            None,
            base.id(),
            ArchiveSqlOrigin::Input,
            &ordinary_budget()
        )
        .is_ok()
    );
    for plan in [false, true] {
        let destination = catalog();
        let target = if plan {
            Destination::PlanCatalog(&destination)
        } else {
            Destination::Store(&destination)
        };
        assert!(
            matches!(copy_metadata(&source, Some(target), base.id(), ArchiveSqlOrigin::Input, &ordinary_budget()), Err(StoreError::Invalid(message)) if message.contains("dedicated history-fenced bridge"))
        );
        assert!(rows(&destination, "operations").is_empty());
    }
}

#[test]
fn take_manifest_and_asset_materialization_require_prepaid_work_and_correct_types() {
    let source = catalog();
    source
        .execute(
            "INSERT INTO native_takes VALUES('bounded-take',?1,?2,1,?3)",
            params![
                "a".repeat(64),
                "b".repeat(64),
                "{damaged manifest".repeat(16_384)
            ],
        )
        .unwrap();
    assert_work_limit(read_take_metadata(
        &source,
        "bounded-take",
        ArchiveSqlOrigin::Input,
        &budget(500_000).0,
    ));
    let bytes = vec![7u8; 100_000];
    let id = format!("{}.wav", checksum(&bytes));
    source
        .execute("INSERT INTO assets VALUES(?1,?2)", params![id, bytes])
        .unwrap();
    assert_work_limit(read_asset(
        &source,
        &id,
        ArchiveSqlOrigin::Input,
        &budget(500_000).0,
    ));
    assert_eq!(
        read_asset(&source, &id, ArchiveSqlOrigin::Input, &ordinary_budget()).unwrap(),
        bytes
    );
    source
        .execute("UPDATE assets SET payload='not a BLOB' WHERE id=?1", [&id])
        .unwrap();
    assert!(matches!(
        read_asset(&source, &id, ArchiveSqlOrigin::Input, &ordinary_budget()),
        Err(StoreError::Invalid(_))
    ));
}

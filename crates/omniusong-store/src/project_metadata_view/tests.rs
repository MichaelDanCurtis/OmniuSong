//! Isolated metadata fixtures. No source registration, source file, catalog
//! path or application is available to these helpers.
use super::*;
use omniusong_core::{contracts::TakeId, native::TakeReference};
use omniusong_recording::{InspectionLimits, InspectionResource, RecordingError};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn budget(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
fn fresh() -> InspectionBudget {
    budget(InspectionLimits::default()).0
}
fn catalog() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,payload TEXT,revision INTEGER);")
        .unwrap();
    connection
}
fn begin(connection: &Connection) {
    connection.execute_batch("BEGIN").unwrap();
}
fn put(connection: &Connection, document: &ProjectDocument) {
    connection
        .execute(
            "INSERT INTO projects(id,payload,revision) VALUES(?1,?2,?3)",
            params![
                document.id(),
                String::from_utf8(document.to_json().unwrap()).unwrap(),
                document.revision() as i64
            ],
        )
        .unwrap();
}
fn claimed_projection() -> ProjectDocument {
    let base = ProjectDocument::new("Untrusted metadata claim").unwrap();
    let source = TakeReference {
        take_id: TakeId("never-opened-metadata-source".into()),
        manifest_sha256: "a".repeat(64),
        stream_id: "audio".into(),
    };
    let digest = "b".repeat(64);
    let review = json!({"version":1,"review_sha256":digest,"request":{"source":{
        "take_id":source.take_id.0,"manifest_sha256":source.manifest_sha256
    }}})
    .to_string();
    let mut value = base.value().clone();
    value["schemaVersion"] = 3.into();
    value["vendor:opaque"] =
        json!({"source":{"takeId":"only-an-opaque-string"},"integer":u64::MAX,"text":"é\\\n"});
    value["performanceProjections"] = json!({"version":1,"entries":[{
        "id":"claimed-projection","version":1,"source":source,
        "review":{"version":1,"canonicalJson":review,"reviewSha256":digest},
        "lanes":[{"source":1,"trackId":base.tracks()[0]["id"],"instrument":"keys",
            "originalNotes":[{"noteId":"claimed-original-note","identitySha256":"c".repeat(64),
                "values":{"pitch":60,"start_tick":0,"duration_ticks":1,"velocity16":0}}]}]
    }]});
    ProjectDocument::from_value(value).unwrap()
}
fn assert_work_stop<T>(result: Result<T>) {
    assert!(matches!(
        result,
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: InspectionResource::WorkBytes,
            ..
        }))
    ));
}
// The first query has exactly two payload references: octet_length and typeof.
// Any subsequent payload SELECT must therefore occur only after raw prepay.
fn metadata_reads_only(connection: &Connection) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&count);
    connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "projects",
                    column_name: "payload",
                    ..
                }
            ) && observed.fetch_add(1, Ordering::SeqCst) >= 2
            {
                return Authorization::Deny;
            }
            Authorization::Allow
        }))
        .unwrap();
    count
}
fn clear_authorizer(connection: &Connection) {
    connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
}

#[test]
fn valid_nonempty_schema_three_is_metadata_without_source_tables_or_authority() {
    let connection = catalog();
    let project = claimed_projection();
    put(&connection, &project);
    begin(&connection);
    let governor = fresh();
    let summaries = list(&connection, &governor).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, project.id());
    assert_eq!(summaries[0].name, project.name());
    assert_eq!(summaries[0].track_count, project.tracks().len());
    assert_eq!(summaries[0].note_count, 0);
    assert_eq!(summaries[0].problem, None);
    assert!(summaries[0].source_authentication_required);
    assert_eq!(load(&connection, project.id(), &governor).unwrap(), project);
    assert!(matches!(
        crate::load_document(&connection, project.id()),
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(governor.usage().file_bytes, 0);
    assert_eq!(
        governor.usage().vm_steps,
        0,
        "helper installed an unowned SQL hook"
    );
    assert!(
        !connection.is_autocommit(),
        "helper ended its caller's transaction"
    );
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1, "fixture has no source registrations to consult");
}

#[test]
fn legacy_opaque_fields_empty_schema_three_and_native_metadata_keep_their_meaning() {
    let connection = catalog();
    let mut documents = Vec::new();
    for schema in [1, 2, 3] {
        let mut value = ProjectDocument::new(&format!("Metadata schema {schema}"))
            .unwrap()
            .value()
            .clone();
        value["schemaVersion"] = schema.into();
        value["vendor:exact"] = json!({"project_id":"opaque-not-rewritten","source":"not-owned"});
        value["performanceProjections"] = if schema == 3 {
            json!({"version":1,"entries":[]})
        } else {
            json!({"version":999,"entries":[{"not":"an owned binding"}]})
        };
        let document = ProjectDocument::from_value(value).unwrap();
        put(&connection, &document);
        documents.push(document);
    }
    begin(&connection);
    let governor = fresh();
    let summaries = list(&connection, &governor).unwrap();
    assert_eq!(summaries.len(), 3);
    for (summary, document) in summaries.iter().zip(documents.iter().rev()) {
        assert_eq!(summary.id, document.id());
        assert_eq!(summary.name, document.name());
        assert_eq!(summary.revision, document.revision());
        assert_eq!(summary.problem, None);
        assert!(!summary.source_authentication_required);
        assert_eq!(
            load(&connection, document.id(), &governor).unwrap(),
            *document
        );
    }
}

#[test]
fn malformed_identity_revision_and_payload_type_rows_remain_damaged_summaries() {
    let connection = catalog();
    let good = ProjectDocument::new("Stored metadata").unwrap();
    let raw = String::from_utf8(good.to_json().unwrap()).unwrap();
    connection
        .execute("INSERT INTO projects VALUES('json-bad','{damaged',4)", [])
        .unwrap();
    connection
        .execute("INSERT INTO projects VALUES('id-bad',?1,0)", [&raw])
        .unwrap();
    connection
        .execute(
            "INSERT INTO projects VALUES(?1,?2,-1)",
            params![good.id(), raw],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO projects VALUES('type-bad',?1,9)",
            [vec![1_u8, 2, 3]],
        )
        .unwrap();
    begin(&connection);
    let governor = fresh();
    let summaries = list(&connection, &governor).unwrap();
    assert_eq!(
        summaries
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["type-bad", good.id(), "id-bad", "json-bad"]
    );
    assert_eq!(summaries[1].revision, 0);
    assert_eq!(summaries[3].revision, 4);
    for summary in &summaries {
        assert_eq!(summary.name, "Damaged session · recovery needed");
        assert_eq!(summary.track_count, 0);
        assert_eq!(summary.note_count, 0);
        assert!(summary.problem.is_some());
        assert!(!summary.source_authentication_required);
    }
    assert_eq!(summaries[0].problem.as_deref(), Some(PAYLOAD_TYPE));
    assert_eq!(summaries[1].problem.as_deref(), Some(IDENTITY));
    assert_eq!(summaries[2].problem.as_deref(), Some(IDENTITY));
    assert!(matches!(
        load(&connection, "json-bad", &governor),
        Err(StoreError::Document(_))
    ));
    assert!(
        matches!(load(&connection,"id-bad",&governor), Err(StoreError::Invalid(message)) if message==IDENTITY)
    );
}

#[test]
fn missing_projects_invalid_input_and_unowned_snapshots_are_refused() {
    let connection = catalog();
    let project = ProjectDocument::new("Caller-owned snapshot").unwrap();
    put(&connection, &project);
    assert!(
        matches!(list(&connection,&fresh()), Err(StoreError::Invalid(message)) if message.contains("active caller-owned"))
    );
    assert!(
        matches!(load(&connection,project.id(),&fresh()), Err(StoreError::Invalid(message)) if message.contains("active caller-owned"))
    );
    begin(&connection);
    assert!(matches!(
        load(&connection, "missing", &fresh()),
        Err(StoreError::Database(rusqlite::Error::QueryReturnedNoRows))
    ));
    for id in [String::new(), "x".repeat(MAX_ID_BYTES + 1)] {
        assert!(matches!(
            load(&connection, &id, &fresh()),
            Err(StoreError::Invalid(_))
        ));
    }
    assert!(!connection.is_autocommit());
}

#[test]
fn damaged_large_json_is_not_requested_before_cumulative_decode_credit() {
    for library in [false, true] {
        let connection = catalog();
        connection
            .execute(
                "INSERT INTO projects VALUES('damaged',?1,0)",
                ["{not-json".repeat(16 * 1024)],
            )
            .unwrap();
        begin(&connection);
        let reads = metadata_reads_only(&connection);
        let governor = budget(InspectionLimits {
            max_work_bytes: 16_384,
            ..Default::default()
        })
        .0;
        if library {
            assert_work_stop(list(&connection, &governor));
        } else {
            assert_work_stop(load(&connection, "damaged", &governor));
        }
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "payload query was prepared before credit"
        );
        assert_work_stop(load(&connection, "damaged", &governor));
        assert_eq!(reads.load(Ordering::SeqCst), 2, "sticky stop was reset");
        clear_authorizer(&connection);
        assert!(!connection.is_autocommit());
    }
}

#[test]
fn oversized_payload_is_guarded_without_materialization_or_source_io() {
    let connection = catalog();
    connection
        .execute(
            "INSERT INTO projects VALUES('oversized',CAST(zeroblob(?1) AS TEXT),2)",
            [(MAX_NATIVE_PROJECT_BYTES + 1) as i64],
        )
        .unwrap();
    begin(&connection);
    for library in [false, true] {
        let reads = metadata_reads_only(&connection);
        let governor = fresh();
        if library {
            let summaries = list(&connection, &governor).unwrap();
            assert_eq!(summaries[0].problem.as_deref(), Some(BOUNDS));
            assert!(!summaries[0].source_authentication_required);
        } else {
            assert!(
                matches!(load(&connection,"oversized",&governor), Err(StoreError::Invalid(message)) if message==BOUNDS)
            );
        }
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert!(governor.usage().work_bytes < 32_768);
        assert_eq!(governor.usage().file_bytes, 0);
        clear_authorizer(&connection);
    }
}

#[test]
fn oversized_catalog_identity_is_refused_before_document_decoding() {
    let connection = catalog();
    connection
        .execute(
            "INSERT INTO projects VALUES(CAST(zeroblob(1048576) AS TEXT),'{}',0)",
            [],
        )
        .unwrap();
    begin(&connection);
    let reads = metadata_reads_only(&connection);
    assert!(
        matches!(list(&connection,&fresh()), Err(StoreError::Invalid(message)) if message.contains("identity") && message.contains("byte limit"))
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    clear_authorizer(&connection);
}

#[test]
fn one_governor_accumulates_across_load_and_library_and_stops_the_whole_call() {
    let connection = catalog();
    let project = ProjectDocument::new("Cumulative metadata credit").unwrap();
    put(&connection, &project);
    begin(&connection);
    let measured = fresh();
    load(&connection, project.id(), &measured).unwrap();
    let governor = budget(InspectionLimits {
        max_work_bytes: measured.usage().work_bytes + 1024,
        ..Default::default()
    })
    .0;
    assert_eq!(load(&connection, project.id(), &governor).unwrap(), project);
    assert_work_stop(list(&connection, &governor));
    assert_work_stop(load(&connection, project.id(), &governor));
    assert!(!connection.is_autocommit());
}

#[test]
fn cancellation_and_zero_row_quota_stop_before_catalog_access() {
    for cancel in [false, true] {
        let connection = catalog();
        begin(&connection);
        let observed = Arc::new(AtomicBool::new(false));
        let mark = Arc::clone(&observed);
        connection
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "projects",
                        ..
                    }
                ) {
                    mark.store(true, Ordering::SeqCst);
                }
                Authorization::Allow
            }))
            .unwrap();
        let (governor, token) = budget(InspectionLimits {
            max_row_visits: 0,
            ..Default::default()
        });
        token.store(cancel, Ordering::Release);
        let result = list(&connection, &governor);
        if cancel {
            assert!(matches!(
                result,
                Err(StoreError::Recording(RecordingError::InspectionCancelled))
            ));
        } else {
            assert!(matches!(
                result,
                Err(StoreError::Recording(RecordingError::InspectionLimit {
                    resource: InspectionResource::RowVisits,
                    ..
                }))
            ));
        }
        assert!(!observed.load(Ordering::SeqCst));
        clear_authorizer(&connection);
        assert!(!connection.is_autocommit());
    }
}

#[test]
fn sql_failure_is_not_a_damaged_summary_or_overwritten_by_late_cancellation() {
    let connection = catalog();
    let project = ProjectDocument::new("Original SQL failure").unwrap();
    put(&connection, &project);
    begin(&connection);
    let (governor, token) = budget(InspectionLimits::default());
    let reads = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&reads);
    connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "projects",
                    column_name: "payload",
                    ..
                }
            ) && count.fetch_add(1, Ordering::SeqCst) >= 2
            {
                token.store(true, Ordering::Release);
                return Authorization::Deny;
            }
            Authorization::Allow
        }))
        .unwrap();
    assert!(
        matches!(list(&connection,&governor),Err(StoreError::Database(error)) if error.sqlite_error().is_some_and(|code|code.extended_code & 0xff == 23))
    );
    assert!(matches!(
        governor.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    clear_authorizer(&connection);
}

#[test]
fn the_513th_identity_refuses_the_collection_before_its_payload_is_prepaid() {
    let connection = catalog();
    for index in 0..MAX_PROJECT_SUMMARIES {
        put(
            &connection,
            &ProjectDocument::new(&format!("Metadata row {index}")).unwrap(),
        );
    }
    begin(&connection);
    assert_eq!(
        list(&connection, &fresh()).unwrap().len(),
        MAX_PROJECT_SUMMARIES
    );
    // Oldest row will be encountered only after the 512 healthy summaries.
    connection.execute("INSERT INTO projects(rowid,id,payload,revision) VALUES(0,'unrequested-513',CAST(zeroblob(?1) AS TEXT),0)", [MAX_NATIVE_PROJECT_BYTES as i64]).unwrap();
    let governor = fresh();
    assert!(
        matches!(list(&connection,&governor),Err(StoreError::Invalid(message)) if message=="Project metadata library exceeds 512 summaries; paging is required")
    );
    assert!(
        governor.usage().work_bytes < (MAX_NATIVE_PROJECT_BYTES as u64) * 64,
        "513th payload was prepaid before collection refusal"
    );
    assert!(!connection.is_autocommit());
}

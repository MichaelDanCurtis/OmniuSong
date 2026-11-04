use super::*;
use crate::catalog_transaction::CatalogTransaction;
use omniusong_core::contracts::{EditOperation, EditScope, ProjectEditIntent};
use omniusong_recording::{InspectionLimits, RecordingError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn budget(limits: InspectionLimits) -> (InspectionBudget, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        InspectionBudget::new(limits, Arc::clone(&cancelled)).unwrap(),
        cancelled,
    )
}
struct Fixture {
    connection: Connection,
    project: String,
}
impl Fixture {
    fn new(corrupt: Option<&str>) -> Self {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,payload TEXT,revision INTEGER,cursor INTEGER);
            CREATE TABLE edits(project_id TEXT,sequence INTEGER,before_json TEXT,after_json TEXT);
            CREATE TABLE operations(operation_id TEXT PRIMARY KEY,project_id TEXT,request_json TEXT,request_sha256 TEXT,previous_revision INTEGER,committed_revision INTEGER,result_json TEXT,result_sha256 TEXT);
            CREATE TABLE assets(id TEXT PRIMARY KEY,payload BLOB);
            CREATE TABLE native_takes(take_id TEXT PRIMARY KEY,manifest_sha256 TEXT,object_sha256 TEXT,byte_length INTEGER,manifest_json TEXT);").unwrap();
        for (_, sql) in crate::archive_copy::LINEAGE_TABLES {
            connection.execute_batch(sql).unwrap();
        }
        let base = ProjectDocument::new("Current").unwrap();
        let project = base.id().to_owned();
        let doc = |name: &str, revision: u64| {
            let mut value = base.value().clone();
            value["name"] = name.into();
            value["revision"] = revision.into();
            // Legacy lookalikes remain opaque and must not acquire a source role.
            value["performanceProjections"] = serde_json::json!({"version":999,"entries":[{"source":{"takeId":"missing","manifestSha256":"bogus"}}]});
            ProjectDocument::from_value(value).unwrap()
        };
        let raw = |name: &str, revision: u64, slot: &str| {
            if corrupt == Some(slot) {
                "{malformed document".to_owned()
            } else {
                String::from_utf8(doc(name, revision).to_json().unwrap()).unwrap()
            }
        };
        connection
            .execute(
                "INSERT INTO projects VALUES(?1,?2,8,1)",
                params![project, raw("Current", 8, "current")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO edits VALUES(?1,1,?2,?3)",
                params![
                    project,
                    raw("Before", 0, "before"),
                    raw("After", 1, "after")
                ],
            )
            .unwrap();
        let intent = ProjectEditIntent {
            operation_id: OperationId("rename-one".into()),
            project_id: ProjectId(project.clone()),
            expected_revision: 3,
            scope: EditScope::Project,
            edit: EditOperation::Rename {
                name: "Operation".into(),
            },
            extensions: Default::default(),
        };
        let request = serde_json::to_string(&intent).unwrap();
        let result = raw("Operation", 4, "operation");
        connection
            .execute(
                "INSERT INTO operations VALUES(?1,?2,?3,?4,3,4,?5,?6)",
                params![
                    intent.operation_id.0,
                    project,
                    request,
                    checksum(request.as_bytes()),
                    result,
                    checksum(result.as_bytes())
                ],
            )
            .unwrap();
        let mut mapping = Sha256::new();
        mapping.update(b"OmniuSong.copy-mapping.v1\0");
        let result = if corrupt == Some("proof") {
            serde_json::json!({"not":"a project"})
        } else {
            doc("Proof", 6).value().clone()
        };
        let result_sha = if corrupt == Some("proof") {
            "a".repeat(64)
        } else {
            checksum(&doc("Proof", 6).to_json().unwrap())
        };
        let proof = ParentProof {
            version: 1,
            operation_id: OperationId("copy-admission".into()),
            source_archive_sha256: "b".repeat(64),
            source_archive_byte_length: 1,
            source_project_id: "historic-source".into(),
            destination_project_id: project.clone(),
            mapping_count: 0,
            mapping_sha256: format!("{:x}", mapping.finalize()),
            ancestors: vec![],
            result,
            result_sha256: result_sha,
        };
        let raw = serde_json::to_string(&proof).unwrap();
        let sha = checksum(raw.as_bytes());
        connection
            .execute(
                "INSERT INTO archive_copy_proofs VALUES(?1,?2)",
                params![sha, raw],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO archive_project_lineage VALUES(?1,?2)",
                params![project, sha],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO archive_copies VALUES('copy-admission',?1,?2)",
                params![project, sha],
            )
            .unwrap();
        Self {
            connection,
            project,
        }
    }
    fn inspect(&self, budget: &InspectionBudget) -> Result<RetainedProjectionClosureInspection> {
        CatalogTransaction::deferred(&self.connection, budget)?.run(|scope| {
            let result = inspect(
                scope,
                Path::new("never-opened-no-source-root"),
                &self.project,
                budget,
            )?;
            scope.rollback()?;
            Ok(result)
        })
    }
}
#[test]
fn current_both_history_sides_operation_and_parent_result_are_all_streamed() {
    let fixture = Fixture::new(None);
    let (budget, _) = budget(InspectionLimits::default());
    let inspection = fixture.inspect(&budget).unwrap();
    assert_eq!(
        inspection,
        RetainedProjectionClosureInspection {
            version: 1,
            project_id: fixture.project.clone(),
            snapshots: 5,
            history_rows: 1,
            operation_rows: 1,
            lineage_proof_rows: 1,
            distinct_bindings: 0
        }
    );
    assert_eq!(budget.usage().file_bytes, 0);
    assert!(budget.usage().vm_steps > 0);
    for location in ["current", "before", "after", "operation", "proof"] {
        let fixture = Fixture::new(Some(location));
        let (budget, _) = self::budget(InspectionLimits::default());
        assert!(
            fixture.inspect(&budget).is_err(),
            "Walker skipped {location}"
        );
        assert!(fixture.connection.is_autocommit());
    }
}
#[test]
fn oversize_rows_refuse_before_json_materialization_or_source_open() {
    for location in ["current", "before", "operation", "proof"] {
        let fixture = Fixture::new(None);
        let (sql, maximum) = match location {
            "current" => ("UPDATE projects SET payload=?1", MAX_NATIVE_PROJECT_BYTES),
            "before" => ("UPDATE edits SET before_json=?1", MAX_NATIVE_PROJECT_BYTES),
            "operation" => ("UPDATE operations SET request_json=?1", WORKER_MAX_BYTES),
            _ => (
                "UPDATE archive_copy_proofs SET receipt_json=?1",
                MAX_PROOF_BYTES,
            ),
        };
        fixture
            .connection
            .execute(sql, ["x".repeat(maximum + 1)])
            .unwrap();
        let (budget, _) = budget(InspectionLimits::default());
        let result = fixture.inspect(&budget);
        assert!(
            matches!(result, Err(StoreError::Invalid(_))),
            "Unexpected {location} refusal: {result:?}"
        );
        assert_eq!(budget.usage().file_bytes, 0);
        assert!(fixture.connection.is_autocommit());
    }
}
#[test]
fn missing_mapping_and_noncanonical_parent_proof_cannot_get_closure_success() {
    let fixture = Fixture::new(None);
    let raw: String = fixture
        .connection
        .query_row("SELECT receipt_json FROM archive_copy_proofs", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut proof: ParentProof = serde_json::from_str(&raw).unwrap();
    proof.mapping_count = 1;
    let raw = serde_json::to_string(&proof).unwrap();
    let sha = checksum(raw.as_bytes());
    // Rehash the entire fixture coherently. Defer this transaction's foreign
    // keys until all three retained associations point at the replacement.
    let mutation = fixture.connection.unchecked_transaction().unwrap();
    mutation
        .execute_batch("PRAGMA defer_foreign_keys=ON")
        .unwrap();
    mutation
        .execute(
            "UPDATE archive_copy_proofs SET receipt_json=?1,proof_sha256=?2",
            params![raw, sha],
        )
        .unwrap();
    mutation
        .execute("UPDATE archive_project_lineage SET proof_sha256=?1", [&sha])
        .unwrap();
    mutation
        .execute("UPDATE archive_copies SET proof_sha256=?1", [&sha])
        .unwrap();
    mutation.commit().unwrap();
    let (budget, _) = budget(InspectionLimits::default());
    assert!(fixture.inspect(&budget).is_err());

    let fixture = Fixture::new(None);
    let raw: String = fixture
        .connection
        .query_row("SELECT receipt_json FROM archive_copy_proofs", [], |r| {
            r.get(0)
        })
        .unwrap();
    let raw = format!(" {raw}");
    let sha = checksum(raw.as_bytes());
    // Rehash the entire fixture coherently. Defer this transaction's foreign
    // keys until all three retained associations point at the replacement.
    let mutation = fixture.connection.unchecked_transaction().unwrap();
    mutation
        .execute_batch("PRAGMA defer_foreign_keys=ON")
        .unwrap();
    mutation
        .execute(
            "UPDATE archive_copy_proofs SET receipt_json=?1,proof_sha256=?2",
            params![raw, sha],
        )
        .unwrap();
    mutation
        .execute("UPDATE archive_project_lineage SET proof_sha256=?1", [&sha])
        .unwrap();
    mutation
        .execute("UPDATE archive_copies SET proof_sha256=?1", [&sha])
        .unwrap();
    mutation.commit().unwrap();
    let (budget, _) = self::budget(InspectionLimits::default());
    assert!(fixture.inspect(&budget).is_err());
}
#[test]
fn cancellation_vm_and_work_refusals_stay_typed_and_cleanup_allows_reuse() {
    let fixture = Fixture::new(None);
    for limits in [
        InspectionLimits {
            max_work_bytes: 1,
            ..Default::default()
        },
        InspectionLimits {
            max_vm_steps: 1,
            ..Default::default()
        },
    ] {
        let (budget, _) = budget(limits);
        let result = fixture.inspect(&budget);
        assert!(
            matches!(
                result,
                Err(StoreError::Recording(
                    RecordingError::InspectionLimit { .. }
                ))
            ),
            "{result:?}"
        );
        assert!(fixture.connection.is_autocommit());
    }
    let (budget, cancelled) = budget(InspectionLimits::default());
    cancelled.store(true, Ordering::Release);
    assert!(matches!(
        fixture.inspect(&budget),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    cancelled.store(false, Ordering::Release);
    assert!(matches!(
        fixture.inspect(&budget),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    let (healthy, _) = self::budget(InspectionLimits::default());
    fixture.inspect(&healthy).unwrap();
}
#[test]
fn explicit_history_count_cap_precedes_materialization() {
    let fixture = Fixture::new(None);
    // Count refusal precedes parsing even though all inserted payloads are bad.
    fixture.connection.execute_batch("WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<10001) INSERT INTO edits SELECT (SELECT id FROM projects),x,'bad','bad' FROM n;").unwrap();
    let (budget, _) = budget(InspectionLimits::default());
    assert!(
        matches!(fixture.inspect(&budget),Err(StoreError::Invalid(message)) if message.contains("row count"))
    );
    assert_eq!(budget.usage().record_visits, 1);
}
#[test]
fn repeated_snapshot_asset_references_are_unioned_and_authenticated_once() {
    let fixture = Fixture::new(None);
    let bytes = b"isolated synthetic asset bytes; no audio decoding or output";
    let id = format!("{}.wav", checksum(bytes));
    fixture
        .connection
        .execute(
            "INSERT INTO assets VALUES(?1,?2)",
            params![id, bytes.as_slice()],
        )
        .unwrap();
    let mut value = ProjectDocument::new("Repeated asset references")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["assetId"] = id.clone().into();
    value["tracks"][0]["sourceAssetId"] = id.clone().into();
    let document = ProjectDocument::from_value(value).unwrap();
    let (budget, _) = budget(InspectionLimits::default());
    let mut collect = Collect {
        set: RegisteredSourceSet::new(&budget),
        takes: BTreeSet::new(),
        assets: BTreeSet::new(),
        budget: &budget,
    };
    for _ in 0..3 {
        collect.document(&document).unwrap();
    }
    assert_eq!(collect.assets, BTreeSet::from([id.clone()]));
    let before = budget.usage().work_bytes;
    authenticate_assets(&fixture.connection, &collect.assets, &budget).unwrap();
    assert_eq!(
        budget.usage().work_bytes - before,
        bytes.len() as u64 * 4 + 3 * 4096
    );
    fixture
        .connection
        .execute(
            "UPDATE assets SET payload=?1 WHERE id=?2",
            params![b"coherently sized but different bytes".as_slice(), id],
        )
        .unwrap();
    assert!(authenticate_assets(&fixture.connection, &collect.assets, &budget).is_err());
}

#[test]
fn dense_sorted_ancestry_comparisons_share_work_limits_and_sticky_cancellation() {
    let available: Vec<_> = (0..256).map(|n| format!("{n:064x}")).collect();
    let required: Vec<_> = available.iter().step_by(2).cloned().collect();
    let (healthy, _) = budget(InspectionLimits::default());
    assert!(sorted_subset(&required, &available, &healthy).unwrap());
    assert!(healthy.usage().work_bytes >= 255 * 256);
    let mut missing = required.clone();
    missing.push(format!("{:064x}", 257));
    assert!(!sorted_subset(&missing, &available, &healthy).unwrap());

    let (limited, _) = budget(InspectionLimits {
        max_work_bytes: 1024,
        ..Default::default()
    });
    assert!(matches!(
        sorted_subset(&required, &available, &limited),
        Err(StoreError::Recording(RecordingError::InspectionLimit {
            resource: omniusong_recording::InspectionResource::WorkBytes,
            ..
        }))
    ));
    let (stopped, token) = budget(InspectionLimits::default());
    token.store(true, Ordering::Release);
    assert!(matches!(
        sorted_subset(&required, &available, &stopped),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    let usage = stopped.usage();
    token.store(false, Ordering::Release);
    assert!(matches!(
        sorted_subset(&required, &available, &stopped),
        Err(StoreError::Recording(RecordingError::InspectionCancelled))
    ));
    assert_eq!(usage, stopped.usage());
}

fn retained_rows(connection: &Connection) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
    let mut names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    names.push("sqlite_schema".into());
    names
        .into_iter()
        .map(|name| {
            let mut statement = connection
                .prepare(&format!(
                    "SELECT * FROM \"{}\" ORDER BY rowid",
                    name.replace('"', "\"\"")
                ))
                .unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<Vec<rusqlite::types::Value>>>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}

#[test]
fn reusable_validator_keeps_separate_namespace_walks_under_one_original_governor() {
    let first = Fixture::new(None);
    let second = Fixture::new(None);
    let original = [
        retained_rows(&first.connection),
        retained_rows(&second.connection),
    ];
    let (budget, _) = budget(InspectionLimits::default());
    let (walk, set, assets) = collect(
        &first.connection,
        RetainedLayout::Catalog,
        &first.project,
        ArchiveSqlOrigin::PrivateOutput,
        &budget,
    )
    .unwrap()
    .into_parts();
    assert!(assets.is_empty());
    let sources = RegisteredProjectionSources::open_collected_from(
        Path::new("never-opened-empty-source-root"),
        &first.connection,
        set,
        ArchiveSqlOrigin::PrivateOutput,
    )
    .unwrap();
    let proofs = sources.authenticate(&budget).unwrap();
    let mut validator = RetainedValidator::new(&proofs, &budget).unwrap();
    let first_summary = validator
        .validate(&first.connection, ArchiveSqlOrigin::PrivateOutput, &walk)
        .unwrap();
    let (second_walk, unused_set, _) = collect(
        &second.connection,
        RetainedLayout::Catalog,
        &second.project,
        ArchiveSqlOrigin::PrivateOutput,
        &budget,
    )
    .unwrap()
    .into_parts();
    drop(unused_set);
    let second_summary = validator
        .validate(
            &second.connection,
            ArchiveSqlOrigin::PrivateOutput,
            &second_walk,
        )
        .unwrap();
    validator
        .verify_final(&second.connection, ArchiveSqlOrigin::PrivateOutput)
        .unwrap();
    assert_ne!(first_summary.project_id, second_summary.project_id);
    assert_eq!((first_summary.snapshots, second_summary.snapshots), (5, 5));
    assert_eq!(
        (
            first_summary.lineage_proof_rows,
            second_summary.lineage_proof_rows
        ),
        (1, 1)
    );
    assert_eq!(budget.usage().file_bytes, 0);
    assert_eq!(
        [
            retained_rows(&first.connection),
            retained_rows(&second.connection)
        ],
        original
    );
    let (other, _) = self::budget(InspectionLimits::default());
    assert!(
        matches!(RetainedValidator::new(&proofs, &other), Err(StoreError::Invalid(message))
        if message.contains("cannot change its cumulative inspection governor"))
    );
    assert_eq!(other.usage(), Default::default());
}

#[test]
fn osar_v1_walk_never_queries_lineage_but_v2_still_requires_its_tables() {
    let fixture = Fixture::new(None);
    fixture.connection.execute_batch("DROP TABLE archive_copies; DROP TABLE archive_project_lineage; DROP TABLE archive_copy_proof_operations; DROP TABLE archive_copy_proofs;").unwrap();
    let before = retained_rows(&fixture.connection);
    let (budget, _) = budget(InspectionLimits::default());
    let (walk, set, _) = collect(
        &fixture.connection,
        RetainedLayout::Osar { version: 1 },
        &fixture.project,
        ArchiveSqlOrigin::Input,
        &budget,
    )
    .unwrap()
    .into_parts();
    let sources = RegisteredProjectionSources::open_collected_from(
        Path::new("never-opened-v1-root"),
        &fixture.connection,
        set,
        ArchiveSqlOrigin::Input,
    )
    .unwrap();
    let proofs = sources.authenticate(&budget).unwrap();
    let mut validator = RetainedValidator::new(&proofs, &budget).unwrap();
    let summary = validator
        .validate(&fixture.connection, ArchiveSqlOrigin::Input, &walk)
        .unwrap();
    assert_eq!((summary.snapshots, summary.lineage_proof_rows), (4, 0));
    for version in [0, 3] {
        assert!(
            matches!(collect(&fixture.connection, RetainedLayout::Osar { version }, &fixture.project,
            ArchiveSqlOrigin::Input, &budget), Err(StoreError::Invalid(message))
            if message == "Unsupported retained closure archive version")
        );
    }
    let Err(StoreError::ArchiveSql(error)) = collect(
        &fixture.connection,
        RetainedLayout::Osar { version: 2 },
        &fixture.project,
        ArchiveSqlOrigin::Input,
        &budget,
    ) else {
        panic!("v2 silently skipped missing lineage")
    };
    assert_eq!(error.origin(), ArchiveSqlOrigin::Input);
    assert_eq!(retained_rows(&fixture.connection), before);
}

#[test]
fn retained_first_walk_is_exact_and_a_refused_validator_cannot_be_reused() {
    let fixture = Fixture::new(None);
    let (budget, _) = budget(InspectionLimits::default());
    let (walk, set, _) = collect(
        &fixture.connection,
        RetainedLayout::Catalog,
        &fixture.project,
        ArchiveSqlOrigin::PrivateOutput,
        &budget,
    )
    .unwrap()
    .into_parts();
    let sources = RegisteredProjectionSources::open_collected_from(
        Path::new("never-opened-no-takes"),
        &fixture.connection,
        set,
        ArchiveSqlOrigin::PrivateOutput,
    )
    .unwrap();
    let proofs = sources.authenticate(&budget).unwrap();
    let mut validator = RetainedValidator::new(&proofs, &budget).unwrap();
    fixture
        .connection
        .execute(
            "UPDATE projects SET payload=json_set(payload,'$.name','Different but valid')",
            [],
        )
        .unwrap();
    let before = retained_rows(&fixture.connection);
    assert!(
        matches!(validator.validate(&fixture.connection, ArchiveSqlOrigin::PrivateOutput, &walk),
        Err(StoreError::Invalid(message)) if message == "Retained closure changed between its source collection and semantic passes")
    );
    assert!(
        matches!(validator.validate(&fixture.connection, ArchiveSqlOrigin::PrivateOutput, &walk),
        Err(StoreError::Invalid(message)) if message == "Retained closure validator already failed")
    );
    assert!(
        matches!(validator.verify_final(&fixture.connection, ArchiveSqlOrigin::PrivateOutput),
        Err(StoreError::Invalid(message)) if message == "Retained closure validator already failed")
    );
    assert_eq!(retained_rows(&fixture.connection), before);
}

#[test]
fn a_retained_walk_cannot_rebind_its_first_pass_to_another_governor() {
    let fixture = Fixture::new(None);
    let (opening, _) = budget(InspectionLimits::default());
    let (walk, _, _) = collect(
        &fixture.connection,
        RetainedLayout::Catalog,
        &fixture.project,
        ArchiveSqlOrigin::PrivateOutput,
        &opening,
    )
    .unwrap()
    .into_parts();
    let (other, _) = budget(InspectionLimits::default());
    let sources = RegisteredProjectionSources::open_collected_from(
        Path::new("never-opened-empty-set"),
        &fixture.connection,
        RegisteredSourceSet::new(&other),
        ArchiveSqlOrigin::PrivateOutput,
    )
    .unwrap();
    let proofs = sources.authenticate(&other).unwrap();
    let mut validator = RetainedValidator::new(&proofs, &other).unwrap();
    let before = other.usage();
    assert!(
        matches!(validator.validate(&fixture.connection, ArchiveSqlOrigin::PrivateOutput, &walk),
        Err(StoreError::Invalid(message)) if message.contains("cannot change its cumulative inspection governor"))
    );
    assert_eq!(other.usage(), before);
}

#[test]
fn namespace_link_checks_do_not_accidentally_visit_a_prior_namespace_memo() {
    let fixture = Fixture::new(None);
    let (budget, _) = budget(InspectionLimits::default());
    let (walk, set, _) = collect(
        &fixture.connection,
        RetainedLayout::Catalog,
        &fixture.project,
        ArchiveSqlOrigin::PrivateOutput,
        &budget,
    )
    .unwrap()
    .into_parts();
    let sources = RegisteredProjectionSources::open_collected_from(
        Path::new("never-opened-empty-memo"),
        &fixture.connection,
        set,
        ArchiveSqlOrigin::PrivateOutput,
    )
    .unwrap();
    let proofs = sources.authenticate(&budget).unwrap();
    let mut validator = RetainedValidator::new(&proofs, &budget).unwrap();
    // Synthetic private-state bookkeeping control, not a source-authenticated
    // binding: invalid bytes make any accidental prior-namespace lookup fail.
    validator
        .bindings
        .insert("prior-namespace-only".into(), vec![0]);
    validator.encountered.insert("prior-namespace-only".into());
    let summary = validator
        .validate(&fixture.connection, ArchiveSqlOrigin::PrivateOutput, &walk)
        .unwrap();
    assert_eq!(summary.distinct_bindings, 0);
    assert_eq!(validator.bindings["prior-namespace-only"], vec![0]);
}

#[test]
fn exact_binding_memo_refuses_a_changed_same_id_before_any_new_source_proof() {
    let fixture = Fixture::new(None);
    let (budget, _) = budget(InspectionLimits::default());
    let sources = RegisteredProjectionSources::open_collected_from(
        Path::new("never-opened-memo-source"),
        &fixture.connection,
        RegisteredSourceSet::new(&budget),
        ArchiveSqlOrigin::PrivateOutput,
    )
    .unwrap();
    let proofs = sources.authenticate(&budget).unwrap();
    let mut validator = RetainedValidator::new(&proofs, &budget).unwrap();
    let binding: PerformanceProjectionBinding = serde_json::from_value(serde_json::json!({
        "id":"memo-projection","version":1,"source":{"takeId":"memo-source","manifestSha256":"a".repeat(64),"streamId":"audio"},
        "review":{"version":1,"canonicalJson":"retained exact bytes","reviewSha256":"b".repeat(64)},
        "lanes":[{"source":1,"trackId":"memo-track","instrument":"keys","originalNotes":[{"noteId":"memo-note","identitySha256":"c".repeat(64),"values":{"pitch":60,"start_tick":0,"duration_ticks":1,"velocity16":65535}}]}]
    })).unwrap();
    // Seed only the private memo, explicitly not an authenticated source. This
    // isolates byte equality from the independently required full proof path.
    let bytes = serde_json::to_vec(&binding).unwrap();
    validator.bindings.insert(binding.id.clone(), bytes.clone());
    validator.binding(&binding).unwrap();
    let mut changed = binding.clone();
    changed.lanes[0].original_notes[0].values.pitch = 61;
    assert!(
        matches!(validator.binding(&changed), Err(StoreError::Invalid(message))
        if message == "Retained snapshots reuse a projection identity with different original binding")
    );
    assert_eq!(validator.bindings[&binding.id], bytes);
    let mut fresh = binding;
    fresh.id = "never-authenticated-new-binding".into();
    assert!(
        validator.binding(&fresh).is_err(),
        "memo seeding must not authenticate a new identity"
    );
}

#[test]
fn explicit_origin_preserves_an_actual_sqlite_fault_and_reports_late_stop_separately() {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/retained-reuse-notadb")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("private.sqlite3");
    std::fs::write(&path, [0xa5; 4096]).unwrap();
    let connection = Connection::open(&path).unwrap();
    for sql_origin in [
        ArchiveSqlOrigin::Input,
        ArchiveSqlOrigin::PrivateOutput,
        ArchiveSqlOrigin::LiveCatalog,
    ] {
        let (budget, stop) = budget(InspectionLimits::default());
        let actual = connection
            .query_row("SELECT name FROM sqlite_schema", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap_err();
        assert_eq!(
            actual.sqlite_error().unwrap().extended_code & 0xff,
            rusqlite::ffi::SQLITE_NOTADB
        );
        stop.store(true, Ordering::Release);
        let result: Result<()> = preserve(
            &budget,
            origin(sql_origin, Err(StoreError::Database(actual))),
        );
        let Err(StoreError::ArchiveSql(error)) = result else {
            panic!("actual SQL fault was erased by a stop")
        };
        assert_eq!(error.origin(), sql_origin);
        assert_eq!(
            error.catalog_sqlite_error_code().is_some(),
            sql_origin == ArchiveSqlOrigin::LiveCatalog
        );
        assert!(matches!(
            budget.checkpoint(),
            Err(RecordingError::InspectionCancelled)
        ));
        assert!(matches!(
            preserve(&budget, Ok(())),
            Err(StoreError::Recording(RecordingError::InspectionCancelled))
        ));
    }
    let (budget, _) = budget(InspectionLimits::default());
    let Err(StoreError::ArchiveSql(error)) = collect(
        &connection,
        RetainedLayout::Osar { version: 1 },
        "isolated-project",
        ArchiveSqlOrigin::Input,
        &budget,
    ) else {
        panic!("collector lost private NOTADB origin")
    };
    assert_eq!(error.origin(), ArchiveSqlOrigin::Input);
    drop(connection);
    assert_eq!(std::fs::read(&path).unwrap(), [0xa5; 4096]);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn actual_ordinary_audio_remains_a_nonperformance_role_and_final_source_change_refuses() {
    use omniusong_core::{
        contracts::{ClockDomainId, ClockEpochId, HostTimestamp, TakeId},
        native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
    };
    use omniusong_recording::{
        CHUNK_VERSION, ChunkHeader, ChunkKind, HostClock, JOURNAL_VERSION, Journal, PcmEncoding,
        PcmFormat, PcmLayout, SealedJournal, SourceRange, StreamDescriptor, StreamKind,
        TakeDescriptor,
    };
    use std::{fs, io::Write};
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/retained-reuse-audio")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory).unwrap();
    let directory = fs::canonicalize(directory).unwrap();
    let original = directory.join("original.sqlite3");
    let descriptor = TakeDescriptor {
        version: JOURNAL_VERSION,
        take_id: TakeId("ordinary-audio-original".into()),
        streams: vec![StreamDescriptor {
            stream_id: "audio".into(),
            segment_id: "segment".into(),
            source_id: "synthetic".into(),
            source_name: "Device-free original".into(),
            source_connection_generation: 1,
            host_clock: HostClock {
                domain: ClockDomainId("host".into()),
                epoch: ClockEpochId("boot".into()),
                ticks_per_second_numerator: 1000,
                ticks_per_second_denominator: 1,
            },
            kind: StreamKind::Audio {
                format: PcmFormat {
                    sample_rate: 48_000,
                    channels: 1,
                    encoding: PcmEncoding::S16Le,
                    layout: PcmLayout::Interleaved,
                },
                audio_epoch: ClockEpochId("audio".into()),
            },
            initial_source_position: 0,
        }],
        provenance: Default::default(),
    };
    let stamp = HostTimestamp {
        domain: ClockDomainId("host".into()),
        epoch: ClockEpochId("boot".into()),
        ticks: 1,
    };
    let mut journal = Journal::create(&original, &descriptor).unwrap();
    journal
        .append(
            &ChunkHeader {
                version: CHUNK_VERSION,
                take_id: descriptor.take_id.clone(),
                stream_id: "audio".into(),
                segment_id: "segment".into(),
                sequence: 0,
                range: SourceRange::AudioFrames { start: 0, count: 1 },
                interpretation: descriptor.streams[0].kind.clone(),
                observed_start: stamp.clone(),
                observed_end: stamp,
                clock_anchors: vec![],
                kind: ChunkKind::Content,
            },
            &[1, 2],
        )
        .unwrap();
    drop(journal);
    let mut journal = Journal::open(&original).unwrap();
    journal.seal_recovered("retained-reuse-recovery").unwrap();
    drop(journal);
    let source = SealedJournal::open(&original).unwrap();
    let original_bytes = fs::read(&original).unwrap();
    let mut store = crate::NativeStore::open(directory.join("owned-store")).unwrap();
    let object = store.ingest_take(&source).unwrap();
    let reference = TakeReference {
        take_id: object.take_id.clone(),
        manifest_sha256: object.manifest_sha256.clone(),
        stream_id: "audio".into(),
    };
    let mut document = ProjectDocument::new("Ordinary audio source role")
        .unwrap()
        .upgrade_native()
        .unwrap();
    document
        .append_take_clip(
            "ordinary-originals",
            "Original audio",
            TakeClip {
                id: "ordinary-clip".into(),
                source: reference.clone(),
                range: TakeRange::AudioFrames { start: 0, count: 1 },
                position: TimelinePosition {
                    frame: 0,
                    sample_rate: 48_000,
                },
                fades: None,
                edit_source: None,
            },
        )
        .unwrap();
    let document = store.create(&document).unwrap();
    let owned = crate::takes::object_path(store.root(), &object.object_sha256).unwrap();
    let owned_bytes = fs::read(&owned).unwrap();
    let before = retained_rows(&store.connection);
    let (budget, _) = budget(InspectionLimits::default());
    CatalogTransaction::deferred(&store.connection, &budget)
        .unwrap()
        .run(|tx| {
            let (walk, set, _) = collect(
                tx,
                RetainedLayout::Catalog,
                document.id(),
                ArchiveSqlOrigin::LiveCatalog,
                &budget,
            )?
            .into_parts();
            let sources = RegisteredProjectionSources::open_collected_from(
                store.root(),
                tx,
                set,
                ArchiveSqlOrigin::LiveCatalog,
            )?;
            let proofs = sources.authenticate(&budget)?;
            assert!(
                matches!(proofs.performance(&reference), Err(StoreError::Invalid(message))
            if message == "Take has no held performance authentication")
            );
            let (held, path) = proofs.original(&reference)?;
            assert_eq!(held, &object);
            assert_eq!(path, owned.as_path());
            let mut validator = RetainedValidator::new(&proofs, &budget)?;
            assert_eq!(
                validator
                    .validate(tx, ArchiveSqlOrigin::LiveCatalog, &walk)?
                    .distinct_bindings,
                0
            );
            validator.verify_final(tx, ArchiveSqlOrigin::LiveCatalog)?;
            assert_eq!(fs::read(&owned).unwrap(), owned_bytes);
            fs::OpenOptions::new()
                .append(true)
                .open(&owned)
                .unwrap()
                .write_all(&[0x7f])
                .unwrap();
            let deliberate_mutation = fs::read(&owned).unwrap();
            assert!(
                matches!(validator.verify_final(tx, ArchiveSqlOrigin::LiveCatalog),
            Err(StoreError::Invalid(message)) if message == "Registered source length changed")
            );
            assert_eq!(fs::read(&owned).unwrap(), deliberate_mutation);
            tx.rollback()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(retained_rows(&store.connection), before);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    assert_eq!(
        fs::read(&owned).unwrap(),
        [owned_bytes.as_slice(), &[0x7f]].concat()
    );
    drop(store);
    drop(source);
    fs::remove_dir_all(directory).unwrap();
}

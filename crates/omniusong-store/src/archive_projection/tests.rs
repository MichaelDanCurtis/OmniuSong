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

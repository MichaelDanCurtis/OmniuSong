//! Reconciliation authenticates an existing artifact; it cannot create one.
use super::*;

/// Verifies the exact recorded existing export without consulting its live
/// source root. Missing, partial or different artifacts remain unresolved.
/// This never invokes the fresh-export path, creates output or repairs bytes.
pub fn reconcile_evidence_export(
    path: impl AsRef<Path>,
    expected_export_id: &str,
    expected_origin_root: &str,
    budget: &InspectionBudget,
) -> Result<EvidenceArchiveInspection> {
    let result = (|| {
        budget.checkpoint()?;
        let path = path.as_ref();
        let path_text = path.to_str().ok_or_else(|| {
            StoreError::Invalid("Recorded evidence archive path is not UTF8".into())
        })?;
        require(
            path.is_absolute()
                && path.file_name().is_some()
                && path_text.len() <= 4096
                && !path_text.chars().any(char::is_control),
            "Evidence reconciliation needs a bounded exact recorded absolute archive path",
        )?;
        require(
            valid_id(expected_export_id),
            "Evidence reconciliation needs the recorded nonzero32hex export identity",
        )?;
        require(
            expected_origin_root.starts_with('/')
                && expected_origin_root.len() <= 4096
                && !expected_origin_root.chars().any(char::is_control),
            "Evidence reconciliation needs bounded absolute origin provenance",
        )?;
        // Admit bounded argument/ancestor observations before inspection clones
        // the archive name for its owner-sidecar guard or opens any descriptor.
        budget.charge_work(
            (path_text.len() + expected_export_id.len() + expected_origin_root.len() + 160) as u64,
        )?;
        for ancestor in path.ancestors() {
            budget.charge_work((ancestor.as_os_str().len() as u64 + 160) * 8)?;
            budget.visit_rows(8)?;
        }
        // Inspection opens only existing bytes and validates their full
        // container, object and known catalog-reference closure. The original
        // root may be missing; its exact string is historical provenance.
        let inspection = inspect_evidence_archive(path, budget)?;
        require(
            inspection.receipt.path == path
                && inspection.receipt.export_id == expected_export_id
                && inspection.manifest.origin_root == expected_origin_root,
            "Existing evidence differs from the recorded export identity or origin",
        )?;
        Ok(inspection)
    })();
    budget.checkpoint()?;
    result
}

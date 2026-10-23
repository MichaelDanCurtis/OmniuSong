use super::*;

#[test]
fn projection_finalizer_retains_original_failure_beside_later_sticky_stop() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let token = Arc::new(AtomicBool::new(false));
    let budget = InspectionBudget::new(
        omniusong_recording::InspectionLimits::default(),
        Arc::clone(&token),
    )
    .unwrap();
    let source_error: Result<()> = Err(RecordingError::Invalid("original changed".into()));
    token.store(true, Ordering::Release);
    assert!(
        matches!(finish(&budget, source_error), Err(RecordingError::Invalid(value)) if value=="original changed")
    );
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    assert!(matches!(
        finish(&budget, Ok(())),
        Err(RecordingError::InspectionCancelled)
    ));
}

pub(super) fn request() -> PerformanceProjectionRequest {
    PerformanceProjectionRequest {
        version: 1,
        source: PerformanceProjectionSourcePin {
            take_id: "projection-test-take".into(),
            manifest_sha256: "1".repeat(64),
            descriptor_sha256: "2".repeat(64),
            original_rows_sha256: "3".repeat(64),
            journal_sha256_at_review: "4".repeat(64),
            checkpoint_sha256: "5".repeat(64),
            selection: RecoveryPrefix {
                chunk_count: 2,
                chunk_index_sha256: "6".repeat(64),
            },
        },
        region: PerformanceFrameRegion { start: 0, end: 40 },
        sources: vec![1],
        tempo_bits: 125.0f64.to_bits(),
        destination_start_tick: 0,
        rounding: PerformanceProjectionRounding::NearestTiesUp,
        duration: PerformanceProjectionDuration::ReleaseGate,
        collapsed: PerformanceProjectionPolicy::Reject,
        incomplete: PerformanceProjectionPolicy::Reject,
        region_crossing: PerformanceProjectionPolicy::Reject,
    }
}

#[test]
fn exact_shape_rejects_noncanonical_pins_and_ambiguous_selection() {
    let base = request();
    base.validate().unwrap();
    let mut bad = base.clone();
    bad.source.take_id = String::new();
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.source.original_rows_sha256 = "A".repeat(64);
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.source.checkpoint_sha256.push('0');
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.source.selection.chunk_count = 0;
    assert!(bad.validate().is_err());
    for handles in [
        vec![],
        vec![0],
        vec![5],
        vec![2, 1],
        vec![1, 1],
        vec![1, 2, 3, 4, 4],
    ] {
        let mut bad = base.clone();
        bad.sources = handles;
        assert!(bad.validate().is_err());
    }
    for region in [
        PerformanceFrameRegion { start: 4, end: 4 },
        PerformanceFrameRegion { start: 5, end: 4 },
        PerformanceFrameRegion {
            start: 0,
            end: u64::MAX,
        },
    ] {
        let mut bad = base.clone();
        bad.region = region;
        assert!(bad.validate().is_err());
    }
    let mut bad = base;
    bad.destination_start_tick = omniusong_core::note_edit_types::MAX_NOTE_START_TICK + 1;
    assert!(bad.validate().is_err());
}

#[test]
fn versioned_request_and_gate_dtos_refuse_unknown_authority_fields() {
    let mut encoded = serde_json::to_value(request()).unwrap();
    encoded
        .as_object_mut()
        .unwrap()
        .insert("publish".into(), true.into());
    assert!(serde_json::from_value::<PerformanceProjectionRequest>(encoded).is_err());
    for text in [
        r#"{"kind":"note_off","frame":99}"#,
        r#"{"kind":"finish","release_velocity16":12}"#,
    ] {
        assert!(serde_json::from_str::<PerformanceGateEndCause>(text).is_err());
    }
    let mut req = request();
    req.tempo_bits = 120.0f64.to_bits() + 1;
    let encoded = serde_json::to_vec(&req).unwrap();
    assert_eq!(
        serde_json::from_slice::<PerformanceProjectionRequest>(&encoded).unwrap(),
        req
    );
}

#[test]
fn bounded_canonical_hash_uses_one_sticky_governor() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let flag = Arc::new(AtomicBool::new(false));
    let budget = InspectionBudget::new(
        omniusong_recording::InspectionLimits::default(),
        flag.clone(),
    )
    .unwrap();
    let one = digest(b"projection-test-one\0", &request(), 4096, &budget).unwrap();
    let two = digest(b"projection-test-two\0", &request(), 4096, &budget).unwrap();
    assert_ne!(one, two);
    assert!(digest(b"projection-test-one\0", &request(), 8, &budget).is_err());
    flag.store(true, Ordering::Release);
    assert!(matches!(
        digest(b"projection-test-one\0", &request(), 4096, &budget),
        Err(RecordingError::InspectionCancelled)
    ));
    flag.store(false, Ordering::Release);
    assert!(matches!(
        budget.checkpoint(),
        Err(RecordingError::InspectionCancelled)
    ));
    let limited = InspectionBudget::new(
        omniusong_recording::InspectionLimits {
            max_work_bytes: 1,
            ..Default::default()
        },
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(matches!(
        digest(b"projection-test-one\0", &request(), 4096, &limited),
        Err(RecordingError::InspectionLimit {
            resource: omniusong_recording::InspectionResource::WorkBytes,
            ..
        })
    ));
}

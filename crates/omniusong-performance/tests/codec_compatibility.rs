//! Fixed bytes from a retained synthetic performance made before extraction.
//! This oracle is independent of the new crate's descriptor/record encoder.
use omniusong_performance::performance_records::{
    PerformanceConfig, PerformanceProfile, STREAM_IDS, create_descriptor_for, decode_envelope,
    encode_record,
};
use omniusong_recording::TakeDescriptor;
use serde_json::Value;
use sha2::{Digest, Sha256};

const RETAINED: &[u8] = include_bytes!("fixtures/retained-performance-v2.json");

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn pre_extraction_descriptor_and_eight_capture_records_keep_exact_bytes() {
    assert_eq!(
        sha(RETAINED),
        "6f1bc73da795dfc09ab5f317ac6ce02f0df84b766ed8dc14aa35d3b9fc5ee3e9"
    );
    let fixture: Value = serde_json::from_slice(RETAINED).unwrap();
    assert_eq!(fixture["version"], 1);
    assert_eq!(
        fixture["source_raw_sha256"],
        "f6e5aef06a0abf7fa020731eb1f1023e2ca7a5847f581651b129cd92c248a111"
    );
    let bytes = fixture["descriptor_encoded"].as_str().unwrap().as_bytes();
    assert_eq!(sha(bytes), fixture["descriptor_bytes_sha256"]);
    let descriptor: TakeDescriptor = serde_json::from_slice(bytes).unwrap();
    let config = PerformanceConfig::from_descriptor(&descriptor).unwrap();
    assert_eq!(
        create_descriptor_for(&config, PerformanceProfile::FinishV2).unwrap(),
        descriptor
    );
    assert_eq!(serde_json::to_vec(&descriptor).unwrap(), bytes);

    let records = fixture["records"].as_array().unwrap();
    assert_eq!(records.len(), 8);
    for retained in records {
        let bytes = retained["encoded"].as_str().unwrap().as_bytes();
        assert_eq!(sha(bytes), retained["sha256"]);
        let (envelope, record) = decode_envelope(bytes).unwrap();
        assert_eq!(record.version, 2);
        let stream = retained["stream_id"].as_str().unwrap();
        let index = STREAM_IDS.iter().position(|id| *id == stream).unwrap();
        assert_eq!(
            encode_record(
                &config,
                index,
                &record,
                envelope.payload.time.observed.ticks,
                envelope.sequence,
            )
            .unwrap(),
            bytes,
            "Changed the retained {stream} record"
        );
    }
}

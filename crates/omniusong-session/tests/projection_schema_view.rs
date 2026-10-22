//! Pure worker-prepared document views: no filesystem, database or device access.
use omniusong_core::{
    ProjectDocument,
    contracts::TakeId,
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_session::DocumentView;
use serde_json::{Value, json};

fn clip() -> TakeClip {
    TakeClip {
        id: "saved-native-clip".into(),
        source: TakeReference {
            take_id: TakeId("synthetic-view-source".into()),
            manifest_sha256: "d".repeat(64),
            stream_id: "audio".into(),
        },
        range: TakeRange::AudioFrames {
            start: 12,
            count: 64,
        },
        position: TimelinePosition {
            frame: -4,
            sample_rate: 8_000,
        },
        fades: None,
        edit_source: None,
    }
}

#[test]
fn empty_schema_three_view_retains_typed_recordings_and_opaque_note_extensions_separately() {
    let mut value = ProjectDocument::new("Read compatibility is not publication proof")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["recording"] =
        json!({"takeId":"opaque-provider-id","clips":{"not":"a native payload"}});
    value["tracks"][0]["provenance"] = json!({"provider":"legacy","source":{"takeId":"foreign"}});
    let legacy = ProjectDocument::from_value(value).unwrap();
    let mut document = legacy.upgrade_performance_projection_schema().unwrap();
    let native = clip();
    document
        .append_take_clip(
            "recording-track",
            "Ordinary sealed recording",
            native.clone(),
        )
        .unwrap();
    let before = document.to_json().unwrap();
    let view = DocumentView::prepare(document).unwrap();
    assert_eq!(view.document.schema_version(), 3);
    assert_eq!(view.document.to_json().unwrap(), before);
    assert_eq!(view.recordings.len(), view.document.tracks().len());
    assert!(view.recordings[0].is_none());
    assert!(view.recordings[1].is_none());
    assert_eq!(view.recordings[2].as_ref().unwrap().clips, vec![native]);
    assert!(view.provenance[0].as_ref().unwrap().contains("legacy"));
    assert!(view.provenance[2].is_none());
    assert_eq!(view.document.tracks()[0], legacy.tracks()[0]);
}

#[test]
fn schema_one_and_two_same_named_projection_metadata_stays_opaque_in_prepared_views() {
    for schema in [1, 2] {
        for opaque in [Value::Null, json!({"version":99,"entries":{"not":"owned"}})] {
            let mut value = ProjectDocument::new("Exact legacy namespace")
                .unwrap()
                .value()
                .clone();
            value["schemaVersion"] = schema.into();
            value["performanceProjections"] = opaque.clone();
            value["tracks"][0]["recording"] = json!({"version":"foreign","clips":null});
            value["tracks"][0]["performanceProjections"] = json!(["unrelated track extension"]);
            let document = ProjectDocument::from_value(value).unwrap();
            assert!(document.performance_projections().unwrap().is_none());
            assert!(document.referenced_takes().unwrap().is_empty());
            assert!(document.upgrade_performance_projection_schema().is_err());
            let before = document.to_json().unwrap();
            let view = DocumentView::prepare(document).unwrap();
            assert_eq!(view.document.schema_version(), schema);
            assert_eq!(view.document.value()["performanceProjections"], opaque);
            assert_eq!(view.document.to_json().unwrap(), before);
            assert!(view.recordings.iter().all(Option::is_none));
        }
    }
}

//! Device-free clip-edit agreement with the actual preparer and callback cursor.
//! Synthetic originals only; no native stream, live catalog or physical timing claim.
use omniusong_core::{
    AudioSourceWindow, ClipPatch, ClipTimelinePolicy, DocumentClipAddress, FieldChange,
    LegacyAudioClipPatch, NativeAudioClipPatch, ProjectDocument, ValueChange,
    native::{AudioFades, FadeCurve, TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_engine::{realtime::RenderCursor, render::prepare};
use omniusong_recording::{
    ChunkHeader, ChunkKind, ClockDomainId, ClockEpochId, GapReason, HostClock, HostTimestamp,
    Journal, PcmEncoding, PcmFormat, PcmLayout, SealedJournal, SourceRange, StreamDescriptor,
    StreamKind, TakeDescriptor, TakeId,
};
use omniusong_store::NativeStore;
use serde_json::{Number, Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.runtime/clip-render-review");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Synthetic clip render evidence retained: {}",
                self.0.display()
            );
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn decimal(raw: &str) -> Number {
    raw.parse().unwrap()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn wav(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
    assert!(samples.len().is_multiple_of(usize::from(channels)));
    let payload: Vec<_> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    let align = channels * 4;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes()); // IEEE float32, not encoded device input.
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
    bytes.extend_from_slice(&align.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}
fn retain_non_audio_evidence(value: &mut Value) {
    value["tracks"][1]["muted"] = true.into();
    value["tracks"][1]["notes"] = json!([{
        "id":"original-note", "pitch":60,"startTick":17,"durationTicks":961,"velocity16":65535,
        "expression":[{"type":"pressure","points":[{"tick":0,"value32":1},{"tick":961,"value32":4294967295u32}]}],
        "input":{"sourceId":"synthetic-keys","sourceName":"No hardware","protocol":"midi2","group":3,"channel":5,"receivedTimeMs":1.25,"releaseVelocity16":60001,"keyDurationTicks":300}
    }]);
    value["inputCapture"] = json!({"version":1,"droppedEvents":0,"events":[{
        "trackId":value["tracks"][1]["id"],"tick":17,"sourceId":"synthetic-keys","sourceName":"No hardware","receivedTimeMs":1.25,
        "hostTime":"18446744073709551615","format":"ump","words":[1083194368u32,4294967295u32]
    }]});
    value["vendor:exact"] = serde_json::from_str(
        r#"{"integer":184467440737095516160001,"decimal":0.12345678901234567890123456789}"#,
    )
    .unwrap();
}
fn assert_evidence(before: &ProjectDocument, after: &ProjectDocument) {
    assert_eq!(after.value()["tracks"][1], before.value()["tracks"][1]);
    assert_eq!(
        after.value()["inputCapture"],
        before.value()["inputCapture"]
    );
    assert_eq!(
        after.value()["vendor:exact"],
        before.value()["vendor:exact"]
    );
    assert_eq!(after.id(), before.id());
    assert_eq!(
        after.revision(),
        before.revision(),
        "Pure clip changes do not publish revisions"
    );
}
/// Exercise the same cursor invoked by production callbacks, including final
/// clamping and silence after the prepared end, in awkward whole-frame blocks.
fn callback_pcm(
    store: &NativeStore,
    document: &ProjectDocument,
    rate: u32,
    channels: u16,
) -> Vec<f32> {
    let prepared = prepare(store, document, rate, channels).unwrap();
    let frames = prepared.len() / usize::from(channels);
    let (mut cursor, meters) = RenderCursor::new(prepared, channels).unwrap();
    let mut rendered = Vec::new();
    let mut remaining = frames;
    for chunk in [1usize, 7, 3, 257].into_iter().cycle() {
        if remaining == 0 {
            break;
        }
        let count = chunk.min(remaining);
        let mut buffer = vec![f32::NAN; count * usize::from(channels)];
        cursor.render_into(&mut buffer);
        assert!(buffer.iter().all(|sample| sample.is_finite()));
        rendered.extend_from_slice(&buffer);
        remaining -= count;
    }
    assert!(cursor.is_finished());
    assert_eq!(meters.snapshot().frames, frames as u64);
    let mut tail = vec![9.0; usize::from(channels) * 11];
    cursor.render_into(&mut tail);
    assert!(tail.iter().all(|sample| *sample == 0.0));
    assert_eq!(meters.snapshot().frames, frames as u64);
    rendered
}
fn near(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() <= 0.000001,
            "sample {index}: {actual} != {expected}"
        );
    }
}

struct LegacyScene {
    store: NativeStore,
    document: ProjectDocument,
    address: DocumentClipAddress,
    asset_id: String,
    original: Vec<u8>,
    asset_path: PathBuf,
    source_json: PathBuf,
    original_json: Vec<u8>,
}
impl LegacyScene {
    fn new(fixture: &Fixture, rate: u32, channels: u16, samples: &[f32]) -> Self {
        let original = wav(rate, channels, samples);
        let asset_id = format!("{}.wav", sha(&original));
        let assets = fixture.0.join("assets");
        fs::create_dir(&assets).unwrap();
        let asset_path = assets.join(&asset_id);
        fs::write(&asset_path, &original).unwrap();
        let mut value = ProjectDocument::new("Synthetic legacy clip render")
            .unwrap()
            .value()
            .clone();
        value["tempo"] = 120.into();
        value["tracks"][0]["id"] = "\0鍵盤/audio".into();
        value["tracks"][0]["kind"] = "audio".into();
        value["tracks"][0]["gain"] = 1.0.into();
        value["tracks"][0]["assetId"] = asset_id.clone().into();
        // Synthetic historical recorder metadata is evidence to preserve, not
        // an assertion that this fixture ever opened a microphone.
        value["tracks"][0]["audio"] = json!({"origin":"microphone","mimeType":"audio/wav",
            "durationSeconds":samples.len() as f64 / f64::from(channels) / f64::from(rate),
            "byteLength":original.len(),"sourceIdentity":{"kind":"synthetic","doNotRewrite":[null,"original"]},
            "recordingTiming":{"version":1,"method":"browser-recorder-start-call","calibration":"unmeasured","captureAudioTimeSeconds":8.0,"songStartAudioTimeSeconds":9.0,"tempo":120,"ppq":960,"loopTicks":3840,"status":"ended-before-song"}});
        retain_non_audio_evidence(&mut value);
        let source = ProjectDocument::from_value(value).unwrap();
        let original_json = source.to_json().unwrap();
        let source_json = fixture.0.join("source.json");
        fs::write(&source_json, &original_json).unwrap();
        let mut store = NativeStore::open(fixture.0.join("store")).unwrap();
        let document = store.import_legacy(&source_json, &assets).unwrap();
        Self {
            store,
            document,
            address: DocumentClipAddress::LegacyAudio {
                track_id: "\0鍵盤/audio".into(),
            },
            asset_id,
            original,
            asset_path,
            source_json,
            original_json,
        }
    }
    fn originals_unchanged(&self) {
        assert_eq!(fs::read(&self.asset_path).unwrap(), self.original);
        assert_eq!(self.store.asset(&self.asset_id).unwrap(), self.original);
        assert_eq!(fs::read(&self.source_json).unwrap(), self.original_json);
    }
}
fn legacy_patch() -> LegacyAudioClipPatch {
    LegacyAudioClipPatch {
        start_tick: FieldChange::Keep,
        trim_start_seconds: FieldChange::Keep,
        trim_end_seconds: FieldChange::Keep,
        fade_in_seconds: FieldChange::Keep,
        fade_out_seconds: FieldChange::Keep,
    }
}

#[test]
fn legacy_changed_window_uses_production_half_frame_rounding_and_restore_retains_exact_original() {
    let fixture = Fixture::new();
    let scene = LegacyScene::new(&fixture, 8_000, 1, &[0.5; 64]);
    let original_json = scene.document.to_json().unwrap();
    let baseline = callback_pcm(&scene.store, &scene.document, 8_000, 2);
    assert_eq!(baseline, vec![0.5; 128]);
    let patch = ClipPatch::LegacyAudio(LegacyAudioClipPatch {
        start_tick: FieldChange::Set { value: 3 }, // 12.5 frames at 120 BPM/8 kHz -> 13.
        trim_start_seconds: FieldChange::Set {
            value: decimal("0.0000625"),
        }, // .5 -> 1.
        trim_end_seconds: FieldChange::Set {
            value: decimal("0.0013125"),
        }, // 10.5 -> 11.
        fade_in_seconds: FieldChange::Set {
            value: decimal("0.0003125000"),
        }, // 2.5 -> 3, retain exact JSON spelling.
        fade_out_seconds: FieldChange::Set {
            value: decimal("0.0003125"),
        },
    });
    let edited = scene
        .document
        .update_clip(&scene.address, &patch, ClipTimelinePolicy::PreserveLoop)
        .unwrap();
    let rendered = callback_pcm(&scene.store, &edited, 8_000, 2);
    let mut expected = vec![0.0; 13 * 2];
    for gain in [
        0.0,
        1.0 / 3.0,
        2.0 / 3.0,
        1.0,
        1.0,
        1.0,
        1.0,
        2.0 / 3.0,
        1.0 / 3.0,
        0.0,
    ] {
        expected.extend_from_slice(&[0.5 * gain; 2]);
    }
    near(&rendered, &expected);
    assert_eq!(
        edited.value()["tracks"][0]["audio"]["editSource"]["original"],
        json!({})
    );
    assert_eq!(
        edited.value()["tracks"][0]["audio"]["fadeInSeconds"]
            .as_number()
            .unwrap()
            .as_str(),
        "0.0003125000"
    );
    assert_eq!(
        edited.value()["tracks"][0]["audio"]["recordingTiming"],
        scene.document.value()["tracks"][0]["audio"]["recordingTiming"]
    );
    assert_evidence(&scene.document, &edited);
    let mut extended_snapshot = edited.value().clone();
    extended_snapshot["tracks"][0]["audio"]["editSource"]["vendor"] =
        json!({"retained":[null,"opaque"]});
    let extended_snapshot = ProjectDocument::from_value(extended_snapshot).unwrap();
    let restored = extended_snapshot
        .restore_clip(&scene.address, ClipTimelinePolicy::PreserveLoop)
        .unwrap();
    assert_eq!(callback_pcm(&scene.store, &restored, 8_000, 2), baseline);
    assert_eq!(
        restored.value()["tracks"][0]["audio"]["editSource"],
        extended_snapshot.value()["tracks"][0]["audio"]["editSource"]
    );
    let mut without_snapshot = restored.value().clone();
    without_snapshot["tracks"][0]["audio"]
        .as_object_mut()
        .unwrap()
        .remove("editSource");
    assert_eq!(without_snapshot, *scene.document.value());
    assert_eq!(scene.document.to_json().unwrap(), original_json);
    scene.originals_unchanged();
}

#[test]
fn legacy_rounded_fades_can_overlap_without_silent_rescaling_and_cursor_clamps_float_output() {
    let fixture = Fixture::new();
    let scene = LegacyScene::new(&fixture, 8_000, 1, &[2.0; 64]);
    let mut patch = legacy_patch();
    patch.trim_end_seconds = FieldChange::Set {
        value: decimal("0.006125"),
    }; // 49 frames.
    patch.fade_in_seconds = FieldChange::Set {
        value: decimal("0.0030625"),
    }; // 24.5 -> 25.
    patch.fade_out_seconds = FieldChange::Set {
        value: decimal("0.0030625"),
    };
    let edited = scene
        .document
        .update_clip(
            &scene.address,
            &ClipPatch::LegacyAudio(patch),
            ClipTimelinePolicy::PreserveLoop,
        )
        .unwrap();
    let prepared = prepare(&scene.store, &edited, 8_000, 1).unwrap();
    assert_eq!(prepared.len(), 49);
    near(&prepared[24..25], &[2.0 * (24.0 / 25.0) * (24.0 / 25.0)]);
    let rendered = callback_pcm(&scene.store, &edited, 8_000, 1);
    assert_eq!(
        rendered[24], 1.0,
        "Native cursor clamps even F32 output after the raw prepared mix"
    );
    assert_eq!((rendered[0], rendered[48]), (0.0, 0.0));
    scene.originals_unchanged();
}

#[test]
fn full_content_one_shot_render_is_independent_of_bars_and_loop_extension_does_not_retime_audio() {
    let fixture = Fixture::new();
    let scene = LegacyScene::new(&fixture, 8_000, 1, &[0.25; 64]);
    let mut value = scene.document.value().clone();
    value["bars"] = 1.into();
    let base = ProjectDocument::from_value(value).unwrap();
    let mut patch = legacy_patch();
    patch.start_tick = FieldChange::Set { value: 3841 };
    let kept = base
        .update_clip(
            &scene.address,
            &ClipPatch::LegacyAudio(patch),
            ClipTimelinePolicy::PreserveLoop,
        )
        .unwrap();
    assert_eq!(kept.bars(), 1);
    let rendered = callback_pcm(&scene.store, &kept, 8_000, 1);
    assert_eq!(rendered.len(), 16_004 + 64);
    assert!(rendered[..16_004].iter().all(|v| *v == 0.0));
    assert!(rendered[16_004..].iter().all(|v| *v == 0.25));
    let extended = kept
        .update_clip(
            &scene.address,
            &ClipPatch::LegacyAudio(legacy_patch()),
            ClipTimelinePolicy::ExtendLoop {
                allow_native_schema_upgrade: false,
            },
        )
        .unwrap();
    assert_eq!(extended.bars(), 2);
    assert_eq!(callback_pcm(&scene.store, &extended, 8_000, 1), rendered);
    assert_evidence(&base, &extended);
    scene.originals_unchanged();
}

enum Segment {
    Audio(Vec<f32>),
    Gap(u64),
}
struct NativeScene {
    store: NativeStore,
    document: ProjectDocument,
    address: DocumentClipAddress,
    original_path: PathBuf,
    owned_path: PathBuf,
    original: Vec<u8>,
    owned: Vec<u8>,
}
impl NativeScene {
    fn new(fixture: &Fixture, channels: u16, layout: PcmLayout, segments: &[Segment]) -> Self {
        let rate = 48_000;
        let initial = 100;
        let epoch = ClockEpochId(uuid::Uuid::new_v4().to_string());
        let descriptor = TakeDescriptor {
            version: 1,
            take_id: TakeId(uuid::Uuid::new_v4().to_string()),
            streams: vec![StreamDescriptor {
                stream_id: "audio".into(),
                segment_id: "synthetic-segment".into(),
                source_id: "synthetic-source".into(),
                source_name: "Device-free clip fixture".into(),
                source_connection_generation: 1,
                host_clock: HostClock {
                    domain: ClockDomainId("synthetic-clock".into()),
                    epoch: epoch.clone(),
                    ticks_per_second_numerator: 1_000_000_000,
                    ticks_per_second_denominator: 1,
                },
                kind: StreamKind::Audio {
                    format: PcmFormat {
                        sample_rate: rate,
                        channels,
                        encoding: PcmEncoding::F32Le,
                        layout,
                    },
                    audio_epoch: epoch,
                },
                initial_source_position: initial,
            }],
            provenance: Default::default(),
        };
        let original_path = fixture.0.join("original.sqlite3");
        let mut journal = Journal::create(&original_path, &descriptor).unwrap();
        let stream = &descriptor.streams[0];
        let mut start = initial;
        for (sequence, segment) in segments.iter().enumerate() {
            let (count, payload, kind) = match segment {
                Segment::Audio(samples) => {
                    assert!(samples.len().is_multiple_of(usize::from(channels)));
                    (
                        samples.len() as u64 / u64::from(channels),
                        samples
                            .iter()
                            .flat_map(|s| s.to_le_bytes())
                            .collect::<Vec<_>>(),
                        ChunkKind::Content,
                    )
                }
                Segment::Gap(count) => (
                    *count,
                    vec![],
                    ChunkKind::Gap {
                        reason: GapReason::TransferOverflow,
                    },
                ),
            };
            let stamp = |position| HostTimestamp {
                domain: stream.host_clock.domain.clone(),
                epoch: stream.host_clock.epoch.clone(),
                ticks: position * 1000,
            };
            let header = ChunkHeader {
                version: 1,
                take_id: descriptor.take_id.clone(),
                stream_id: stream.stream_id.clone(),
                segment_id: stream.segment_id.clone(),
                sequence: sequence as u64,
                range: SourceRange::AudioFrames { start, count },
                interpretation: stream.kind.clone(),
                observed_start: stamp(start),
                observed_end: stamp(start + count),
                clock_anchors: vec![],
                kind,
            };
            journal.append(&header, &payload).unwrap();
            start += count;
        }
        let manifest = journal.seal("seal-synthetic-render").unwrap();
        drop(journal);
        let original = fs::read(&original_path).unwrap();
        let sealed = SealedJournal::open(&original_path).unwrap();
        let store_root = fixture.0.join("store");
        let mut store = NativeStore::open(&store_root).unwrap();
        let object = store.ingest_take(&sealed).unwrap();
        drop(sealed);
        let owned_path = store_root
            .join("takes")
            .join(format!("{}.sqlite3", object.object_sha256));
        let owned = fs::read(&owned_path).unwrap();
        let mut document = ProjectDocument::new("Synthetic native clip render")
            .unwrap()
            .upgrade_native()
            .unwrap();
        document
            .append_take_clip(
                "recording-track",
                "Synthetic audio",
                TakeClip {
                    id: "clip".into(),
                    source: TakeReference {
                        take_id: descriptor.take_id,
                        manifest_sha256: manifest.sha256,
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: initial,
                        count: start - initial,
                    },
                    position: TimelinePosition {
                        frame: 0,
                        sample_rate: rate,
                    },
                    fades: None,
                    edit_source: None,
                },
            )
            .unwrap();
        let mut value = document.value().clone();
        value["tempo"] = 120.into();
        value["tracks"][2]["gain"] = 1.0.into();
        value["tracks"][2]["notes"] = json!([null,{"opaque":"not musical notes"}]);
        retain_non_audio_evidence(&mut value);
        let document = store
            .create(&ProjectDocument::from_value(value).unwrap())
            .unwrap();
        Self {
            store,
            document,
            address: DocumentClipAddress::NativeAudio {
                track_id: "recording-track".into(),
                clip_id: "clip".into(),
            },
            original_path,
            owned_path,
            original,
            owned,
        }
    }
    fn originals_unchanged(&self) {
        assert_eq!(fs::read(&self.original_path).unwrap(), self.original);
        assert_eq!(fs::read(&self.owned_path).unwrap(), self.owned);
    }
}
fn native_patch() -> NativeAudioClipPatch {
    NativeAudioClipPatch {
        position_frame: ValueChange::Keep,
        source_window: ValueChange::Keep,
        fades: FieldChange::Keep,
    }
}

#[test]
fn native_exact_source_window_crosses_chunks_signed_placement_crops_without_restarting_fade_and_restores()
 {
    let fixture = Fixture::new();
    let samples = [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0];
    let scene = NativeScene::new(
        &fixture,
        1,
        PcmLayout::Interleaved,
        &[
            Segment::Audio(samples[..4].to_vec()),
            Segment::Audio(samples[4..].to_vec()),
        ],
    );
    let original_json = scene.document.to_json().unwrap();
    let baseline = callback_pcm(&scene.store, &scene.document, 48_000, 2);
    let expected: Vec<_> = samples.iter().flat_map(|v| [*v, *v]).collect();
    assert_eq!(baseline, expected);
    let patch = ClipPatch::NativeAudio(NativeAudioClipPatch {
        position_frame: ValueChange::Set { value: -2 },
        source_window: ValueChange::Set {
            value: AudioSourceWindow {
                start_frame: 101,
                frame_count: 6,
            },
        },
        fades: FieldChange::Set {
            value: AudioFades {
                in_frames: 0,
                out_frames: 2,
                curve: FadeCurve::Linear,
            },
        },
    });
    let edited = scene
        .document
        .update_clip(&scene.address, &patch, ClipTimelinePolicy::PreserveLoop)
        .unwrap();
    assert_eq!(
        callback_pcm(&scene.store, &edited, 48_000, 2),
        vec![0.5, 0.5, 0.625, 0.625, 0.375, 0.375, 0.0, 0.0]
    );
    assert_eq!(edited.value()["tracks"][2]["recording"]["version"], 2);
    let current = edited.take_clips().unwrap().pop().unwrap();
    assert_eq!(
        current.original_view().unwrap().unwrap(),
        scene.document.take_clips().unwrap().pop().unwrap()
    );
    let restored = edited
        .restore_clip(&scene.address, ClipTimelinePolicy::PreserveLoop)
        .unwrap();
    assert_eq!(callback_pcm(&scene.store, &restored, 48_000, 2), baseline);
    assert_eq!(
        restored.value()["tracks"][2]["recording"]["clips"][0]["editSource"],
        edited.value()["tracks"][2]["recording"]["clips"][0]["editSource"]
    );
    assert_evidence(&scene.document, &restored);
    assert_eq!(
        restored.value()["tracks"][2]["notes"],
        scene.document.value()["tracks"][2]["notes"]
    );
    assert_eq!(scene.document.to_json().unwrap(), original_json);
    scene.originals_unchanged();
}

#[test]
fn native_equal_power_fade_and_integer_callback_conversion_share_the_current_endpoint_law() {
    let fixture = Fixture::new();
    let scene = NativeScene::new(
        &fixture,
        1,
        PcmLayout::Interleaved,
        &[Segment::Audio(vec![1.0; 8])],
    );
    let mut patch = native_patch();
    patch.fades = FieldChange::Set {
        value: AudioFades {
            in_frames: 2,
            out_frames: 2,
            curve: FadeCurve::EqualPower,
        },
    };
    let edited = scene
        .document
        .update_clip(
            &scene.address,
            &ClipPatch::NativeAudio(patch),
            ClipTimelinePolicy::PreserveLoop,
        )
        .unwrap();
    let root_half = std::f32::consts::FRAC_1_SQRT_2;
    near(
        &callback_pcm(&scene.store, &edited, 48_000, 1),
        &[0.0, root_half, 1.0, 1.0, 1.0, 1.0, root_half, 0.0],
    );
    let (mut cursor, _) =
        RenderCursor::new(prepare(&scene.store, &edited, 48_000, 1).unwrap(), 1).unwrap();
    let mut integer = [0i16; 10];
    cursor.render_i16(&mut integer);
    assert_eq!(
        integer,
        [0, 23170, 32767, 32767, 32767, 32767, 23170, 0, 0, 0]
    );
    let restored = edited
        .restore_clip(&scene.address, ClipTimelinePolicy::PreserveLoop)
        .unwrap();
    assert_eq!(
        callback_pcm(&scene.store, &restored, 48_000, 1),
        vec![1.0; 8]
    );
    scene.originals_unchanged();
}

#[test]
fn native_planar_channel_order_survives_exact_window_edit_and_channel_conversion_rejects() {
    let fixture = Fixture::new();
    let scene = NativeScene::new(
        &fixture,
        2,
        PcmLayout::Planar,
        &[Segment::Audio(vec![
            0.125, 0.25, 0.5, 0.75, -0.125, -0.25, -0.5, -0.75,
        ])],
    );
    let mut patch = native_patch();
    patch.source_window = ValueChange::Set {
        value: AudioSourceWindow {
            start_frame: 101,
            frame_count: 2,
        },
    };
    let edited = scene
        .document
        .update_clip(
            &scene.address,
            &ClipPatch::NativeAudio(patch),
            ClipTimelinePolicy::PreserveLoop,
        )
        .unwrap();
    assert_eq!(
        callback_pcm(&scene.store, &edited, 48_000, 2),
        vec![0.25, -0.25, 0.5, -0.5]
    );
    assert!(
        prepare(&scene.store, &edited, 48_000, 1)
            .unwrap_err()
            .contains("Channel conversion")
    );
    assert!(
        prepare(&scene.store, &edited, 44_100, 2)
            .unwrap_err()
            .contains("rate")
    );
    scene.originals_unchanged();
}

#[test]
fn selected_source_gap_rejects_even_when_preroll_cropped_but_explicit_clean_window_can_render() {
    let fixture = Fixture::new();
    let scene = NativeScene::new(
        &fixture,
        1,
        PcmLayout::Interleaved,
        &[
            Segment::Audio(vec![0.25; 4]),
            Segment::Gap(2),
            Segment::Audio(vec![0.5; 4]),
        ],
    );
    assert!(
        prepare(&scene.store, &scene.document, 48_000, 1)
            .unwrap_err()
            .contains("source loss")
    );
    let mut patch = native_patch();
    patch.position_frame = ValueChange::Set { value: -6 };
    let preroll = scene
        .document
        .update_clip(
            &scene.address,
            &ClipPatch::NativeAudio(patch),
            ClipTimelinePolicy::PreserveLoop,
        )
        .unwrap();
    assert!(
        prepare(&scene.store, &preroll, 48_000, 1)
            .unwrap_err()
            .contains("source loss"),
        "Cropping a gap before frame0 cannot turn an incomplete selected source into successful audio"
    );
    let mut patch = native_patch();
    patch.position_frame = ValueChange::Set { value: 0 };
    patch.source_window = ValueChange::Set {
        value: AudioSourceWindow {
            start_frame: 106,
            frame_count: 4,
        },
    };
    let clean = preroll
        .update_clip(
            &scene.address,
            &ClipPatch::NativeAudio(patch),
            ClipTimelinePolicy::PreserveLoop,
        )
        .unwrap();
    assert_eq!(callback_pcm(&scene.store, &clean, 48_000, 1), vec![0.5; 4]);
    let restored = clean
        .restore_clip(&scene.address, ClipTimelinePolicy::PreserveLoop)
        .unwrap();
    assert!(
        prepare(&scene.store, &restored, 48_000, 1)
            .unwrap_err()
            .contains("source loss")
    );
    scene.originals_unchanged();
}

#[test]
fn semantic_legacy_edit_does_not_claim_decoder_rate_support_or_mutate_original_on_render_failure() {
    let fixture = Fixture::new();
    let scene = LegacyScene::new(&fixture, 8_000, 1, &[0.5; 64]);
    let original_json = scene.document.to_json().unwrap();
    let mut patch = legacy_patch();
    patch.start_tick = FieldChange::Set { value: 1 };
    let edited = scene
        .document
        .update_clip(
            &scene.address,
            &ClipPatch::LegacyAudio(patch),
            ClipTimelinePolicy::PreserveLoop,
        )
        .unwrap();
    let before = edited.to_json().unwrap();
    assert!(
        prepare(&scene.store, &edited, 48_000, 1)
            .unwrap_err()
            .contains("rate")
    );
    assert_eq!(edited.to_json().unwrap(), before);
    assert_eq!(scene.document.to_json().unwrap(), original_json);
    assert_evidence(&scene.document, &edited);
    scene.originals_unchanged();
}

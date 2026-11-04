//! Device-free owned v1/v2 codec checks. No journal, stream or normal seal.
use omniusong_core::contracts::{
    CAPTURE_MAX_BYTES, CapturedEvent, ClockEpochId, EndpointId, Envelope, MusicalEvent, RawPacket,
    TakeId,
};
use omniusong_engine::{
    LiveBatch, LiveClock, LiveEvent, LiveEventKind, LiveFinishFence, LiveFinishPolicy, LiveId,
    LivePatch, LiveScope, LiveSourceBinding, LiveSourceKind,
    performance_records::{
        LocalRecord, PERFORMANCE_NAMESPACE, PERFORMANCE_NAMESPACE_V2, PerformanceCheckpoint,
        PerformanceChunkIdentity, PerformanceConfig, PerformanceProfile, PerformanceRecord,
        PerformanceSource, RECORD_VERSION, STREAM_IDS, checkpoint_sha256, create_descriptor,
        create_descriptor_for, decode_envelope, decode_record, descriptor_profile, encode_record,
        from_descriptor,
    },
    performance_tap::{
        BatchDisposition, TapBegin, TapClear, TapClearReason, TapFinish, TapInfo, TapTransition,
        TapTransitionKind,
    },
};
use omniusong_recording::{SourcePosition, StreamPrefix};
use serde_json::json;
use sha2::{Digest, Sha256};

fn config() -> PerformanceConfig {
    let kinds = [
        LiveSourceKind::ComputerKeys,
        LiveSourceKind::PointerKeys,
        LiveSourceKind::Pads,
        LiveSourceKind::AccessiblePulse,
    ];
    PerformanceConfig {
        take_id: TakeId("synthetic-performance-writer-take-v1".into()),
        info: TapInfo {
            scope: LiveScope {
                worker_generation: LiveId([0x31; 16]),
                session_id: LiveId([0x42; 16]),
                panic_generation: 1,
            },
            sample_rate: 48_000,
            channels: 8,
        },
        gui_clock: LiveClock {
            domain: "synthetic-gui-observation".into(),
            epoch: "synthetic-gui-epoch-v1".into(),
            ticks_per_second_numerator: 1_000_000_000_000,
            ticks_per_second_denominator: 1_000,
        },
        output_epoch: ClockEpochId("synthetic-performance-output-epoch-v1".into()),
        sources: std::array::from_fn(|index| PerformanceSource {
            binding: LiveSourceBinding {
                kind: kinds[index],
                handle: (index + 1) as u8,
                connection_generation: 1,
            },
            endpoint: EndpointId(format!("synthetic-performance-source-{}", index + 1)),
        }),
    }
}
fn local(profile: PerformanceProfile, record: PerformanceRecord) -> LocalRecord {
    LocalRecord {
        version: profile.version(),
        record,
    }
}
fn begin(config: &PerformanceConfig, profile: PerformanceProfile) -> LocalRecord {
    local(
        profile,
        PerformanceRecord::Begin {
            callback_index: 1,
            begin: TapBegin {
                scope: config.info.scope,
                start_frame: 0,
                frames: 16_384,
                sample_rate: 48_000,
                channels: 8,
            },
        },
    )
}
fn finish(config: &PerformanceConfig) -> LocalRecord {
    local(
        PerformanceProfile::FinishV2,
        PerformanceRecord::Finish {
            callback_index: 7,
            finish: TapFinish {
                fence: LiveFinishFence {
                    id: LiveId([0x53; 16]),
                    scope: config.info.scope,
                    accepted_sequence: Some(17),
                },
                callback_index: 7,
                cut_frame: 16_384,
                voices_cut: 64,
                release_tails_cut: 32,
                policy: LiveFinishPolicy::CutAtCompleteCallbackEnd,
            },
        },
    )
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn checkpoint(config: &PerformanceConfig, maximum: bool) -> PerformanceCheckpoint {
    let audio_end = if maximum { i64::MAX as u64 } else { 16_384 };
    PerformanceCheckpoint {
        group_sequence: if maximum { u64::MAX } else { 1 },
        previous_checkpoint_sha256: maximum.then(|| "f".repeat(64)),
        first_callback_index: if maximum { u64::MAX - 2 } else { 1 },
        last_callback_index: if maximum { u64::MAX - 1 } else { 1 },
        audio_end,
        scope_after: config.info.scope,
        chunks: (0..if maximum { 63 } else { 1 })
            .map(|index| PerformanceChunkIdentity {
                stream_id: STREAM_IDS[index % 6].into(),
                sequence: if maximum {
                    i64::MAX as u64 - 63 + index as u64
                } else {
                    0
                },
                sha256: "f".repeat(64),
            })
            .collect(),
        prefixes: STREAM_IDS
            .iter()
            .enumerate()
            .map(|(index, stream)| {
                let count = if index == 0 {
                    audio_end
                } else if maximum {
                    i64::MAX as u64
                } else {
                    1
                };
                StreamPrefix {
                    stream_id: (*stream).into(),
                    next_sequence: if maximum { i64::MAX as u64 } else { 1 },
                    source_end: if index == 0 {
                        SourcePosition::AudioFrames(count)
                    } else {
                        SourcePosition::Events(count)
                    },
                    recorded_units: count,
                    gap_units: 0,
                    gap_chunks: 0,
                }
            })
            .collect(),
    }
}

#[test]
fn original_v1_matches_frozen_pre_finish_descriptor_and_envelope_bytes() {
    // Independent golden hashes read from the completed v1-only synthetic
    // performance-writer qualifier before this codec gained profile support.
    // These constants require no runtime fixture, file, database or device.
    let config = config();
    let descriptor = create_descriptor(&config).unwrap();
    assert_eq!(RECORD_VERSION, 1);
    assert_eq!(
        descriptor_profile(&descriptor).unwrap(),
        PerformanceProfile::OriginalV1
    );
    assert_eq!(
        descriptor.sha256().unwrap(),
        "ba3291cc8821283af70aec8f7ba03c4a45bce415409561bd7e199a13ecde6262"
    );
    assert_eq!(
        sha(&serde_json::to_vec(&descriptor).unwrap()),
        "36794020ee84e9786d3e6bc813df8d742459573facf94fffdf7e8978df102fff"
    );
    let record = begin(&config, PerformanceProfile::OriginalV1);
    let encoded = encode_record(&config, 5, &record, 0, 0).unwrap();
    assert_eq!(encoded.len(), 941);
    assert_eq!(
        sha(&encoded),
        "8cbc21808cce7acc76189073e217f4ea81a955f91837e3b45af2a6342ce22bd9"
    );
    assert_eq!(decode_record(&encoded).unwrap(), record);
}

#[test]
fn explicit_profiles_regenerate_exact_descriptors_without_clock_or_stream_migration() {
    let config = config();
    let v1 = create_descriptor(&config).unwrap();
    let v2 = create_descriptor_for(&config, PerformanceProfile::FinishV2).unwrap();
    assert_eq!(v1.streams, v2.streams);
    assert_ne!(v1.sha256().unwrap(), v2.sha256().unwrap());
    for (profile, descriptor) in [
        (PerformanceProfile::OriginalV1, v1),
        (PerformanceProfile::FinishV2, v2),
    ] {
        assert_eq!(descriptor.provenance.len(), 1);
        assert_eq!(
            descriptor.provenance[profile.namespace()]["version"],
            profile.version()
        );
        assert_eq!(descriptor_profile(&descriptor).unwrap(), profile);
        assert_eq!(from_descriptor(&descriptor).unwrap(), config);
        assert_eq!(
            PerformanceConfig::from_descriptor(&descriptor).unwrap(),
            config
        );
        assert_eq!(
            descriptor.streams[1].host_clock.ticks_per_second_numerator,
            1_000_000_000
        );
        assert_eq!(
            descriptor.streams[1]
                .host_clock
                .ticks_per_second_denominator,
            1
        );
        assert_eq!(
            descriptor.provenance[profile.namespace()]["config"]["gui_clock"]["ticks_per_second_denominator"],
            1_000
        );
    }
}

#[test]
fn generic_valid_descriptor_hashes_cannot_hide_mixed_future_or_mismatched_owned_profiles() {
    for profile in [PerformanceProfile::OriginalV1, PerformanceProfile::FinishV2] {
        for changed in 0..8 {
            let mut descriptor = create_descriptor_for(&config(), profile).unwrap();
            let namespace = profile.namespace();
            match changed {
                0 => {
                    descriptor.provenance.insert(
                        if profile == PerformanceProfile::OriginalV1 {
                            PERFORMANCE_NAMESPACE_V2
                        } else {
                            PERFORMANCE_NAMESPACE
                        }
                        .into(),
                        json!(null),
                    );
                }
                1 => {
                    descriptor.provenance.get_mut(namespace).unwrap()["version"] =
                        json!(if profile.version() == 1 { 2 } else { 1 });
                }
                2 => {
                    descriptor.provenance.insert(namespace.into(), json!(null));
                }
                3 => {
                    let value = descriptor.provenance.remove(namespace).unwrap();
                    descriptor
                        .provenance
                        .insert("omniusong.performance.v3".into(), value);
                }
                4 => {
                    descriptor.provenance.get_mut(namespace).unwrap()["future"] = json!(true);
                }
                5 => {
                    descriptor.provenance.get_mut(namespace).unwrap()["config"]["future"] =
                        json!(true);
                }
                6 => {
                    descriptor.provenance.remove(namespace);
                }
                _ => descriptor.streams[5]
                    .source_name
                    .push_str(" rehashed change"),
            }
            // The recording layer can authenticate these exact generic bytes;
            // owned-format validation must still reject their interpretation.
            assert!(descriptor.sha256().is_ok(), "generic hash case {changed}");
            assert!(
                descriptor_profile(&descriptor).is_err(),
                "profile {} case {changed}",
                profile.version()
            );
            assert!(from_descriptor(&descriptor).is_err());
        }
    }
}

#[test]
fn finish_is_v2_control_only_with_exact_render_cut_and_no_musical_release_invention() {
    let config = config();
    let record = finish(&config);
    let encoded = encode_record(&config, 5, &record, 16_384, 11).unwrap();
    let (envelope, decoded) = decode_envelope(&encoded).unwrap();
    assert_eq!(decoded, record);
    assert!(encoded.len() <= CAPTURE_MAX_BYTES);
    assert_eq!(envelope.sequence, 11);
    assert_eq!(
        envelope.payload.source.endpoint.0,
        create_descriptor_for(&config, PerformanceProfile::FinishV2)
            .unwrap()
            .streams[5]
            .source_id
    );
    assert_eq!(envelope.payload.time.observed.epoch, config.output_epoch);
    assert_eq!(envelope.payload.time.observed.ticks, 16_384);
    assert_eq!(envelope.payload.time.alignment, None);
    assert_eq!(envelope.payload.source.channel, None);
    assert!(matches!(envelope.payload.raw, RawPacket::Local));
    assert!(matches!(
        envelope.payload.musical,
        MusicalEvent::Uninterpreted
    ));
    assert!(
        envelope
            .payload
            .extensions
            .contains_key(PERFORMANCE_NAMESPACE_V2)
    );
    assert!(
        !envelope
            .payload
            .extensions
            .contains_key(PERFORMANCE_NAMESPACE)
    );
    for stream in [0, 1, 2, 3, 4, 6] {
        assert!(encode_record(&config, stream, &record, 16_384, 11).is_err());
    }
    assert!(encode_record(&config, 5, &record, 16_383, 11).is_err());
    let mut old = record.clone();
    old.version = 1;
    assert!(old.validate().is_err());
    assert!(encode_record(&config, 5, &old, 16_384, 11).is_err());
    for ticks in [0, 16_385] {
        let mut changed = envelope.clone();
        changed.payload.time.observed.ticks = ticks;
        let bytes = changed.encode().unwrap();
        assert!(decode_record(&bytes).is_err());
    }
    let mut changed = envelope;
    changed.payload.time.observed.domain.0 = "other-clock".into();
    assert!(decode_record(&changed.encode().unwrap()).is_err());
}

#[test]
fn finish_shape_bounds_reject_invalid_ids_scopes_counts_and_versions() {
    for changed in 0..9 {
        let mut record = finish(&config());
        let PerformanceRecord::Finish {
            callback_index,
            finish,
        } = &mut record.record
        else {
            unreachable!()
        };
        match changed {
            0 => *callback_index = 0,
            1 => finish.callback_index += 1,
            2 => finish.fence.id = LiveId([0; 16]),
            3 => finish.fence.scope.panic_generation = 0,
            4 => finish.fence.accepted_sequence = Some(0),
            5 => finish.cut_frame = 0,
            6 => finish.cut_frame = i64::MAX as u64 + 1,
            7 => finish.voices_cut = 65,
            _ => finish.release_tails_cut = 65,
        }
        assert!(record.validate().is_err(), "shape case {changed}");
    }
    let mut empty = finish(&config());
    let PerformanceRecord::Finish { finish, .. } = &mut empty.record else {
        unreachable!()
    };
    finish.fence.accepted_sequence = None;
    finish.voices_cut = 0;
    finish.release_tails_cut = 0;
    assert!(empty.validate().is_ok());
    for version in [0, 3, u16::MAX] {
        let mut record = empty.clone();
        record.version = version;
        assert!(record.validate().is_err());
    }
}

#[test]
fn generic_valid_envelopes_reject_mixed_misplaced_future_and_unknown_owned_fields() {
    let bytes = encode_record(&config(), 5, &finish(&config()), 16_384, 11).unwrap();
    for changed in 0..12 {
        let mut envelope = Envelope::<CapturedEvent>::decode(&bytes).unwrap();
        let namespace = PERFORMANCE_NAMESPACE_V2;
        match changed {
            0 => {
                envelope
                    .payload
                    .extensions
                    .insert(PERFORMANCE_NAMESPACE.into(), json!(null));
            }
            1 => envelope.payload.extensions.get_mut(namespace).unwrap()["version"] = json!(1),
            2 => envelope.payload.extensions.get_mut(namespace).unwrap()["version"] = json!(3),
            3 => {
                let value = envelope.payload.extensions.remove(namespace).unwrap();
                envelope
                    .payload
                    .extensions
                    .insert("omniusong.performance.v3".into(), value);
            }
            4 => envelope.payload.extensions.get_mut(namespace).unwrap()["future"] = json!(true),
            5 => {
                envelope.payload.extensions.get_mut(namespace).unwrap()["record"]["future"] =
                    json!(true)
            }
            6 => {
                envelope.payload.extensions.get_mut(namespace).unwrap()["record"]["finish"]["future"] =
                    json!(true)
            }
            7 => {
                envelope.payload.extensions.get_mut(namespace).unwrap()["record"]["finish"]["fence"]
                    ["future"] = json!(true)
            }
            8 => {
                envelope.payload.extensions.get_mut(namespace).unwrap()["record"]["finish"]["policy"] =
                    json!({"cut_at_complete_callback_end":{"future":true}})
            }
            9 => {
                envelope
                    .extensions
                    .insert(PERFORMANCE_NAMESPACE.into(), json!(null));
            }
            10 => {
                envelope
                    .payload
                    .source
                    .extensions
                    .insert(PERFORMANCE_NAMESPACE_V2.into(), json!(null));
            }
            _ => {
                envelope
                    .payload
                    .time
                    .extensions
                    .insert("omniusong.performance.v3".into(), json!(null));
            }
        }
        let generic_valid_bytes = envelope.encode().unwrap();
        assert!(
            decode_envelope(&generic_valid_bytes).is_err(),
            "owned envelope case {changed}"
        );
    }
    // A v2 marker cannot upgrade a v1 record, and the old marker cannot disguise
    // a v2 record, even when no second marker is left behind.
    for profile in [PerformanceProfile::OriginalV1, PerformanceProfile::FinishV2] {
        let original = encode_record(&config(), 5, &begin(&config(), profile), 0, 0).unwrap();
        let mut envelope = Envelope::<CapturedEvent>::decode(&original).unwrap();
        let value = envelope
            .payload
            .extensions
            .remove(profile.namespace())
            .unwrap();
        envelope.payload.extensions.insert(
            if profile == PerformanceProfile::OriginalV1 {
                PERFORMANCE_NAMESPACE_V2
            } else {
                PERFORMANCE_NAMESPACE
            }
            .into(),
            value,
        );
        assert!(decode_record(&envelope.encode().unwrap()).is_err());
    }
}

#[test]
fn v2_all_records_keep_full_event_values_and_split_control_evidence_within_core_caps() {
    let config = config();
    let profile = PerformanceProfile::FinishV2;
    let mut batch = LiveBatch::empty(config.info.scope, u64::MAX, Some(LiveId([9; 16])));
    for index in 0..32 {
        batch
            .try_push(LiveEvent {
                source: (index % 4 + 1) as u8,
                connection_generation: 1,
                observed_ticks: u64::MAX,
                event: LiveEventKind::Pulse {
                    note: u64::MAX - index,
                    voice: u64::MAX - index,
                    key: 127,
                    velocity16: if index == 0 { 0 } else { u16::MAX },
                    release_velocity16: u16::MAX,
                    patch: LivePatch::Drums,
                    duration_frames: 24_000,
                },
            })
            .unwrap();
    }
    let records = [
        (begin(&config, profile), 0),
        (
            local(
                profile,
                PerformanceRecord::Batch {
                    callback_index: 1,
                    packet_ordinal: 4,
                    batch: Box::new(batch),
                    disposition: BatchDisposition::Applied,
                },
            ),
            0,
        ),
        (
            local(
                profile,
                PerformanceRecord::Clear {
                    callback_index: 1,
                    clear: TapClear {
                        scope: config.info.scope,
                        frame: 16_384,
                        reason: TapClearReason::FaultPending,
                    },
                },
            ),
            16_384,
        ),
        (
            local(
                profile,
                PerformanceRecord::Transition {
                    callback_index: 1,
                    transition: TapTransition {
                        scope: config.info.scope,
                        source: 4,
                        voice: u64::MAX,
                        frame: 16_383,
                        kind: TapTransitionKind::AutoRelease {
                            release_velocity16: u16::MAX,
                        },
                    },
                },
            ),
            16_383,
        ),
        (finish(&config), 16_384),
        (
            local(
                profile,
                PerformanceRecord::End {
                    callback_index: 7,
                    end_frame: 16_384,
                    scope_after: config.info.scope,
                    applied_sequence: Some(17),
                    active_voices: 0,
                    release_tail_voices: 0,
                    clear_count: 2,
                    transition_count: 128,
                    first_fault: None,
                },
            ),
            16_384,
        ),
    ];
    for (record, ticks) in records {
        let bytes = encode_record(&config, 5, &record, ticks, 1).unwrap();
        assert!(bytes.len() <= CAPTURE_MAX_BYTES);
        assert_eq!(decode_record(&bytes).unwrap(), record);
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            json["payload"][PERFORMANCE_NAMESPACE_V2]["record"]
                .get("transitions")
                .is_none()
        );
    }
    for source in 1..=4 {
        let original = LiveEvent {
            source,
            connection_generation: 1,
            observed_ticks: u64::MAX,
            event: LiveEventKind::NoteOff {
                voice: u64::MAX,
                release_velocity16: u16::MAX,
            },
        };
        let record = local(
            profile,
            PerformanceRecord::AppliedEvent {
                callback_index: 1,
                batch_ordinal: 4,
                event_index: 31,
                scope: config.info.scope,
                batch_sequence: u64::MAX,
                applied_frame: 17,
                original,
            },
        );
        let bytes = encode_record(
            &config,
            usize::from(source),
            &record,
            u64::MAX,
            i64::MAX as u64 - 1,
        )
        .unwrap();
        let (envelope, decoded) = decode_envelope(&bytes).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(envelope.payload.time.observed.ticks, u64::MAX);
        assert_eq!(
            envelope.payload.time.observed.domain.0,
            config.gui_clock.domain
        );
    }
}

#[test]
fn checkpoints_keep_v1_domain_and_separate_v2_with_maximum_member_envelopes() {
    let config = config();
    for profile in [PerformanceProfile::OriginalV1, PerformanceProfile::FinishV2] {
        let record = local(
            profile,
            PerformanceRecord::Checkpoint {
                checkpoint: checkpoint(&config, true),
            },
        );
        let encoded =
            encode_record(&config, 5, &record, i64::MAX as u64, i64::MAX as u64 - 1).unwrap();
        assert!(encoded.len() <= CAPTURE_MAX_BYTES);
        assert_eq!(decode_record(&encoded).unwrap(), record);
        let domain: &[u8] = if profile == PerformanceProfile::OriginalV1 {
            b"OmniuSong.performance-checkpoint.v1\0"
        } else {
            b"OmniuSong.performance-checkpoint.v2\0"
        };
        let other: &[u8] = if profile == PerformanceProfile::OriginalV1 {
            b"OmniuSong.performance-checkpoint.v2\0"
        } else {
            b"OmniuSong.performance-checkpoint.v1\0"
        };
        let mut expected = Sha256::new();
        expected.update(domain);
        expected.update(&encoded);
        let actual = checkpoint_sha256(&encoded).unwrap();
        assert_eq!(actual, format!("{:x}", expected.finalize()));
        let mut wrong = Sha256::new();
        wrong.update(other);
        wrong.update(&encoded);
        assert_ne!(actual, format!("{:x}", wrong.finalize()));
        let mut whitespace = vec![b' '];
        whitespace.extend_from_slice(&encoded);
        assert_ne!(actual, checkpoint_sha256(&whitespace).unwrap());
    }
    assert!(
        checkpoint_sha256(&encode_record(&config, 5, &finish(&config), 16_384, 1).unwrap())
            .is_err()
    );
}

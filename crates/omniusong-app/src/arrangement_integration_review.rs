//! Independent local arrangement admission tests. Synthetic documents and a
//! bounded injected session peer only: no source files, devices or real catalog.
use super::*;
use crate::clip_ui::{ClipDraftFields, ClipSelection, FieldMode, ValueMode};
use omniusong_core::{
    ClipPatch, DocumentClipAddress, DocumentNoteAddress, FieldChange, ValueChange,
    native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_session::{Request, ServiceError, TestPeer};
use serde_json::json;

const LEGACY: &str = "\0🎛/source";
const NATIVE: &str = "native-source";

fn document() -> ProjectDocument {
    let mut value = ProjectDocument::new("Independent arrangement admission")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["id"] = "notes".into();
    value["tracks"][0]["notes"] = json!([
        {"id":"","pitch":60,"startTick":17,"durationTicks":961,"velocity16":32769,"expression":null},
        {"id":"same-pitch","pitch":60,"startTick":17,"durationTicks":961,"velocity16":1,"expression":[]}
    ]);
    value["tracks"][1]["id"] = LEGACY.into();
    value["tracks"][1]["kind"] = "audio".into();
    value["tracks"][1]["assetId"] = format!("{}.wav", "a".repeat(64)).into();
    value["tracks"][1]["sourceAssetId"] = format!("{}.wav", "b".repeat(64)).into();
    value["tracks"][1]["audio"] = serde_json::from_str(
        r#"{"origin":"import","mimeType":"audio/wav","durationSeconds":2.0,"byteLength":100,"trimStartSeconds":0.123456789012345678901234567890123456789,"fadeInSeconds":0.0000,"vendor":{"recording":null,"opaque":123456789012345678901234567890123456789}}"#,
    ).unwrap();
    value["tracks"][1]["notes"] = json!([
        {"id":"retained-audio-note","pitch":72,"startTick":3,"durationTicks":7,"velocity16":60001,"expression":null}
    ]);
    let mut doc = ProjectDocument::from_value(value)
        .unwrap()
        .upgrade_native()
        .unwrap();
    for (id, range, position) in [
        (
            "audio",
            TakeRange::AudioFrames {
                start: 9_007_199_254_740_997,
                count: 128,
            },
            -9_007_199_254_740_993,
        ),
        (
            "events",
            TakeRange::Events {
                start: 7,
                count: 13,
            },
            17,
        ),
    ] {
        doc.append_take_clip(
            NATIVE,
            "Repeated name",
            TakeClip {
                id: id.into(),
                source: TakeReference {
                    take_id: omniusong_core::contracts::TakeId("synthetic-original".into()),
                    manifest_sha256: "c".repeat(64),
                    stream_id: id.into(),
                },
                range,
                position: TimelinePosition {
                    frame: position,
                    sample_rate: 48_000,
                },
                fades: None,
                edit_source: None,
            },
        )
        .unwrap();
    }
    let mut value = doc.value().clone();
    value["tracks"][2]["recording"]["clips"][0]["fades"] = Value::Null;
    value["tracks"][2]["notes"] = json!([{"uninterpreted":true}]);
    value["tracks"][2]["vendor"] = json!({"takeId":"opaque-original","recording":null});
    let mut locked = value["tracks"][1].clone();
    locked["id"] = "locked".into();
    locked["locked"] = true.into();
    value["tracks"].as_array_mut().unwrap().push(locked);
    ProjectDocument::from_value(value).unwrap()
}

fn legacy(doc: &ProjectDocument) -> ClipSelection {
    ClipSelection {
        project_id: doc.id().into(),
        address: DocumentClipAddress::LegacyAudio {
            track_id: LEGACY.into(),
        },
    }
}
fn native(doc: &ProjectDocument) -> ClipSelection {
    ClipSelection {
        project_id: doc.id().into(),
        address: DocumentClipAddress::NativeAudio {
            track_id: NATIVE.into(),
            clip_id: "audio".into(),
        },
    }
}
fn note(doc: &ProjectDocument) -> NoteSelection {
    NoteSelection {
        project_id: doc.id().into(),
        address: DocumentNoteAddress {
            track_id: "notes".into(),
            note_id: String::new(),
        },
    }
}
fn manual() -> (StudioApp, TestPeer) {
    let (service, peer) = SessionService::test_pair();
    let mut app = StudioApp::new(Paths {
        native: PathBuf::from("/synthetic/arrangement-review/no-catalog"),
        legacy: PathBuf::from("/synthetic/arrangement-review/no-originals"),
    });
    app.service = Some(service);
    app.ready = true;
    app.install_document(DocumentView::prepare(document()).unwrap());
    let target = legacy(app.document.as_ref().unwrap());
    app.dispatch(Action::Clip(ClipAction::Select(target)), None)
        .unwrap();
    (app, peer)
}
fn frame(ctx: &egui::Context, app: &mut StudioApp, events: Vec<egui::Event>) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1400.0),
            )),
            events,
            ..Default::default()
        },
        |ui| app.native_ui(ui),
    );
    output.textures_delta.clear();
    output
}
fn click(ctx: &egui::Context, app: &mut StudioApp, label: &str) {
    let _ = frame(ctx, app, vec![]);
    let tree = frame(ctx, app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    let target = tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some(label))
        .unwrap_or_else(|| panic!("Missing control: {label}"))
        .0;
    let _ = frame(
        ctx,
        app,
        vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: tree.tree_id,
                target_node: target,
                data: None,
            },
        )],
    );
}
fn stage(app: &mut StudioApp, value: PlacementValue) -> AppResult<()> {
    let expected = app.clips.context();
    app.dispatch(
        Action::StagePlacement {
            expected: Box::new(expected),
            value,
        },
        None,
    )
}
fn command_intent(request: &Request) -> &ProjectEditIntent {
    let Command::Edit { intent } = &request.command else {
        panic!("Expected a clip edit");
    };
    intent
}
fn answer(
    app: &mut StudioApp,
    peer: &TestPeer,
    request: &Request,
    result: Result<Reply, ServiceError>,
) {
    peer.responses
        .try_send(Completion {
            id: request.id,
            kind: request.command.kind(),
            result,
        })
        .unwrap();
    app.poll();
}
fn changed(doc: &ProjectDocument, intent: &ProjectEditIntent, revision: u64) -> ProjectDocument {
    let EditScope::DocumentClip { address } = &intent.scope else {
        panic!("Expected exact clip address");
    };
    let EditOperation::UpdateClip { patch, timeline } = &intent.edit else {
        panic!("Expected clip update");
    };
    doc.update_clip(address, patch, *timeline)
        .unwrap()
        .with_revision(revision)
        .unwrap()
}
fn success(doc: ProjectDocument) -> Reply {
    Reply::Document {
        view: DocumentView::prepare(doc).unwrap(),
        library: Library::default(),
        replayed: false,
    }
}
fn legacy_fields(app: &mut StudioApp) -> &mut clip_ui::LegacyClipDraft {
    let ClipDraftFields::LegacyAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        panic!("Legacy draft expected");
    };
    fields
}
fn native_fields(app: &mut StudioApp) -> &mut clip_ui::NativeClipDraft {
    let ClipDraftFields::NativeAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        panic!("Native draft expected");
    };
    fields
}

#[test]
fn placement_is_local_until_save_and_keeps_all_unrelated_raw_source_fields() {
    let (mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    let bytes = original.to_json().unwrap();
    let before = app.clips.draft.as_ref().unwrap().clone();
    stage(&mut app, PlacementValue::LegacyTick(12345)).unwrap();
    let mut expected = before;
    let ClipDraftFields::LegacyAudio(fields) = &mut expected.fields else {
        panic!();
    };
    fields.start_tick.mode = FieldMode::Set;
    fields.start_tick.text = "12345".into();
    assert_eq!(app.clips.draft.as_ref(), Some(&expected));
    assert!(peer.requests.try_recv().is_err());
    assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), bytes);
    assert!(app.active.is_none() && app.retry.is_none());
    app.dispatch(Action::Clip(ClipAction::Save), None).unwrap();
    let request = peer.requests.try_recv().unwrap();
    let EditOperation::UpdateClip {
        patch: ClipPatch::LegacyAudio(patch),
        timeline,
    } = &command_intent(&request).edit
    else {
        panic!();
    };
    assert_eq!(patch.start_tick, FieldChange::Set { value: 12345 });
    assert_eq!(patch.trim_start_seconds, FieldChange::Keep);
    assert_eq!(patch.trim_end_seconds, FieldChange::Keep);
    assert_eq!(patch.fade_in_seconds, FieldChange::Keep);
    assert_eq!(patch.fade_out_seconds, FieldChange::Keep);
    assert_eq!(*timeline, omniusong_core::ClipTimelinePolicy::PreserveLoop);
    let next = changed(&original, command_intent(&request), 1);
    let current = &next.value()["tracks"][1]["audio"];
    let source = &original.value()["tracks"][1]["audio"];
    assert_eq!(current["vendor"], source["vendor"]);
    assert_eq!(
        current["trimStartSeconds"].as_number().unwrap().as_str(),
        "0.123456789012345678901234567890123456789"
    );
    assert_eq!(
        current["fadeInSeconds"].as_number().unwrap().as_str(),
        "0.0000"
    );
    assert!(current["editSource"]["original"].get("startTick").is_none());
    assert_eq!(
        current["editSource"]["original"]["trimStartSeconds"],
        source["trimStartSeconds"]
    );
    assert_eq!(
        next.value()["tracks"][1]["notes"],
        original.value()["tracks"][1]["notes"]
    );
}

#[test]
fn native_stage_preserves_dirty_window_fades_and_policy_with_large_exact_source_ordinals() {
    let (mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    app.dispatch(Action::Clip(ClipAction::Select(native(&doc))), None)
        .unwrap();
    let fields = native_fields(&mut app);
    fields.window_mode = ValueMode::Set;
    fields.source_start_frame = "9007199254741001".into();
    fields.frame_count = "120".into();
    fields.fades_mode = FieldMode::Remove;
    app.clips.draft.as_mut().unwrap().extend_loop = true;
    app.clips.draft.as_mut().unwrap().allow_upgrade = true;
    let before = app.clips.draft.as_ref().unwrap().clone();
    stage(
        &mut app,
        PlacementValue::NativeFrame(-9_007_199_254_740_991),
    )
    .unwrap();
    let mut expected = before;
    let ClipDraftFields::NativeAudio(fields) = &mut expected.fields else {
        panic!();
    };
    fields.position_mode = ValueMode::Set;
    fields.position_frame = "-9007199254740991".into();
    assert_eq!(app.clips.draft.as_ref(), Some(&expected));
    assert!(peer.requests.try_recv().is_err());
    assert_eq!(
        app.document.as_ref().unwrap().to_json().unwrap(),
        doc.to_json().unwrap()
    );
    let intent = app.clips.intent(&doc, &ClipAction::Save).unwrap();
    let EditOperation::UpdateClip {
        patch: ClipPatch::NativeAudio(patch),
        ..
    } = &intent.edit
    else {
        panic!();
    };
    assert_eq!(
        patch.position_frame,
        ValueChange::Set {
            value: -9_007_199_254_740_991
        }
    );
    assert_eq!(patch.fades, FieldChange::Remove);
    let next = changed(&doc, &intent, 1);
    let original_clip = &doc.value()["tracks"][2]["recording"]["clips"][0];
    let next_clip = &next.value()["tracks"][2]["recording"]["clips"][0];
    assert_eq!(next_clip["source"], original_clip["source"]);
    assert_eq!(
        next_clip["editSource"]["original"]["range"],
        original_clip["range"]
    );
    assert_eq!(
        next_clip["editSource"]["original"].get("fades"),
        Some(&Value::Null)
    );
    assert_eq!(
        next.value()["tracks"][2]["notes"],
        doc.value()["tracks"][2]["notes"]
    );
}

#[test]
fn equal_effective_placement_preserves_every_mode_text_and_context_serial() {
    for (mode, raw) in [
        (FieldMode::Keep, "unsent-kept-text"),
        (FieldMode::Remove, "kept removal text"),
        (FieldMode::Set, "0000"),
    ] {
        let (mut app, peer) = manual();
        let fields = legacy_fields(&mut app);
        fields.start_tick.mode = mode;
        fields.start_tick.text = raw.into();
        let before = app.clips.context();
        stage(&mut app, PlacementValue::LegacyTick(0)).unwrap();
        assert_eq!(app.clips.context(), before);
        assert!(peer.requests.try_recv().is_err());
    }
    let (mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    app.dispatch(Action::Clip(ClipAction::Select(native(&doc))), None)
        .unwrap();
    let fields = native_fields(&mut app);
    fields.position_mode = ValueMode::Set;
    fields.position_frame = "-0009007199254740993".into();
    let before = app.clips.context();
    stage(
        &mut app,
        PlacementValue::NativeFrame(-9_007_199_254_740_993),
    )
    .unwrap();
    assert_eq!(app.clips.context(), before);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn stale_whole_draft_and_same_revision_installation_cannot_rebase_a_gesture() {
    let (mut app, peer) = manual();
    let captured = app.clips.context();
    legacy_fields(&mut app).trim_end_seconds.text = "invalid newer text".into();
    let newer = app.clips.context();
    assert!(
        app.dispatch(
            Action::StagePlacement {
                expected: Box::new(captured),
                value: PlacementValue::LegacyTick(19)
            },
            None
        )
        .is_err()
    );
    assert_eq!(app.clips.context(), newer);
    app.clips.discard(app.document.as_ref().unwrap());
    let captured = app.clips.context();
    let same_revision = app.document.as_ref().unwrap().clone();
    app.install_document(DocumentView::prepare(same_revision).unwrap());
    let installed = app.clips.context();
    assert!(
        app.dispatch(
            Action::StagePlacement {
                expected: Box::new(captured),
                value: PlacementValue::LegacyTick(19)
            },
            None
        )
        .is_err()
    );
    assert_eq!(app.clips.context(), installed);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn source_replacement_wrong_units_locked_targets_and_copy_identity_fail_without_mutation() {
    let (mut app, peer) = manual();
    let before = app.clips.context();
    assert!(stage(&mut app, PlacementValue::NativeFrame(19)).is_err());
    assert_eq!(app.clips.context(), before);
    assert!(stage(&mut app, PlacementValue::LegacyTick(1_000_000_001)).is_err());
    assert_eq!(app.clips.context(), before);
    let mut value = app.document.as_ref().unwrap().value().clone();
    value["tracks"][1]["assetId"] = format!("{}.wav", "d".repeat(64)).into();
    app.document = Some(ProjectDocument::from_value(value).unwrap());
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), before);
    let mut value = app.document.as_ref().unwrap().value().clone();
    value["id"] = uuid::Uuid::new_v4().to_string().into();
    app.document = Some(ProjectDocument::from_value(value).unwrap());
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), before);
    let doc = app.document.as_ref().unwrap().clone();
    let locked = ClipSelection {
        project_id: doc.id().into(),
        address: DocumentClipAddress::LegacyAudio {
            track_id: "locked".into(),
        },
    };
    app.clips.select(&doc, locked).unwrap();
    let locked_context = app.clips.context();
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), locked_context);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn main_guard_matrix_keeps_invalid_foreign_fields_and_refuses_direct_staging() {
    for guard in 0..10 {
        let (mut app, peer) = manual();
        match guard {
            0 => app.ready = false,
            1 => app.pending = Some(Action::Close),
            2 => app.closing = true,
            3 => app.audio_close_requested = true,
            4 => app.recovery_close_requested = true,
            5 => app.note_close_requested = true,
            6 => app.clip_close_requested = true,
            7 => app.draft.tempo = "not a tempo".into(),
            8 => app.draft.name = "".into(),
            9 => {
                let doc = app.document.as_ref().unwrap();
                app.notes.select(doc, note(doc)).unwrap();
                app.notes.draft.as_mut().unwrap().velocity = "invalid velocity".into();
            }
            _ => unreachable!(),
        }
        let clip = app.clips.context();
        let notes = app.notes.context();
        let metadata = app.draft.clone();
        assert!(
            stage(&mut app, PlacementValue::LegacyTick(19)).is_err(),
            "guard {guard}"
        );
        assert_eq!(app.clips.context(), clip, "guard {guard}");
        assert_eq!(app.notes.context(), notes, "guard {guard}");
        assert!(app.draft == metadata, "guard {guard}");
        assert!(peer.requests.try_recv().is_err());
    }
}

#[test]
fn outstanding_read_and_disconnected_service_do_not_admit_placement() {
    let (mut app, peer) = manual();
    app.submit(Command::Refresh, Action::Refresh, None).unwrap();
    let _held = peer.requests.try_recv().unwrap();
    let before = app.clips.context();
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), before);
    assert!(peer.requests.try_recv().is_err());
    drop(peer);
    app.poll();
    assert!(app.disconnected());
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), before);
}

#[test]
fn playback_and_retained_capture_attachment_block_staging_without_touching_originals() {
    let (mut app, peer) = manual();
    let (audio, audio_peer) = audio_ui::test_support::pair();
    app.audio = audio;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    click(&ctx, &mut app, "Play saved session");
    assert!(matches!(
        audio_peer.command(),
        Some(omniusong_engine::EngineCommand::StartPlayback { .. })
    ));
    audio_peer.reply(Ok(omniusong_engine::EngineResult::Playing(
        audio_ui::test_support::status("playing"),
    )));
    app.audio.poll();
    assert!(app.audio.blocks_session());
    let before = app.clips.context();
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), before);
    assert!(peer.requests.try_recv().is_err());

    app.audio = AudioUi::new();
    app.recovery.scanned(recovery_ui::test_support::scan());
    app.recovery
        .inspected(recovery_ui::test_support::inspection(true))
        .unwrap();
    let project_id = app.document.as_ref().unwrap().id().to_owned();
    let RecoveryAction::ChooseAttachment(choice) =
        recovery_ui::test_support::attach(&app.recovery, &project_id)
    else {
        panic!();
    };
    let action = app
        .recovery
        .prepare_attachment(&choice, &project_id, 0)
        .unwrap();
    app.recovery.started(&action);
    assert!(app.recovery.blocks_session());
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), before);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn held_and_suspended_uncertain_clip_intents_are_never_replaced_by_placement() {
    for suspend in [false, true] {
        let (mut app, peer) = manual();
        stage(&mut app, PlacementValue::LegacyTick(17)).unwrap();
        let doc = app.document.as_ref().unwrap();
        let intent = app.clips.intent(doc, &ClipAction::Save).unwrap();
        let retained = app.clips.draft.clone();
        app.clips
            .failed(intent.clone(), "Uncertain synthetic save".into(), false);
        if suspend {
            app.clips.suspend();
        }
        let before = app.clips.context();
        assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
        assert_eq!(app.clips.context(), before);
        let failure = app.clips.failure.as_ref().unwrap();
        assert_eq!(failure.intent, intent);
        assert_eq!(failure.draft, retained);
        assert!(!failure.known_rejected);
        assert_eq!(failure.suspended, suspend);
        assert!(peer.requests.try_recv().is_err());
    }
}

#[test]
fn exact_cross_track_selection_clears_only_opposite_clean_selection_and_preserves_failure_evidence()
{
    let (mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    app.dispatch(Action::Note(NoteAction::Select(note(&doc))), None)
        .unwrap();
    assert_eq!(app.selected_track, 0);
    assert!(app.clips.selection.is_none());
    app.notes.draft.as_mut().unwrap().velocity = "60000".into();
    let note_intent = app.notes.intent(&doc, &NoteAction::Save).unwrap();
    app.notes.failed(
        note_intent.clone(),
        "Unknown older note outcome".into(),
        false,
    );
    app.notes.suspend();
    app.dispatch(Action::Clip(ClipAction::Select(native(&doc))), None)
        .unwrap();
    assert_eq!(app.selected_track, 2);
    assert_eq!(app.clips.selection.as_ref(), Some(&native(&doc)));
    assert!(app.notes.selection.is_none());
    assert_eq!(app.notes.failure.as_ref().unwrap().intent, note_intent);
    let before = app.clips.context();
    assert!(stage(&mut app, PlacementValue::NativeFrame(19)).is_err());
    assert_eq!(app.clips.context(), before);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn dirty_selection_modal_cannot_carry_a_stale_drag_into_the_next_target() {
    let (mut app, peer) = manual();
    stage(&mut app, PlacementValue::LegacyTick(17)).unwrap();
    let captured = app.clips.context();
    let doc = app.document.as_ref().unwrap().clone();
    app.request(Action::Clip(ClipAction::Select(native(&doc))));
    assert!(app.pending.is_some());
    assert_eq!(app.selected_track, 1);
    assert!(
        app.dispatch(
            Action::StagePlacement {
                expected: Box::new(captured.clone()),
                value: PlacementValue::LegacyTick(19)
            },
            None
        )
        .is_err()
    );
    assert_eq!(app.clips.context(), captured);
    app.discard_active_family().unwrap();
    let next = app.pending.take().unwrap();
    app.dispatch(next, None).unwrap();
    let selected = app.clips.context();
    assert_eq!(app.selected_track, 2);
    assert!(
        app.dispatch(
            Action::StagePlacement {
                expected: Box::new(captured),
                value: PlacementValue::LegacyTick(19)
            },
            None
        )
        .is_err()
    );
    assert_eq!(app.clips.context(), selected);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn uncertain_save_exact_retry_keeps_original_operation_and_pending_patch() {
    let (mut app, peer) = manual();
    stage(&mut app, PlacementValue::LegacyTick(17)).unwrap();
    app.dispatch(Action::Clip(ClipAction::Save), None).unwrap();
    let first = peer.requests.try_recv().unwrap();
    let original_intent = command_intent(&first).clone();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError {
            kind: FailureKind::Rejected,
            message: "Synthetic late result failure".into(),
            outcome_unknown: true,
        }),
    );
    let before = app.clips.context();
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    assert_eq!(app.clips.context(), before);
    app.dispatch(Action::Clip(ClipAction::Retry), None).unwrap();
    let repeated = peer.requests.try_recv().unwrap();
    assert_eq!(command_intent(&repeated), &original_intent);
    assert_ne!(first.id, repeated.id);
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, original_intent);
    assert!(!app.clips.failure.as_ref().unwrap().known_rejected);
}

#[test]
fn stale_save_completion_keeps_newer_local_fields_and_original_admitted_draft() {
    let (mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    stage(&mut app, PlacementValue::LegacyTick(17)).unwrap();
    let admitted_draft = app.clips.draft.clone();
    app.dispatch(
        Action::Clip(ClipAction::Save),
        Some(Action::Open(doc.id().into())),
    )
    .unwrap();
    let request = peer.requests.try_recv().unwrap();
    assert!(stage(&mut app, PlacementValue::LegacyTick(19)).is_err());
    legacy_fields(&mut app).trim_end_seconds.text = "newer invalid unsent text".into();
    legacy_fields(&mut app).trim_end_seconds.mode = FieldMode::Set;
    let newer = app.clips.draft.clone();
    let next = changed(&doc, command_intent(&request), 1);
    answer(&mut app, &peer, &request, Ok(success(next)));
    assert_eq!(app.clips.draft, newer);
    assert_eq!(app.document.as_ref().unwrap().revision(), 0);
    let failure = app.clips.failure.as_ref().unwrap();
    assert_eq!(failure.intent, *command_intent(&request));
    assert_eq!(failure.draft, admitted_draft);
    assert!(!failure.known_rejected);
    assert!(
        peer.requests.try_recv().is_err(),
        "Stale completion must not continue navigation"
    );
}

#[test]
fn raw_event_address_is_not_an_audio_move_target_and_leaves_selected_clip_intact() {
    let (mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap();
    let forged = ClipSelection {
        project_id: doc.id().into(),
        address: DocumentClipAddress::NativeAudio {
            track_id: NATIVE.into(),
            clip_id: "events".into(),
        },
    };
    let before = app.clips.context();
    assert!(
        app.dispatch(Action::Clip(ClipAction::Select(forged)), None)
            .is_err()
    );
    assert_eq!(app.clips.context(), before);
    assert_eq!(app.selected_track, 1);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn snapped_subunit_motion_preserves_off_grid_positions_and_complete_local_context() {
    use crate::arrangement_math::{PlacementBasis, TempoRatio, drag_candidate};
    use crate::arrangement_types::{Snap, TickFp};
    for snap in [Snap::Beat, Snap::SixteenthNote] {
        for delta in [TickFp(-1), TickFp(1)] {
            let (mut app, peer) = manual();
            stage(&mut app, PlacementValue::LegacyTick(123)).unwrap();
            let before = app.clips.context();
            let tempo = TempoRatio::from_bpm(119.25).unwrap();
            let value = drag_candidate(
                PlacementBasis::Legacy {
                    start_tick: 123,
                    duration_seconds: 1.0,
                },
                delta,
                &tempo,
                snap,
            )
            .unwrap();
            assert_eq!(value, PlacementValue::LegacyTick(123));
            stage(&mut app, value).unwrap();
            assert_eq!(app.clips.context(), before);
            assert!(peer.requests.try_recv().is_err());

            let doc = app.document.as_ref().unwrap().clone();
            app.clips.discard(&doc);
            app.dispatch(Action::Clip(ClipAction::Select(native(&doc))), None)
                .unwrap();
            stage(&mut app, PlacementValue::NativeFrame(12345)).unwrap();
            let before = app.clips.context();
            let value = drag_candidate(
                PlacementBasis::Native {
                    position_frame: 12345,
                    frame_count: 128,
                    sample_rate: 48_000,
                },
                delta,
                &tempo,
                snap,
            )
            .unwrap();
            assert_eq!(value, PlacementValue::NativeFrame(12345));
            stage(&mut app, value).unwrap();
            assert_eq!(app.clips.context(), before);
            assert!(peer.requests.try_recv().is_err());
        }
    }
}

#[test]
fn independent_full_domain_motion_preserves_valid_signed_endpoints_without_i64_delta_narrowing() {
    use crate::arrangement_math::{PlacementBasis, TempoRatio, drag_candidate};
    use crate::arrangement_types::{Snap, TICK_SCALE, TickFp};
    // Independently, at 300 BPM and 8000 Hz: each frame = (300/60)*960/8000 = 3/5 tick.
    let low = i64::MIN;
    let high = i64::MAX - 1;
    let frame_difference = i128::from(high) - i128::from(low);
    assert!(frame_difference > i128::from(i64::MAX));
    let delta = TickFp(frame_difference * TICK_SCALE * 3 / 5);
    let tempo = TempoRatio::from_bpm(300.0).unwrap();
    assert_eq!(
        drag_candidate(
            PlacementBasis::Native {
                position_frame: low,
                frame_count: 1,
                sample_rate: 8000
            },
            delta,
            &tempo,
            Snap::Off
        )
        .unwrap(),
        PlacementValue::NativeFrame(high)
    );
    assert_eq!(
        drag_candidate(
            PlacementBasis::Native {
                position_frame: high,
                frame_count: 1,
                sample_rate: 8000
            },
            TickFp(-delta.0),
            &tempo,
            Snap::Off
        )
        .unwrap(),
        PlacementValue::NativeFrame(low)
    );
}

#[test]
fn same_identity_revision_tempo_install_rebuilds_the_ui_nudge_baseline() {
    let (mut app, peer) = manual();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let original = app.document.as_ref().unwrap().clone();
    // Populate the actual arrangement cache with the omitted (zero) placement.
    let _ = frame(&ctx, &mut app, vec![]);
    let _ = frame(&ctx, &mut app, vec![]);
    let mut refreshed = original.value().clone();
    refreshed["tracks"][1]["audio"]["startTick"] = 1234.into();
    let refreshed = ProjectDocument::from_value(refreshed).unwrap();
    assert_eq!(refreshed.id(), original.id());
    assert_eq!(refreshed.revision(), original.revision());
    assert_eq!(refreshed.tempo().to_bits(), original.tempo().to_bits());
    app.install_document(DocumentView::prepare(refreshed.clone()).unwrap());
    assert!(!app.clips.dirty());
    // The genuine UI action must use freshly installed saved metadata. Without
    // explicit cache invalidation this proposes tick 1 from the previous image.
    click(&ctx, &mut app, "Nudge clip later");
    let fields = legacy_fields(&mut app);
    assert_eq!(fields.start_tick.mode, FieldMode::Set);
    assert_eq!(fields.start_tick.text, "1235");
    assert_eq!(
        app.document.as_ref().unwrap().to_json().unwrap(),
        refreshed.to_json().unwrap()
    );
    assert!(
        peer.requests.try_recv().is_err(),
        "Nudge stages locally; it must not save"
    );
}

#[test]
fn one_frame_geometry_at_signed_extremes_subtracts_exact_origin_before_pixels() {
    use crate::arrangement_math::{ClipTiming, TempoRatio, project_interval};
    use crate::arrangement_types::{TickFp, Viewport};
    // At 120 BPM and 48 kHz each source frame is exactly 1/25 tick,
    // independently equal to 40,000,000 fixed display units.
    let tempo = TempoRatio::from_bpm(120.0).unwrap();
    for frame in [i64::MIN, i64::MAX - 2] {
        let interval = project_interval(
            ClipTiming::Native {
                position_frame: frame,
                frame_count: 1,
                sample_rate: 48_000,
            },
            &tempo,
        )
        .unwrap();
        let exact_start = i128::from(frame) * 40_000_000;
        assert_eq!(interval.start, TickFp(exact_start));
        assert_eq!(interval.end, TickFp(exact_start + 40_000_000));
        let view = Viewport::new(TickFp(exact_start), TickFp(80_000_000)).unwrap();
        let pixels = view.project_interval(interval, 800.0).unwrap().unwrap();
        assert_eq!(pixels.start, 0.0);
        assert_eq!(pixels.end, 400.0);
    }
}

#[test]
fn legacy_estimate_preserves_integer_start_before_binary_duration_projection() {
    use crate::arrangement_math::{ClipTiming, TempoRatio, project_interval};
    use crate::arrangement_types::TickFp;
    let interval = project_interval(
        ClipTiming::Legacy {
            start_tick: 1_000_000_000,
            duration_seconds: 0.123456789,
        },
        &TempoRatio::from_bpm(119.25).unwrap(),
    )
    .unwrap();
    assert_eq!(interval.start, TickFp(1_000_000_000_000_000_000));
    assert_eq!(interval.end, TickFp(1_000_000_235_555_553_412));
    // This deliberate display estimate differs from first adding the start in
    // f64 as the authoritative core loop-extent calculation does.
    assert_ne!(interval.end, TickFp(1_000_000_235_555_553_437));
}

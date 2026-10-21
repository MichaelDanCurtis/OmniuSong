//! Independent main/clip integration using real egui and bounded injected workers.
//! Documents are synthetic unbound views, not authenticated takes or device audio.
use super::*;
use crate::clip_ui::{
    ClipAction, ClipDraftFields, ClipEditor, ClipSelection, FieldMode, ValueMode,
};
use omniusong_core::{
    ClipPatch, ClipTimelinePolicy, DocumentClipAddress, DocumentNoteAddress, FieldChange,
    ValueChange,
    native::{AudioFades, FadeCurve, TakeClip, TakeRange, TakeReference, TimelinePosition},
};
use omniusong_session::{Request, ServiceError, TestPeer};
use serde_json::json;

const LEGACY: &str = "\0🎛/legacy";
const RECORDING: &str = "recording-track";
fn document() -> ProjectDocument {
    let mut value = ProjectDocument::new("Independent synthetic clip UI")
        .unwrap()
        .value()
        .clone();
    value["tracks"][0]["id"] = "note-track".into();
    value["tracks"][0]["notes"] = json!([{"id":"note","pitch":60,"startTick":17,"durationTicks":961,"velocity16":32769,"expression":null}]);
    value["tracks"][1]["id"] = LEGACY.into();
    value["tracks"][1]["name"] = "Synthetic legacy audio".into();
    value["tracks"][1]["kind"] = "audio".into();
    value["tracks"][1]["assetId"] = format!("{}.wav", "a".repeat(64)).into();
    value["tracks"][1]["audio"] = serde_json::from_str(r#"{"origin":"import","mimeType":"audio/wav","durationSeconds":2.0,"byteLength":100,"trimStartSeconds":0.123456789012345678901234567890123456789,"fadeInSeconds":0.0000,"sourceIdentity":{"opaque":"DO_NOT_RENDER_SOURCE"}}"#).unwrap();
    let mut doc = ProjectDocument::from_value(value)
        .unwrap()
        .upgrade_native()
        .unwrap();
    for (id, range, position, fades) in [
        (
            "clip-a",
            TakeRange::AudioFrames {
                start: 100,
                count: 128,
            },
            0,
            None,
        ),
        (
            "clip-b",
            TakeRange::AudioFrames {
                start: 9_007_199_254_740_997,
                count: 123,
            },
            -9_007_199_254_740_993,
            Some(AudioFades {
                in_frames: 2,
                out_frames: 4,
                curve: FadeCurve::EqualPower,
            }),
        ),
        (
            "event-only",
            TakeRange::Events { start: 4, count: 2 },
            7,
            None,
        ),
    ] {
        doc.append_take_clip(
            RECORDING,
            "Synthetic declared clips",
            TakeClip {
                id: id.into(),
                source: TakeReference {
                    take_id: omniusong_core::contracts::TakeId("synthetic-unbound-take".into()),
                    manifest_sha256: "b".repeat(64),
                    stream_id: if id == "event-only" { "raw" } else { "audio" }.into(),
                },
                range,
                position: TimelinePosition {
                    frame: position,
                    sample_rate: 48_000,
                },
                fades,
                edit_source: None,
            },
        )
        .unwrap();
    }
    let mut value = doc.value().clone();
    value["tracks"][2]["recording"]["clips"][0]["fades"] = Value::Null;
    value["tracks"][2]["notes"] = json!([null,{"opaque":"NOT_EDITABLE_NOTES"}]);
    let mut locked = value["tracks"][1].clone();
    locked["id"] = "locked-audio".into();
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
fn native(doc: &ProjectDocument, id: &str) -> ClipSelection {
    ClipSelection {
        project_id: doc.id().into(),
        address: DocumentClipAddress::NativeAudio {
            track_id: RECORDING.into(),
            clip_id: id.into(),
        },
    }
}
fn manual() -> (egui::Context, StudioApp, TestPeer) {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let (service, peer) = SessionService::test_pair();
    let mut app = StudioApp::new(Paths {
        native: PathBuf::from("/synthetic/clip-review/no-files/native"),
        legacy: PathBuf::from("/synthetic/clip-review/no-files/legacy"),
    });
    app.service = Some(service);
    app.ready = true;
    app.install_document(DocumentView::prepare(document()).unwrap());
    app.selected_track = 1;
    let selection = legacy(app.document.as_ref().unwrap());
    app.clips
        .select(app.document.as_ref().unwrap(), selection)
        .unwrap();
    (ctx, app, peer)
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
fn action(
    ctx: &egui::Context,
    app: &mut StudioApp,
    label: &str,
    kind: egui::accesskit::Action,
    data: Option<egui::accesskit::ActionData>,
) {
    let _ = frame(ctx, app, vec![]);
    let tree = frame(ctx, app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    let target = tree
        .nodes
        .iter()
        .find(|(_, node)| {
            node.label() == Some(label)
                && (kind != egui::accesskit::Action::SetValue || node.supports_action(kind))
        })
        .unwrap_or_else(|| {
            panic!(
                "Missing {label}; text={:?}",
                tree.nodes
                    .iter()
                    .flat_map(|(_, n)| [n.label(), n.value()].into_iter().flatten())
                    .collect::<Vec<_>>()
            )
        })
        .0;
    let _ = frame(
        ctx,
        app,
        vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: kind,
                target_tree: tree.tree_id,
                target_node: target,
                data,
            },
        )],
    );
}
fn click(ctx: &egui::Context, app: &mut StudioApp, label: &str) {
    action(ctx, app, label, egui::accesskit::Action::Click, None);
}
fn set_text(ctx: &egui::Context, app: &mut StudioApp, label: &str, value: &str) {
    action(
        ctx,
        app,
        label,
        egui::accesskit::Action::SetValue,
        Some(egui::accesskit::ActionData::Value(value.into())),
    );
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
fn reply(doc: ProjectDocument, replayed: bool) -> Reply {
    Reply::Document {
        view: DocumentView::prepare(doc).unwrap(),
        library: Library::default(),
        replayed,
    }
}
fn edit(request: &Request) -> &ProjectEditIntent {
    let Command::Edit { intent } = &request.command else {
        panic!("Expected Edit, got {:?}", request.command)
    };
    intent
}
fn changed(doc: &ProjectDocument, intent: &ProjectEditIntent, revision: u64) -> ProjectDocument {
    let EditScope::DocumentClip { address } = &intent.scope else {
        panic!("Expected exact clip scope")
    };
    match &intent.edit {
        EditOperation::UpdateClip { patch, timeline } => doc.update_clip(address, patch, *timeline),
        EditOperation::RestoreClip { timeline } => doc.restore_clip(address, *timeline),
        _ => panic!("Expected clip operation"),
    }
    .unwrap()
    .with_revision(revision)
    .unwrap()
}
fn set_start(app: &mut StudioApp, raw: &str) {
    let ClipDraftFields::LegacyAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        panic!("legacy draft")
    };
    fields.start_tick.mode = FieldMode::Set;
    fields.start_tick.text = raw.into();
}
fn position(app: &mut StudioApp, raw: &str) {
    let ClipDraftFields::NativeAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        panic!("native draft")
    };
    fields.position_mode = ValueMode::Set;
    fields.position_frame = raw.into();
}
fn known_conflict() -> ServiceError {
    ServiceError {
        kind: FailureKind::RevisionConflict,
        message: "Synthetic expected-revision conflict".into(),
        outcome_unknown: false,
    }
}

#[test]
fn legacy_modes_preserve_exact_decimal_and_omission_while_real_text_input_submits_only_explicit_fields()
 {
    let (ctx, mut app, peer) = manual();
    let source = app.document.as_ref().unwrap().to_json().unwrap();
    let draft = app.clips.draft.as_ref().unwrap();
    let ClipDraftFields::LegacyAudio(fields) = &draft.fields else {
        panic!()
    };
    assert_eq!(fields.trim_start_seconds.mode, FieldMode::Keep);
    assert_eq!(
        fields.trim_start_seconds.text,
        "0.123456789012345678901234567890123456789"
    );
    assert_eq!(fields.fade_in_seconds.text, "0.0000");
    assert!(!draft.dirty());
    let ClipPatch::LegacyAudio(patch) = draft.patch().unwrap() else {
        panic!()
    };
    assert_eq!(patch.trim_start_seconds, FieldChange::Keep);
    assert_eq!(patch.trim_end_seconds, FieldChange::Keep);
    let ClipDraftFields::LegacyAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        panic!()
    };
    fields.trim_start_seconds.mode = FieldMode::Set;
    set_text(
        &ctx,
        &mut app,
        "Source in (seconds)",
        "0.250000000000000000000000000000000000001",
    );
    click(&ctx, &mut app, "Save clip");
    let request = peer.requests.try_recv().unwrap();
    let EditOperation::UpdateClip {
        patch: ClipPatch::LegacyAudio(patch),
        timeline,
    } = &edit(&request).edit
    else {
        panic!()
    };
    assert_eq!(*timeline, ClipTimelinePolicy::PreserveLoop);
    assert_eq!(patch.start_tick, FieldChange::Keep);
    assert_eq!(patch.fade_in_seconds, FieldChange::Keep);
    assert_eq!(patch.trim_end_seconds, FieldChange::Keep);
    let FieldChange::Set { value } = &patch.trim_start_seconds else {
        panic!()
    };
    assert_eq!(value.as_str(), "0.250000000000000000000000000000000000001");
    assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), source);
}

#[test]
fn native_signed_large_frames_and_null_fade_are_kept_exact_until_explicit_set_or_remove() {
    let (ctx, mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    app.selected_track = 2;
    app.clips.select(&doc, native(&doc, "clip-b")).unwrap();
    let ClipDraftFields::NativeAudio(fields) = &app.clips.draft.as_ref().unwrap().fields else {
        panic!()
    };
    assert_eq!(fields.position_frame, "-9007199254740993");
    assert_eq!(fields.source_start_frame, "9007199254740997");
    assert_eq!(fields.frame_count, "123");
    let tree = frame(&ctx, &mut app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    assert!(
        tree.nodes
            .iter()
            .any(|(_, n)| n.label() == Some("Placement (frames)")
                && n.value() == Some("-9007199254740993"))
    );
    app.clips.select(&doc, native(&doc, "clip-a")).unwrap();
    let ClipDraftFields::NativeAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        panic!()
    };
    assert_eq!(fields.fades_mode, FieldMode::Keep);
    fields.fades_mode = FieldMode::Remove;
    click(&ctx, &mut app, "Save clip");
    let request = peer.requests.try_recv().unwrap();
    let EditOperation::UpdateClip {
        patch: ClipPatch::NativeAudio(patch),
        ..
    } = &edit(&request).edit
    else {
        panic!()
    };
    assert_eq!(patch.fades, FieldChange::Remove);
    assert_eq!(patch.position_frame, ValueChange::Keep);
    assert_eq!(patch.source_window, ValueChange::Keep);
    let next = changed(&doc, edit(&request), 1);
    assert!(
        next.value()["tracks"][2]["recording"]["clips"][0]
            .get("fades")
            .is_none()
    );
    assert_eq!(
        next.value()["tracks"][2]["recording"]["clips"][0]["editSource"]["original"].get("fades"),
        Some(&Value::Null)
    );
}

#[test]
fn invalid_intermediate_native_and_legacy_text_cannot_publish_and_discard_remains_local() {
    let (ctx, mut app, peer) = manual();
    set_start(&mut app, "");
    click(&ctx, &mut app, "Save clip");
    assert!(peer.requests.try_recv().is_err());
    assert!(app.clips.draft.as_ref().unwrap().patch().is_err());
    click(&ctx, &mut app, "Discard clip");
    assert!(!app.clips.dirty());
    let doc = app.document.as_ref().unwrap().clone();
    app.selected_track = 2;
    app.clips.select(&doc, native(&doc, "clip-a")).unwrap();
    position(&mut app, "9223372036854775808");
    click(&ctx, &mut app, "Save clip");
    assert!(peer.requests.try_recv().is_err());
    assert!(app.clips.dirty());
    click(&ctx, &mut app, "Discard clip");
    assert!(!app.clips.dirty());
}

#[test]
fn exact_native_identity_survives_reorder_missing_target_and_copied_project_widgets_are_inert() {
    let (ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    app.selected_track = 2;
    app.clips
        .select(&original, native(&original, "clip-a"))
        .unwrap();
    let _ = frame(&ctx, &mut app, vec![]);
    let tree = frame(&ctx, &mut app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    let old_row = tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Select audio clip clip-b"))
        .expect("all native clip rows selectable")
        .0;
    let mut value = original.value().clone();
    value["tracks"][2]["recording"]["clips"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reordered = ProjectDocument::from_value(value)
        .unwrap()
        .with_revision(1)
        .unwrap();
    app.install_document(DocumentView::prepare(reordered.clone()).unwrap());
    let old_click = || {
        egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: tree.tree_id,
            target_node: old_row,
            data: None,
        })
    };
    let _ = frame(&ctx, &mut app, vec![old_click()]);
    assert_eq!(
        app.clips.selection.as_ref(),
        Some(&native(&reordered, "clip-b"))
    );
    let mut value = reordered.value().clone();
    value["tracks"][2]["recording"]["clips"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["id"] != "clip-b");
    let missing = ProjectDocument::from_value(value).unwrap();
    app.install_document(DocumentView::prepare(missing).unwrap());
    assert!(app.clips.draft.is_none());
    assert_eq!(
        app.clips.selection.as_ref(),
        Some(&native(&reordered, "clip-b"))
    );
    app.install_document(
        DocumentView::prepare(reordered.clone().with_revision(2).unwrap()).unwrap(),
    );
    assert!(app.clips.draft.is_some());
    position(&mut app, "-17");
    let save_tree = frame(&ctx, &mut app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    let old_save = save_tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Save clip"))
        .unwrap()
        .0;
    let mut value = reordered.value().clone();
    value["id"] = uuid::Uuid::new_v4().to_string().into();
    let copy = ProjectDocument::from_value(value).unwrap();
    app.install_document(DocumentView::prepare(copy.clone()).unwrap());
    let _ = frame(&ctx, &mut app, vec![old_click()]);
    assert!(app.clips.selection.is_none());
    app.clips.select(&copy, native(&copy, "clip-b")).unwrap();
    position(&mut app, "-23");
    let _ = frame(
        &ctx,
        &mut app,
        vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: save_tree.tree_id,
                target_node: old_save,
                data: None,
            },
        )],
    );
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn metadata_to_clip_save_continuation_and_clip_to_note_cancel_discard_do_not_mix_families() {
    let (ctx, mut app, peer) = manual();
    app.draft.name = "Metadata first".into();
    app.request(Action::Clip(ClipAction::BeginClip));
    assert!(app.pending.is_some());
    click(&ctx, &mut app, "Cancel");
    assert!(app.pending.is_none());
    assert_eq!(app.draft.name, "Metadata first");
    app.request(Action::Clip(ClipAction::BeginClip));
    click(&ctx, &mut app, "Save and continue");
    let request = peer.requests.try_recv().unwrap();
    assert!(matches!(
        edit(&request).edit,
        EditOperation::UpdateSession { .. }
    ));
    let mut next = app.document.as_ref().unwrap().clone();
    next.rename("Metadata first").unwrap();
    answer(
        &mut app,
        &peer,
        &request,
        Ok(reply(next.with_revision(1).unwrap(), false)),
    );
    assert!(!app.metadata_dirty());
    set_start(&mut app, "23");
    let selected = NoteSelection {
        project_id: app.document.as_ref().unwrap().id().into(),
        address: DocumentNoteAddress {
            track_id: "note-track".into(),
            note_id: "note".into(),
        },
    };
    app.request(Action::Note(NoteAction::Select(selected.clone())));
    click(&ctx, &mut app, "Cancel");
    assert!(app.clips.dirty());
    assert!(peer.requests.try_recv().is_err());
    app.request(Action::Note(NoteAction::Select(selected.clone())));
    click(&ctx, &mut app, "Discard and continue");
    assert!(!app.clips.dirty());
    assert_eq!(app.notes.selection.as_ref(), Some(&selected));
    assert_eq!(app.document.as_ref().unwrap().revision(), 1);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn save_clip_before_navigation_binds_saved_result_and_cannot_advance_while_worker_held() {
    let (ctx, mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    set_start(&mut app, "19");
    app.request(Action::Open(doc.id().into()));
    click(&ctx, &mut app, "Save and continue");
    let first = peer.requests.try_recv().unwrap();
    assert!(matches!(
        edit(&first).edit,
        EditOperation::UpdateClip { .. }
    ));
    assert_eq!(edit(&first).expected_revision, 0);
    assert!(peer.requests.try_recv().is_err());
    for _ in 0..3 {
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.busy());
    }
    let next = changed(&doc, edit(&first), 1);
    answer(&mut app, &peer, &first, Ok(reply(next, false)));
    let open = peer.requests.try_recv().unwrap();
    assert!(matches!(open.command, Command::Open { .. }));
    assert_eq!(app.document.as_ref().unwrap().revision(), 1);
}

#[test]
fn injected_multiple_dirty_families_fail_closed_without_selecting_discard_order() {
    let (ctx, mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    app.notes
        .select(
            &doc,
            NoteSelection {
                project_id: doc.id().into(),
                address: DocumentNoteAddress {
                    track_id: "note-track".into(),
                    note_id: "note".into(),
                },
            },
        )
        .unwrap();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    set_start(&mut app, "23");
    app.draft.name = "Third unsent family".into();
    let metadata = app.draft.clone();
    let note = app.notes.draft.clone();
    let clip = app.clips.draft.clone();
    assert!(app.dirty_family().is_err());
    assert!(app.discard_active_family().is_err());
    assert!(app.dispatch(Action::Save, None).is_err());
    assert!(app.dispatch(Action::Clip(ClipAction::Save), None).is_err());
    assert!(app.dispatch(Action::Note(NoteAction::Save), None).is_err());
    assert!(!app.can_start_audio());
    let _ = frame(&ctx, &mut app, vec![]);
    assert_eq!(app.draft, metadata);
    assert_eq!(app.notes.draft, note);
    assert_eq!(app.clips.draft, clip);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn busy_readonly_worker_and_os_close_preserve_invalid_unsent_clip_until_explicit_discard() {
    let (ctx, mut app, peer) = manual();
    set_start(&mut app, "invalid");
    app.execute(Action::Refresh);
    let _ = peer.requests.try_recv().unwrap();
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 1400.0),
        )),
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let mut output = ctx.run_ui(input, |ui| app.native_ui(ui));
    output.textures_delta.clear();
    assert!(app.pending.is_some());
    assert!(!app.closing);
    assert!(app.busy());
    click(&ctx, &mut app, "Cancel");
    assert!(app.clips.dirty());
    click(&ctx, &mut app, "Close window");
    click(&ctx, &mut app, "Discard and continue");
    assert!(app.closing);
    assert!(app.busy());
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn stale_success_preserves_newer_clip_and_attempted_evidence_and_cancels_deferred_navigation() {
    let (_ctx, mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    set_start(&mut app, "23");
    app.dispatch(
        Action::Clip(ClipAction::Save),
        Some(Action::Open("never-continue".into())),
    )
    .unwrap();
    let request = peer.requests.try_recv().unwrap();
    let attempted = app.clips.draft.clone();
    set_start(&mut app, "29");
    let newer = app.clips.draft.clone();
    answer(
        &mut app,
        &peer,
        &request,
        Ok(reply(changed(&doc, edit(&request), 1), false)),
    );
    assert_eq!(app.document.as_ref().unwrap().revision(), 0);
    assert_eq!(app.clips.draft, newer);
    assert_eq!(app.clips.failure.as_ref().unwrap().draft, attempted);
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, *edit(&request));
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn suspended_unknown_clip_survives_open_refresh_initialize_and_retries_old_intent_at_current_context()
 {
    let (_ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    set_start(&mut app, "23");
    app.dispatch(
        Action::Clip(ClipAction::Save),
        Some(Action::Open("cancelled".into())),
    )
    .unwrap();
    let first = peer.requests.try_recv().unwrap();
    let exact = serde_json::to_vec(edit(&first)).unwrap();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError::backend(
            "Synthetic unknown clip outcome",
            true,
        )),
    );
    app.execute(Action::Clip(ClipAction::Suspend));
    assert!(!app.clips.dirty());
    app.request(Action::Refresh);
    let refresh = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &refresh,
        Ok(Reply::Library(Library::default())),
    );
    assert_eq!(
        serde_json::to_vec(&app.clips.failure.as_ref().unwrap().intent).unwrap(),
        exact
    );
    app.execute_initialize();
    let init = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &init,
        Ok(Reply::Library(Library::default())),
    );
    assert!(peer.requests.try_recv().is_err());
    let other = ProjectDocument::new("Other synthetic document").unwrap();
    app.request(Action::Open(other.id().into()));
    let open = peer.requests.try_recv().unwrap();
    answer(&mut app, &peer, &open, Ok(reply(other, false)));
    assert!(app.clips.failure.is_some());
    app.request(Action::Open(original.id().into()));
    let open = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &open,
        Ok(reply(original.with_revision(5).unwrap(), false)),
    );
    app.request(Action::Clip(ClipAction::Retry));
    let retry = peer.requests.try_recv().unwrap();
    assert_ne!(first.id, retry.id);
    assert_eq!(serde_json::to_vec(edit(&retry)).unwrap(), exact);
    assert_eq!(
        app.active.as_ref().unwrap().context,
        Some((original.id().into(), 5))
    );
    answer(
        &mut app,
        &peer,
        &retry,
        Ok(reply(original.with_revision(6).unwrap(), true)),
    );
    assert!(app.clips.failure.is_none());
    assert_eq!(app.document.as_ref().unwrap().revision(), 6);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn known_clip_rejection_review_latest_does_not_rebase_changed_source_or_lose_retained_values() {
    let (ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    set_start(&mut app, "23");
    app.request(Action::Clip(ClipAction::Save));
    let first = peer.requests.try_recv().unwrap();
    let exact = edit(&first).clone();
    answer(&mut app, &peer, &first, Err(known_conflict()));
    app.request(Action::Clip(ClipAction::ReviewLatest));
    assert!(app.pending.is_none());
    let open = peer.requests.try_recv().unwrap();
    let mut value = original.value().clone();
    value["tracks"][1]["assetId"] = format!("{}.wav", "c".repeat(64)).into();
    let latest = ProjectDocument::from_value(value)
        .unwrap()
        .with_revision(4)
        .unwrap();
    answer(&mut app, &peer, &open, Ok(reply(latest, false)));
    app.request(Action::Clip(ClipAction::NewChoice));
    assert!(app.clips.failure.is_some());
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, exact);
    assert!(peer.requests.try_recv().is_err());
    click(&ctx, &mut app, "Release rejected clip choice");
    assert!(app.clips.failure.is_none());
    assert!(peer.requests.try_recv().is_err());
    let target = legacy(app.document.as_ref().unwrap());
    app.request(Action::Clip(ClipAction::Select(target)));
    set_start(&mut app, "31");
    click(&ctx, &mut app, "Save clip");
    let next = peer.requests.try_recv().unwrap();
    assert_ne!(edit(&next).operation_id, exact.operation_id);
    assert_eq!(edit(&next).expected_revision, 4);
}

#[test]
fn generic_metadata_retry_cannot_capture_and_overwrite_new_clip_context() {
    let (_ctx, mut app, peer) = manual();
    app.draft.name = "Old metadata save".into();
    app.save().unwrap();
    let first = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError::backend(
            "Uncertain metadata acknowledgement",
            true,
        )),
    );
    app.draft = Draft::from_document(app.document.as_ref().unwrap());
    set_start(&mut app, "23");
    let clip = app.clips.draft.clone();
    app.retry_last();
    assert!(peer.requests.try_recv().is_err());
    assert_eq!(app.clips.draft, clip);
}

#[test]
fn unresolved_clip_blocks_foreign_note_mutation_and_audio_but_explicit_stop_remains_available() {
    use audio_ui::test_support as audio;
    use omniusong_engine::{EngineCommand, EngineResult};
    let (ctx, mut app, peer) = manual();
    let (port, audio_peer) = audio::pair();
    app.audio = port;
    set_start(&mut app, "23");
    app.request(Action::Clip(ClipAction::Save));
    let first = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError::backend("Synthetic unknown clip result", true)),
    );
    app.request(Action::Clip(ClipAction::Suspend));
    assert!(!app.can_start_audio());
    click(&ctx, &mut app, "Advanced · solo input capture");
    for label in [
        "Play saved session",
        "Record solo",
        "Record keys/pads",
        "Start live audition",
    ] {
        click(&ctx, &mut app, label);
    }
    assert!(audio_peer.command().is_none());
    let doc = app.document.as_ref().unwrap().clone();
    app.notes
        .select(
            &doc,
            NoteSelection {
                project_id: doc.id().into(),
                address: DocumentNoteAddress {
                    track_id: "note-track".into(),
                    note_id: "note".into(),
                },
            },
        )
        .unwrap();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    assert!(app.dispatch(Action::Note(NoteAction::Save), None).is_err());
    assert!(peer.requests.try_recv().is_err());
    // Independent clean app: while playback starts, a pending Stop must survive
    // injected draft changes and use the existing bounded audio control path.
    let (ctx, mut app, peer) = manual();
    let (port, audio_peer) = audio::pair();
    app.audio = port;
    click(&ctx, &mut app, "Play saved session");
    assert!(matches!(
        audio_peer.command(),
        Some(EngineCommand::StartPlayback { .. })
    ));
    set_start(&mut app, "29");
    assert!(app.dispatch(Action::Clip(ClipAction::Save), None).is_err());
    click(&ctx, &mut app, "Stop audio");
    assert!(audio_peer.command().is_none());
    audio_peer.reply(Ok(EngineResult::Playing(audio::status("playing"))));
    app.poll_audio();
    assert!(matches!(audio_peer.command(), Some(EngineCommand::Stop)));
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn oversized_saved_legacy_number_is_kept_without_truncation_and_set_requires_fresh_input() {
    let (ctx, mut app, peer) = manual();
    let mut value = app.document.as_ref().unwrap().value().clone();
    let raw = format!("0.{}1", "0".repeat(400));
    value["tracks"][1]["audio"]["trimStartSeconds"] = Value::Number(raw.parse().unwrap());
    let doc = ProjectDocument::from_value(value).unwrap();
    let original = doc.to_json().unwrap();
    app.install_document(DocumentView::prepare(doc.clone()).unwrap());
    app.clips.select(&doc, legacy(&doc)).unwrap();
    let ClipDraftFields::LegacyAudio(fields) = &app.clips.draft.as_ref().unwrap().fields else {
        panic!()
    };
    assert_eq!(fields.trim_start_seconds.mode, FieldMode::Keep);
    assert!(fields.trim_start_seconds.text.is_empty());
    let ClipPatch::LegacyAudio(patch) = app.clips.draft.as_ref().unwrap().patch().unwrap() else {
        panic!()
    };
    assert_eq!(patch.trim_start_seconds, FieldChange::Keep);
    let tree = frame(&ctx, &mut app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    assert!(
        !tree
            .nodes
            .iter()
            .flat_map(|(_, n)| [n.label(), n.value()].into_iter().flatten())
            .any(|text| text.contains(&raw)),
        "An oversized saved lexeme must not become a full text field or source label"
    );
    let ClipDraftFields::LegacyAudio(fields) = &mut app.clips.draft.as_mut().unwrap().fields else {
        panic!()
    };
    fields.trim_start_seconds.mode = FieldMode::Set;
    click(&ctx, &mut app, "Save clip");
    assert!(peer.requests.try_recv().is_err());
    assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
    set_text(&ctx, &mut app, "Source in (seconds)", "0.5");
    click(&ctx, &mut app, "Save clip");
    assert!(matches!(
        edit(&peer.requests.try_recv().unwrap()).edit,
        EditOperation::UpdateClip { .. }
    ));
}

#[test]
fn locked_audio_is_inspectable_and_event_units_never_become_editable_audio() {
    let (ctx, mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    let mut editor = ClipEditor::default();
    editor.select(&doc, native(&doc, "clip-a")).unwrap();
    let before = editor.context();
    assert!(editor.select(&doc, native(&doc, "event-only")).is_err());
    assert_eq!(editor.context(), before);
    let locked = ClipSelection {
        project_id: doc.id().into(),
        address: DocumentClipAddress::LegacyAudio {
            track_id: "locked-audio".into(),
        },
    };
    app.selected_track = 3;
    app.clips.select(&doc, locked.clone()).unwrap();
    set_start(&mut app, "29");
    assert!(app.clips.intent(&doc, &ClipAction::Save).is_err());
    assert!(
        app.clips
            .intent(
                &doc,
                &ClipAction::Restore(locked, ClipTimelinePolicy::PreserveLoop)
            )
            .is_err()
    );
    click(&ctx, &mut app, "Save clip");
    assert!(peer.requests.try_recv().is_err());
    app.selected_track = 2;
    app.clips.clear_selection();
    let tree = frame(&ctx, &mut app, vec![])
        .platform_output
        .accesskit_update
        .unwrap();
    let text: Vec<_> = tree
        .nodes
        .iter()
        .flat_map(|(_, n)| [n.label(), n.value()].into_iter().flatten())
        .collect();
    assert!(text.iter().any(|text| text.contains("event-only")));
    assert!(
        !tree
            .nodes
            .iter()
            .any(|(_, n)| n.label() == Some("Select audio clip event-only")),
        "An event ordinal range must not expose the audio edit-selection action"
    );
}

#[test]
fn foreign_dirty_family_blocks_clip_retry_and_review_resolves_it_without_consuming_failed_values() {
    let (ctx, mut app, peer) = manual();
    let doc = app.document.as_ref().unwrap().clone();
    set_start(&mut app, "23");
    app.request(Action::Clip(ClipAction::Save));
    let failed = peer.requests.try_recv().unwrap();
    let exact = edit(&failed).clone();
    answer(&mut app, &peer, &failed, Err(known_conflict()));
    app.request(Action::Clip(ClipAction::Suspend));
    app.draft.name = "Foreign metadata draft".into();
    app.request(Action::Clip(ClipAction::Retry));
    assert!(peer.requests.try_recv().is_err());
    assert_eq!(app.draft.name, "Foreign metadata draft");
    app.request(Action::Clip(ClipAction::ReviewLatest));
    assert!(app.pending.is_some());
    click(&ctx, &mut app, "Cancel");
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, exact);
    assert!(peer.requests.try_recv().is_err());
    app.request(Action::Clip(ClipAction::ReviewLatest));
    click(&ctx, &mut app, "Discard and continue");
    let open = peer.requests.try_recv().unwrap();
    assert!(matches!(open.command, Command::Open { .. }));
    answer(
        &mut app,
        &peer,
        &open,
        Ok(reply(doc.with_revision(4).unwrap(), false)),
    );
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, exact);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn native_new_choice_rejects_changed_sample_rate_and_preserves_exact_old_patch() {
    let (_ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    app.selected_track = 2;
    app.clips
        .select(&original, native(&original, "clip-a"))
        .unwrap();
    position(&mut app, "-31");
    app.request(Action::Clip(ClipAction::Save));
    let first = peer.requests.try_recv().unwrap();
    let exact = edit(&first).clone();
    let attempted = app.clips.draft.clone();
    answer(&mut app, &peer, &first, Err(known_conflict()));
    app.request(Action::Clip(ClipAction::ReviewLatest));
    let open = peer.requests.try_recv().unwrap();
    let mut value = original.value().clone();
    value["tracks"][2]["recording"]["clips"][0]["position"]["sampleRate"] = 44_100.into();
    let latest = ProjectDocument::from_value(value)
        .unwrap()
        .with_revision(2)
        .unwrap();
    answer(&mut app, &peer, &open, Ok(reply(latest, false)));
    app.request(Action::Clip(ClipAction::NewChoice));
    let held = app
        .clips
        .failure
        .as_ref()
        .expect("changed source keeps exact old attempt");
    assert_eq!(held.intent, exact);
    assert_eq!(held.draft, attempted);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn retry_queue_rejection_and_suspended_close_keep_unknown_clip_evidence() {
    let (ctx, mut app, peer) = manual();
    set_start(&mut app, "23");
    app.request(Action::Clip(ClipAction::Save));
    let first = peer.requests.try_recv().unwrap();
    let intent = edit(&first).clone();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError::backend("May have committed", true)),
    );
    app.clips
        .not_admitted(intent.clone(), "Retry queue unavailable".into());
    assert!(!app.clips.failure.as_ref().unwrap().known_rejected);
    app.request(Action::Clip(ClipAction::Suspend));
    assert!(!app.dirty());
    app.request(Action::Close);
    assert!(!app.closing);
    assert!(app.clip_close_requested);
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, intent);
    let _ = frame(&ctx, &mut app, vec![]);
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn metadata_save_before_clip_review_retains_rejected_intent_and_opens_only_after_acknowledgement() {
    let (ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    set_start(&mut app, "23");
    app.request(Action::Clip(ClipAction::Save));
    let failed = peer.requests.try_recv().unwrap();
    let exact = edit(&failed).clone();
    let attempted = app.clips.draft.clone();
    answer(&mut app, &peer, &failed, Err(known_conflict()));
    app.request(Action::Clip(ClipAction::Suspend));
    assert!(app.clips.new_choice(&original).is_err());

    app.draft.name = "Saved metadata before clip review".into();
    app.draft.tempo = "121.5".into();
    app.request(Action::Clip(ClipAction::ReviewLatest));
    assert!(app.pending.is_some());
    click(&ctx, &mut app, "Save and continue");
    let save = peer.requests.try_recv().unwrap();
    assert!(matches!(edit(&save).scope, EditScope::Project));
    let EditOperation::UpdateSession { name, tempo } = &edit(&save).edit else {
        panic!("The dialog must save metadata, not replay the suspended clip")
    };
    assert_eq!(name, "Saved metadata before clip review");
    assert_eq!(*tempo, 121.5);
    assert_eq!(edit(&save).expected_revision, 0);
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, exact);
    assert_eq!(app.clips.failure.as_ref().unwrap().draft, attempted);
    assert!(app.pending.is_none());
    assert!(app.busy());
    assert!(
        peer.requests.try_recv().is_err(),
        "Review waits for metadata acknowledgement"
    );

    let mut saved = original.clone();
    saved.rename("Saved metadata before clip review").unwrap();
    saved.set_tempo(121.5).unwrap();
    let saved = saved.with_revision(1).unwrap();
    answer(&mut app, &peer, &save, Ok(reply(saved.clone(), false)));
    assert_eq!(app.document.as_ref().unwrap(), &saved);
    let open = peer.requests.try_recv().unwrap();
    assert!(matches!(&open.command, Command::Open { project_id } if project_id.0 == original.id()));
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, exact);
    assert!(
        peer.requests.try_recv().is_err(),
        "Review must not submit a clip edit"
    );

    let reviewed = saved.with_revision(4).unwrap();
    answer(&mut app, &peer, &open, Ok(reply(reviewed, false)));
    assert_eq!(app.clips.failure.as_ref().unwrap().intent, exact);
    assert_eq!(app.clips.failure.as_ref().unwrap().draft, attempted);
    assert!(peer.requests.try_recv().is_err());
    app.request(Action::Clip(ClipAction::NewChoice));
    assert!(app.clips.failure.is_none());
    assert_eq!(app.clips.draft.as_ref().unwrap().base_revision, 4);
    let ClipDraftFields::LegacyAudio(fields) = &app.clips.draft.as_ref().unwrap().fields else {
        panic!("Expected retained legacy clip fields")
    };
    assert_eq!(fields.start_tick.mode, FieldMode::Set);
    assert_eq!(fields.start_tick.text, "23");
    assert!(
        peer.requests.try_recv().is_err(),
        "A new choice still requires explicit Save"
    );
    app.request(Action::Clip(ClipAction::Save));
    let next = peer.requests.try_recv().unwrap();
    assert_ne!(edit(&next).operation_id, exact.operation_id);
    assert_eq!(edit(&next).expected_revision, 4);
    assert!(matches!(edit(&next).edit, EditOperation::UpdateClip { .. }));
}

#[test]
fn metadata_save_before_note_review_uses_the_same_modal_without_replaying_the_rejected_note() {
    let (ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    app.selected_track = 0;
    app.notes
        .select(
            &original,
            NoteSelection {
                project_id: original.id().into(),
                address: DocumentNoteAddress {
                    track_id: "note-track".into(),
                    note_id: "note".into(),
                },
            },
        )
        .unwrap();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    app.request(Action::Note(NoteAction::Save));
    let failed = peer.requests.try_recv().unwrap();
    let exact = edit(&failed).clone();
    let attempted = app.notes.draft.clone();
    answer(&mut app, &peer, &failed, Err(known_conflict()));
    app.request(Action::Note(NoteAction::Suspend));
    assert!(app.notes.new_choice(&original).is_err());
    app.draft.name = "Saved metadata before note review".into();
    app.request(Action::Note(NoteAction::ReviewLatest));
    click(&ctx, &mut app, "Save and continue");
    let save = peer.requests.try_recv().unwrap();
    assert!(
        matches!(&edit(&save).edit, EditOperation::UpdateSession { name, .. } if name == "Saved metadata before note review")
    );
    assert_eq!(app.notes.failure.as_ref().unwrap().intent, exact);
    assert_eq!(app.notes.failure.as_ref().unwrap().draft, attempted);
    assert!(app.pending.is_none());
    assert!(peer.requests.try_recv().is_err());

    let mut saved = original.clone();
    saved.rename("Saved metadata before note review").unwrap();
    let saved = saved.with_revision(1).unwrap();
    answer(&mut app, &peer, &save, Ok(reply(saved.clone(), false)));
    let open = peer.requests.try_recv().unwrap();
    assert!(matches!(&open.command, Command::Open { project_id } if project_id.0 == original.id()));
    assert_eq!(app.notes.failure.as_ref().unwrap().intent, exact);
    assert!(peer.requests.try_recv().is_err());
    answer(
        &mut app,
        &peer,
        &open,
        Ok(reply(saved.with_revision(5).unwrap(), false)),
    );
    app.request(Action::Note(NoteAction::NewChoice));
    assert!(app.notes.failure.is_none());
    assert_eq!(app.notes.draft.as_ref().unwrap().base_revision, 5);
    assert_eq!(app.notes.draft.as_ref().unwrap().velocity, "0");
    assert!(
        peer.requests.try_recv().is_err(),
        "No automatic note submission follows review"
    );
    app.request(Action::Note(NoteAction::Save));
    let next = peer.requests.try_recv().unwrap();
    assert_ne!(edit(&next).operation_id, exact.operation_id);
    assert_eq!(edit(&next).expected_revision, 5);
    assert!(matches!(edit(&next).edit, EditOperation::UpdateNote { .. }));
}

#[test]
fn direct_add_note_rejects_each_dirty_family_without_replacing_fields_then_allows_clean_local_add()
{
    for family in [DraftFamily::Metadata, DraftFamily::Note, DraftFamily::Clip] {
        let (_ctx, mut app, peer) = manual();
        let doc = app.document.as_ref().unwrap().clone();
        match family {
            DraftFamily::Metadata => app.draft.name = "Unsent metadata".into(),
            DraftFamily::Note => {
                app.notes
                    .select(
                        &doc,
                        NoteSelection {
                            project_id: doc.id().into(),
                            address: DocumentNoteAddress {
                                track_id: "note-track".into(),
                                note_id: "note".into(),
                            },
                        },
                    )
                    .unwrap();
                app.notes.draft.as_mut().unwrap().velocity = "0".into();
            }
            DraftFamily::Clip => set_start(&mut app, "23"),
        }
        let metadata = app.draft.clone();
        let notes = app.notes.context();
        let clips = app.clips.context();
        let target = NoteSelection {
            project_id: doc.id().into(),
            address: DocumentNoteAddress {
                track_id: "note-track".into(),
                note_id: "new-note".into(),
            },
        };
        assert!(
            app.dispatch(Action::Note(NoteAction::Add(target.clone())), None)
                .is_err()
        );
        assert_eq!(app.draft, metadata);
        assert_eq!(app.notes.context(), notes);
        assert_eq!(app.clips.context(), clips);
        assert_eq!(app.document.as_ref().unwrap(), &doc);
        assert!(
            peer.requests.try_recv().is_err(),
            "Add must not enqueue over a dirty family"
        );

        app.discard_active_family().unwrap();
        assert_eq!(app.dirty_family().unwrap(), None);
        app.dispatch(Action::Note(NoteAction::Add(target.clone())), None)
            .unwrap();
        assert_eq!(app.notes.selection.as_ref(), Some(&target));
        assert!(app.notes.draft.as_ref().unwrap().is_new);
        assert_eq!(app.notes.draft.as_ref().unwrap().base_revision, 0);
        assert_eq!(app.document.as_ref().unwrap(), &doc);
        assert!(
            peer.requests.try_recv().is_err(),
            "A clean Add only starts a local draft until explicit Save"
        );
    }
}

fn rejected_card_release_keeps_foreign_metadata_retry(note: bool) {
    let (ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    if note {
        app.selected_track = 0;
        app.notes
            .select(
                &original,
                NoteSelection {
                    project_id: original.id().into(),
                    address: DocumentNoteAddress {
                        track_id: "note-track".into(),
                        note_id: "note".into(),
                    },
                },
            )
            .unwrap();
        app.notes.draft.as_mut().unwrap().velocity = "0".into();
        app.request(Action::Note(NoteAction::Save));
    } else {
        set_start(&mut app, "23");
        app.request(Action::Clip(ClipAction::Save));
    }
    let rejected = peer.requests.try_recv().unwrap();
    let rejected_intent = edit(&rejected).clone();
    answer(&mut app, &peer, &rejected, Err(known_conflict()));
    app.request(if note {
        Action::Note(NoteAction::ReviewLatest)
    } else {
        Action::Clip(ClipAction::ReviewLatest)
    });
    let open = peer.requests.try_recv().unwrap();
    let reviewed = original.with_revision(2).unwrap();
    answer(&mut app, &peer, &open, Ok(reply(reviewed.clone(), false)));

    app.draft.name = "Uncertain foreign metadata save".into();
    app.draft.tempo = "131.25".into();
    app.save().unwrap();
    let metadata = peer.requests.try_recv().unwrap();
    let exact_metadata = serde_json::to_vec(edit(&metadata)).unwrap();
    assert!(matches!(
        edit(&metadata).edit,
        EditOperation::UpdateSession { .. }
    ));
    answer(
        &mut app,
        &peer,
        &metadata,
        Err(ServiceError::backend(
            "Metadata acknowledgement unknown",
            true,
        )),
    );
    let local_metadata = app.draft.clone();
    let retry = app
        .retry
        .as_ref()
        .expect("Exact metadata retry must remain available");
    let Command::Edit { intent } = &retry.command else {
        panic!("Expected metadata retry")
    };
    assert_eq!(serde_json::to_vec(intent).unwrap(), exact_metadata);
    assert!(
        app.dispatch(
            if note {
                Action::Note(NoteAction::NewChoice)
            } else {
                Action::Clip(ClipAction::NewChoice)
            },
            None
        )
        .is_err()
    );
    assert_eq!(app.draft, local_metadata);

    click(
        &ctx,
        &mut app,
        if note {
            "Release rejected note choice"
        } else {
            "Release rejected clip choice"
        },
    );
    assert_eq!(app.draft, local_metadata);
    assert!(app.metadata_dirty());
    let retry = app
        .retry
        .as_ref()
        .expect("Releasing another rejected choice must not erase a metadata intent");
    let Command::Edit { intent } = &retry.command else {
        panic!("Expected retained metadata retry")
    };
    assert_eq!(serde_json::to_vec(intent).unwrap(), exact_metadata);
    assert_ne!(intent.operation_id, rejected_intent.operation_id);
    assert!(
        peer.requests.try_recv().is_err(),
        "Release is local and does not submit either edit"
    );

    app.retry_last();
    let retry = peer
        .requests
        .try_recv()
        .expect("The preserved exact metadata retry must still be usable");
    assert_ne!(retry.id, metadata.id);
    assert_eq!(serde_json::to_vec(edit(&retry)).unwrap(), exact_metadata);
    let mut saved = reviewed;
    saved.rename("Uncertain foreign metadata save").unwrap();
    saved.set_tempo(131.25).unwrap();
    answer(
        &mut app,
        &peer,
        &retry,
        Ok(reply(saved.with_revision(3).unwrap(), true)),
    );
    assert!(!app.dirty());
    assert_eq!(app.document.as_ref().unwrap().tracks(), original.tracks());
    assert!(
        peer.requests.try_recv().is_err(),
        "Metadata replay must not re-submit a released musical edit"
    );
}

#[test]
fn releasing_reviewed_clip_rejection_preserves_an_uncertain_metadata_retry_and_all_local_metadata()
{
    rejected_card_release_keeps_foreign_metadata_retry(false);
}

#[test]
fn releasing_reviewed_note_rejection_preserves_an_uncertain_metadata_retry_and_all_local_metadata()
{
    rejected_card_release_keeps_foreign_metadata_retry(true);
}

#[test]
fn releasing_rejected_note_cannot_capture_newer_clip_context_into_an_old_metadata_retry() {
    for dirty_clip in [false, true] {
        let (ctx, mut app, peer) = manual();
        let original = app.document.as_ref().unwrap().clone();
        app.notes
            .select(
                &original,
                NoteSelection {
                    project_id: original.id().into(),
                    address: DocumentNoteAddress {
                        track_id: "note-track".into(),
                        note_id: "note".into(),
                    },
                },
            )
            .unwrap();
        app.notes.draft.as_mut().unwrap().velocity = "0".into();
        app.request(Action::Note(NoteAction::Save));
        let rejected = peer.requests.try_recv().unwrap();
        answer(&mut app, &peer, &rejected, Err(known_conflict()));
        app.request(Action::Note(NoteAction::ReviewLatest));
        let open = peer.requests.try_recv().unwrap();
        answer(
            &mut app,
            &peer,
            &open,
            Ok(reply(original.with_revision(2).unwrap(), false)),
        );
        app.draft.name = "Retained metadata intent".into();
        app.save().unwrap();
        let metadata = peer.requests.try_recv().unwrap();
        let exact_metadata = serde_json::to_vec(edit(&metadata)).unwrap();
        answer(
            &mut app,
            &peer,
            &metadata,
            Err(ServiceError::backend(
                "Metadata acknowledgement unknown",
                true,
            )),
        );

        // Inject a newer context to exercise the defensive completion guard.
        // Even a clean selection change must not be captured by an older retry.
        app.clips.clear_selection();
        if dirty_clip {
            let doc = app.document.as_ref().unwrap().clone();
            app.clips.select(&doc, legacy(&doc)).unwrap();
            set_start(&mut app, "29");
        }
        let newer_clip = app.clips.context();
        let newer_metadata = app.draft.clone();
        click(&ctx, &mut app, "Release rejected note choice");
        assert!(app.notes.failure.is_none());
        assert_eq!(app.clips.context(), newer_clip);
        let retained = app
            .retry
            .as_ref()
            .expect("Keep exact metadata evidence even when its context no longer matches");
        let Command::Edit { intent } = &retained.command else {
            panic!("Expected metadata retry")
        };
        assert_eq!(serde_json::to_vec(intent).unwrap(), exact_metadata);
        app.retry_last();
        assert!(
            peer.requests.try_recv().is_err(),
            "Release must not authorize an old result to overwrite newer clip state"
        );
        assert_eq!(app.clips.context(), newer_clip);
        assert_eq!(app.draft, newer_metadata);
        assert_eq!(app.document.as_ref().unwrap().revision(), 2);
    }
}

//! Independent note-editor state and real-egui workflow regressions.
//! All storage completions and audio ports are synthetic; no device or catalog is opened.
use super::*;
use omniusong_core::{DocumentNoteAddress, NoteTimelinePolicy, NoteValues};
use omniusong_session::{Request, ServiceError, TestPeer};
use serde_json::json;

fn document() -> ProjectDocument {
    let mut value = ProjectDocument::new("Independent note UI review")
        .unwrap()
        .value()
        .clone();
    value["tracks"].as_array_mut().unwrap().truncate(1);
    value["tracks"][0]["id"] = "\0🎹/track".into();
    value["tracks"][0]["notes"] = json!([
        {"id":"", "pitch":60,"startTick":17,"durationTicks":961,"velocity16":32769,
         "expression":null,"input":{"sourceId":"original-keys","sourceName":"Original keys","protocol":"midi2","group":15,"channel":15,"receivedTimeMs":1.25,"releaseVelocity16":60001,"keyDurationTicks":300},
         "vendor":{"id":"unrelated","opaque":"DO_NOT_RENDER_OPAQUE"}},
        {"id":"same-pitch", "pitch":60,"startTick":17,"durationTicks":961,"velocity16":7,"expression":[]}
    ]);
    let mut other_track = value["tracks"][0].clone();
    other_track["id"] = "other-track".into();
    value["tracks"].as_array_mut().unwrap().push(other_track);
    ProjectDocument::from_value(value).unwrap()
}
fn selected(doc: &ProjectDocument, track: &str, note: &str) -> NoteSelection {
    NoteSelection {
        project_id: doc.id().into(),
        address: DocumentNoteAddress {
            track_id: track.into(),
            note_id: note.into(),
        },
    }
}
fn first(doc: &ProjectDocument) -> NoteSelection {
    selected(doc, "\0🎹/track", "")
}
fn manual() -> (egui::Context, StudioApp, TestPeer) {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let (service, peer) = SessionService::test_pair();
    let mut app = StudioApp::new(Paths {
        native: PathBuf::from("/synthetic/independent-note-ui/native"),
        legacy: PathBuf::from("/synthetic/independent-note-ui/legacy"),
    });
    app.service = Some(service);
    app.ready = true;
    app.install_document(DocumentView::prepare(document()).unwrap());
    let target = first(app.document.as_ref().unwrap());
    app.notes
        .select(app.document.as_ref().unwrap(), target)
        .unwrap();
    (ctx, app, peer)
}
fn frame(ctx: &egui::Context, app: &mut StudioApp, events: Vec<egui::Event>) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1200.0),
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
        .find(|(_, node)| node.label() == Some(label))
        .unwrap_or_else(|| {
            panic!(
                "Missing {label}; labels={:?}",
                tree.nodes
                    .iter()
                    .filter_map(|(_, node)| node.label())
                    .collect::<Vec<_>>()
            )
        })
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
fn document_reply(document: ProjectDocument, replayed: bool) -> Reply {
    Reply::Document {
        view: DocumentView::prepare(document).unwrap(),
        library: Library::default(),
        replayed,
    }
}
fn edit(request: &Request) -> &ProjectEditIntent {
    let Command::Edit { intent } = &request.command else {
        panic!("Expected edit, got {:?}", request.command)
    };
    intent
}
fn changed(doc: &ProjectDocument, intent: &ProjectEditIntent, revision: u64) -> ProjectDocument {
    let EditScope::DocumentNote { address } = &intent.scope else {
        panic!("Expected document note")
    };
    let result = match intent.edit {
        EditOperation::UpdateNote { values, timeline } => {
            doc.update_note(address, values, timeline)
        }
        EditOperation::AddNote { values, timeline } => doc.add_note(address, values, timeline),
        EditOperation::DeleteNote => doc.delete_note(address),
        EditOperation::RestoreNote { timeline } => doc.restore_note(address, timeline),
        _ => panic!("Expected note edit"),
    };
    result.unwrap().with_revision(revision).unwrap()
}

#[test]
fn exact_legacy_selection_survives_reorder_tombstone_and_undo_without_neighbor_retarget() {
    let doc = document();
    let original = doc.to_json().unwrap();
    let mut editor = NoteEditor::default();
    let identity = first(&doc);
    editor.select(&doc, identity.clone()).unwrap();
    assert_eq!(editor.draft.as_ref().unwrap().velocity, "32769");
    let mut reordered = doc.value().clone();
    reordered["tracks"][0]["notes"]
        .as_array_mut()
        .unwrap()
        .reverse();
    reordered["tracks"].as_array_mut().unwrap().reverse();
    let reordered = ProjectDocument::from_value(reordered)
        .unwrap()
        .with_revision(1)
        .unwrap();
    editor.document_installed(&reordered);
    assert_eq!(editor.selection.as_ref(), Some(&identity));
    assert_eq!(editor.draft.as_ref().unwrap().velocity, "32769");
    let deleted = reordered
        .delete_note(&identity.address)
        .unwrap()
        .with_revision(2)
        .unwrap();
    editor.document_installed(&deleted);
    assert!(editor.draft.is_none());
    assert!(note_ui::note(&deleted, &identity).is_none());
    editor.document_installed(&reordered.with_revision(3).unwrap());
    assert_eq!(editor.selection.as_ref(), Some(&identity));
    assert_eq!(editor.draft.as_ref().unwrap().selection.address.note_id, "");
    assert_eq!(doc.to_json().unwrap(), original);
    assert!(note_ui::display_key("\0🎹/track").starts_with("\\0"));
}

#[test]
fn integer_drafts_retain_invalid_text_and_preserve_zero_without_document_mutation() {
    let doc = document();
    let before = doc.to_json().unwrap();
    let mut editor = NoteEditor::default();
    editor.select(&doc, first(&doc)).unwrap();
    for invalid in ["", "-1", "1.5", " 7", "65536", "18446744073709551616"] {
        editor.draft.as_mut().unwrap().velocity = invalid.into();
        assert!(editor.intent(&doc, &NoteAction::Save).is_err());
        assert_eq!(editor.draft.as_ref().unwrap().velocity, invalid);
    }
    editor.draft.as_mut().unwrap().velocity = "0".into();
    let intent = editor.intent(&doc, &NoteAction::Save).unwrap();
    assert!(matches!(
        intent.edit,
        EditOperation::UpdateNote {
            values: NoteValues { velocity16: 0, .. },
            timeline: NoteTimelinePolicy::PreserveLoop
        }
    ));
    assert_eq!(doc.to_json().unwrap(), before);
    editor.discard(&doc);
    assert!(!editor.dirty());
    assert!(editor.intent(&doc, &NoteAction::Save).is_err());
}

#[test]
fn rejected_choice_keeps_retained_draft_when_reviewed_target_disappeared() {
    let doc = document();
    let target = first(&doc);
    let mut editor = NoteEditor::default();
    editor.select(&doc, target.clone()).unwrap();
    editor.draft.as_mut().unwrap().velocity = "0".into();
    let intent = editor.intent(&doc, &NoteAction::Save).unwrap();
    editor.failed(intent.clone(), "Known conflict".into(), true);
    editor.suspend();
    let latest = doc
        .delete_note(&target.address)
        .unwrap()
        .with_revision(1)
        .unwrap();
    editor.document_installed(&latest);
    assert!(editor.new_choice(&latest).is_err());
    let retained = editor
        .failure
        .as_ref()
        .expect("Missing target must not consume retained local evidence");
    assert_eq!(retained.intent, intent);
    assert_eq!(retained.draft.as_ref().unwrap().velocity, "0");
}

#[test]
fn stale_success_keeps_newer_fields_exact_attempt_evidence_and_cancels_continuation() {
    let (_ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    app.dispatch(
        Action::Note(NoteAction::Save),
        Some(Action::Open("other-session".into())),
    )
    .unwrap();
    let request = peer.requests.try_recv().unwrap();
    let intent = edit(&request).clone();
    // Adversarial local state change while the worker holds an accepted request.
    app.notes.draft.as_mut().unwrap().velocity = "99".into();
    answer(
        &mut app,
        &peer,
        &request,
        Ok(document_reply(changed(&original, &intent, 1), false)),
    );
    assert_eq!(app.document.as_ref().unwrap().revision(), 0);
    assert_eq!(app.notes.draft.as_ref().unwrap().velocity, "99");
    let failure = app.notes.failure.as_ref().unwrap();
    assert_eq!(failure.intent, intent);
    assert_eq!(
        failure.draft.as_ref().unwrap().velocity,
        "0",
        "Card must keep attempted values separately from newer active draft"
    );
    assert!(
        peer.requests.try_recv().is_err(),
        "Stale success must not execute deferred navigation"
    );
    assert!(app.pending.is_none());
}

#[test]
fn uncertain_edit_survives_navigation_and_retries_same_bytes_against_fresh_context() {
    let (_ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    app.dispatch(Action::Note(NoteAction::Save), None).unwrap();
    let first = peer.requests.try_recv().unwrap();
    let intent = edit(&first).clone();
    let exact = serde_json::to_vec(&intent).unwrap();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError::backend("Lost acknowledgement", true)),
    );
    assert!(!app.notes.failure.as_ref().unwrap().known_rejected);
    app.execute(Action::Note(NoteAction::Suspend));
    assert!(!app.notes.dirty());
    let other = ProjectDocument::new("Other saved session").unwrap();
    app.request(Action::Open(other.id().into()));
    let open = peer.requests.try_recv().unwrap();
    answer(&mut app, &peer, &open, Ok(document_reply(other, false)));
    assert_eq!(app.notes.failure.as_ref().unwrap().intent, intent);
    app.request(Action::Refresh);
    let refresh = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &refresh,
        Ok(Reply::Library(Library::default())),
    );
    assert_eq!(app.notes.failure.as_ref().unwrap().intent, intent);
    app.request(Action::Open(original.id().into()));
    let reopen = peer.requests.try_recv().unwrap();
    let mut latest = changed(&original, &intent, 7);
    latest.rename("Later saved work").unwrap();
    answer(
        &mut app,
        &peer,
        &reopen,
        Ok(document_reply(latest.clone(), false)),
    );
    assert!(
        peer.requests.try_recv().is_err(),
        "Read/navigation must not retry"
    );
    app.execute(Action::Note(NoteAction::Retry));
    let retry = peer.requests.try_recv().unwrap();
    assert_ne!(first.id, retry.id);
    assert_eq!(serde_json::to_vec(edit(&retry)).unwrap(), exact);
    assert_eq!(edit(&retry).expected_revision, 0);
    answer(&mut app, &peer, &retry, Ok(document_reply(latest, true)));
    assert!(app.notes.failure.is_none());
    assert_eq!(app.document.as_ref().unwrap().revision(), 7);
    assert_eq!(app.document.as_ref().unwrap().name(), "Later saved work");
    assert!(!app.dirty());
}

#[test]
fn known_conflict_requires_explicit_review_and_new_save_without_automatic_rebase() {
    let (_ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    app.dispatch(Action::Note(NoteAction::Save), None).unwrap();
    let first = peer.requests.try_recv().unwrap();
    let original_intent = edit(&first).clone();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError {
            kind: FailureKind::RevisionConflict,
            message: "Known revision mismatch".into(),
            outcome_unknown: false,
        }),
    );
    app.execute(Action::Note(NoteAction::ReviewLatest));
    let open = peer.requests.try_recv().unwrap();
    assert!(
        matches!(open.command, Command::Open { .. }),
        "Review must not save stale draft first"
    );
    answer(
        &mut app,
        &peer,
        &open,
        Ok(document_reply(original.with_revision(4).unwrap(), false)),
    );
    assert!(peer.requests.try_recv().is_err());
    app.execute(Action::Note(NoteAction::NewChoice));
    assert!(
        peer.requests.try_recv().is_err(),
        "Preparing new choice does not submit it"
    );
    assert_eq!(app.notes.draft.as_ref().unwrap().velocity, "0");
    app.dispatch(Action::Note(NoteAction::Save), None).unwrap();
    let new_request = peer.requests.try_recv().unwrap();
    assert_ne!(
        edit(&new_request).operation_id,
        original_intent.operation_id
    );
    assert_eq!(edit(&new_request).expected_revision, 4);
    assert_eq!(edit(&new_request).scope, original_intent.scope);
    assert_eq!(edit(&new_request).edit, original_intent.edit);
}

#[test]
fn busy_read_only_close_requires_explicit_discard_and_never_waits_for_worker() {
    let (ctx, mut app, peer) = manual();
    app.notes.draft.as_mut().unwrap().velocity = "".into();
    let retained = app.notes.draft.clone();
    app.request(Action::Refresh);
    let held = peer.requests.try_recv().unwrap();
    assert!(matches!(held.command, Command::Refresh));
    app.request(Action::Close);
    assert!(!app.closing);
    assert_eq!(app.notes.draft, retained);
    assert!(app.pending.is_some() || app.note_close_requested);
    // Paint remains responsive; invalid input does not block explicit discard.
    for _ in 0..3 {
        assert!(!frame(&ctx, &mut app, vec![]).shapes.is_empty());
    }
    click(&ctx, &mut app, "Discard and continue");
    assert!(app.closing);
    assert!(
        app.busy(),
        "Close must not join or fake completion of the held read"
    );
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn native_close_event_cancels_close_for_unsent_note_even_with_held_read() {
    let (ctx, mut app, peer) = manual();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    let draft = app.notes.draft.clone();
    app.request(Action::Refresh);
    let _held = peer.requests.try_recv().unwrap();
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 1200.0),
        )),
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    let mut output = ctx.run_ui(input, |ui| app.native_ui(ui));
    output.textures_delta.clear();
    assert!(
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .unwrap()
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
    assert!(!app.closing);
    assert_eq!(app.notes.draft, draft);
    assert!(matches!(app.pending, Some(Action::Close)));
    assert!(app.busy());
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn save_before_delete_binds_exact_address_to_acknowledged_new_revision() {
    let (ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    let target = first(&original);
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    app.request(Action::Note(NoteAction::Delete(target.clone())));
    assert!(app.pending.is_some());
    assert!(peer.requests.try_recv().is_err());
    click(&ctx, &mut app, "Save and continue");
    let save = peer.requests.try_recv().unwrap();
    let saved = changed(&original, edit(&save), 1);
    answer(
        &mut app,
        &peer,
        &save,
        Ok(document_reply(saved.clone(), false)),
    );
    let delete = peer.requests.try_recv().unwrap();
    assert_eq!(edit(&delete).expected_revision, 1);
    assert_eq!(
        edit(&delete).scope,
        EditScope::DocumentNote {
            address: target.address
        }
    );
    assert!(matches!(edit(&delete).edit, EditOperation::DeleteNote));
    answer(
        &mut app,
        &peer,
        &delete,
        Ok(document_reply(changed(&saved, edit(&delete), 2), false)),
    );
    assert!(app.notes.draft.is_none());
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn new_note_identity_survives_exact_retry_and_discard_never_submits_delete() {
    let (_ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    let identity = selected(&original, "\0🎹/track", &uuid::Uuid::new_v4().to_string());
    app.request(Action::Note(NoteAction::Add(identity.clone())));
    assert!(app.notes.draft.as_ref().unwrap().is_new);
    assert!(peer.requests.try_recv().is_err());
    app.request(Action::Note(NoteAction::Discard));
    assert!(!app.notes.dirty());
    assert!(peer.requests.try_recv().is_err());
    app.request(Action::Note(NoteAction::Add(identity.clone())));
    app.dispatch(Action::Note(NoteAction::Save), None).unwrap();
    let first = peer.requests.try_recv().unwrap();
    assert!(matches!(edit(&first).edit, EditOperation::AddNote { .. }));
    assert_eq!(
        edit(&first).scope,
        EditScope::DocumentNote {
            address: identity.address
        }
    );
    let wire = serde_json::to_vec(edit(&first)).unwrap();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError::backend("Unknown Add outcome", true)),
    );
    app.execute(Action::Note(NoteAction::Retry));
    let retry = peer.requests.try_recv().unwrap();
    assert_ne!(first.id, retry.id);
    assert_eq!(serde_json::to_vec(edit(&retry)).unwrap(), wire);
    answer(
        &mut app,
        &peer,
        &retry,
        Ok(document_reply(changed(&original, edit(&retry), 1), true)),
    );
    assert!(!app.notes.draft.as_ref().unwrap().is_new);
    assert_eq!(
        app.document.as_ref().unwrap().tracks()[0]["notes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn retry_queue_rejection_does_not_downgrade_earlier_uncertainty() {
    let doc = document();
    let mut editor = NoteEditor::default();
    editor.select(&doc, first(&doc)).unwrap();
    editor.draft.as_mut().unwrap().velocity = "0".into();
    let intent = editor.intent(&doc, &NoteAction::Save).unwrap();
    editor.failed(intent.clone(), "Possibly committed".into(), false);
    editor.not_admitted(intent.clone(), "Local queue unavailable".into());
    let failure = editor.failure.as_ref().unwrap();
    assert!(!failure.known_rejected);
    assert_eq!(failure.intent, intent);
    editor.suspend();
    editor.document_installed(&doc.with_revision(1).unwrap());
    assert!(editor.new_choice(&doc.with_revision(1).unwrap()).is_err());
    assert!(editor.failure.is_some());
}

#[test]
fn suspended_uncertain_card_still_requires_explicit_close_choice() {
    let (ctx, mut app, peer) = manual();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    app.dispatch(Action::Note(NoteAction::Save), None).unwrap();
    let first = peer.requests.try_recv().unwrap();
    let intent = edit(&first).clone();
    answer(
        &mut app,
        &peer,
        &first,
        Err(ServiceError::backend("Uncertain edit", true)),
    );
    app.execute(Action::Note(NoteAction::Suspend));
    assert!(!app.dirty());
    app.request(Action::Close);
    assert!(!app.closing);
    assert!(app.note_close_requested);
    click(&ctx, &mut app, "Keep note draft window open");
    assert!(!app.closing);
    assert_eq!(app.notes.failure.as_ref().unwrap().intent, intent);
    app.request(Action::Close);
    click(
        &ctx,
        &mut app,
        "Discard local note card and close without waiting",
    );
    assert!(app.closing);
    assert!(
        peer.requests.try_recv().is_err(),
        "Close does not retry or cancel durable work"
    );
}

#[test]
fn selection_cancel_preserves_draft_and_does_not_leave_hidden_navigation() {
    let (ctx, mut app, peer) = manual();
    let original = app.document.as_ref().unwrap().clone();
    let old_selection = app.notes.selection.clone();
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    let other = selected(&original, "\0🎹/track", "same-pitch");
    app.request(Action::Note(NoteAction::Select(other.clone())));
    click(&ctx, &mut app, "Cancel");
    assert!(app.pending.is_none());
    assert_eq!(app.notes.selection, old_selection);
    assert_eq!(app.notes.draft.as_ref().unwrap().velocity, "0");
    app.dispatch(Action::Note(NoteAction::Save), None).unwrap();
    let save = peer.requests.try_recv().unwrap();
    answer(
        &mut app,
        &peer,
        &save,
        Ok(document_reply(changed(&original, edit(&save), 1), false)),
    );
    assert_eq!(app.notes.selection, old_selection);
    assert!(peer.requests.try_recv().is_err());
    app.draft.name = "Unsaved metadata".into();
    app.request(Action::Note(NoteAction::Select(other)));
    assert!(app.metadata_dirty());
    assert!(!app.notes.dirty());
    assert!(peer.requests.try_recv().is_err());
    app.notes.draft.as_mut().unwrap().velocity = "8".into();
    assert!(app.dispatch(Action::Note(NoteAction::Save), None).is_err());
    assert!(
        app.dispatch(Action::Save, None).is_err(),
        "Neither family may silently save the other"
    );
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn dirty_note_blocks_audio_and_existing_audio_or_attachment_blocks_note_submission() {
    use audio_ui::test_support as audio;
    use omniusong_engine::{EngineCommand, EngineResult};
    let (ctx, mut app, peer) = manual();
    let (audio_ui, audio_peer) = audio::pair();
    app.audio = audio_ui;
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    for label in ["Play saved session", "Record solo", "Start live audition"] {
        click(&ctx, &mut app, label);
    }
    assert!(audio_peer.command().is_none());
    app.notes.discard(app.document.as_ref().unwrap());
    click(&ctx, &mut app, "Play saved session");
    assert!(matches!(
        audio_peer.command(),
        Some(EngineCommand::StartPlayback { .. })
    ));
    assert!(app.audio.blocks_session());
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    assert!(app.dispatch(Action::Note(NoteAction::Save), None).is_err());
    assert!(peer.requests.try_recv().is_err());
    click(&ctx, &mut app, "Stop audio");
    assert!(
        audio_peer.command().is_none(),
        "Playback Stop waits behind the admitted StartPlayback acknowledgement"
    );
    audio_peer.reply(Ok(EngineResult::Playing(audio::status("playing"))));
    app.poll_audio();
    assert!(
        matches!(audio_peer.command(), Some(EngineCommand::Stop)),
        "Draft guard must not discard the queued playback Stop"
    );
    app.poll_audio();
    assert!(audio_peer.command().is_none(), "Exactly one Stop is sent");

    let (_ctx, mut app, peer) = manual();
    let id = app.document.as_ref().unwrap().id().to_owned();
    app.recovery.scanned(recovery_ui::test_support::scan());
    app.recovery
        .inspected(recovery_ui::test_support::inspection(true))
        .unwrap();
    let choice = recovery_ui::test_support::attach(&app.recovery, &id);
    let RecoveryAction::ChooseAttachment(choice) = choice else {
        panic!("Expected attachment choice")
    };
    let attachment = app.recovery.prepare_attachment(&choice, &id, 0).unwrap();
    app.recovery.started(&attachment);
    assert!(app.recovery.blocks_session());
    app.notes.draft.as_mut().unwrap().velocity = "0".into();
    assert!(app.dispatch(Action::Note(NoteAction::Save), None).is_err());
    assert!(peer.requests.try_recv().is_err());
}

#[test]
fn locked_or_opaque_non_note_tracks_never_produce_edit_intents() {
    for (kind, locked) in [("notes", true), ("audio", false)] {
        let mut value = document().value().clone();
        value["tracks"][0]["kind"] = kind.into();
        value["tracks"][0]["locked"] = locked.into();
        let doc = ProjectDocument::from_value(value).unwrap();
        let target = first(&doc);
        let mut editor = NoteEditor::default();
        if kind == "notes" {
            editor.select(&doc, target.clone()).unwrap();
            editor.draft.as_mut().unwrap().velocity = "0".into();
            assert!(editor.intent(&doc, &NoteAction::Save).is_err());
        } else {
            assert!(editor.select(&doc, target.clone()).is_err());
        }
        assert!(
            editor
                .intent(&doc, &NoteAction::Delete(target.clone()))
                .is_err()
        );
        assert!(
            editor
                .intent(
                    &doc,
                    &NoteAction::Restore(target, NoteTimelinePolicy::PreserveLoop)
                )
                .is_err()
        );
    }
}

#[test]
fn source_projection_bounds_lanes_points_and_never_serializes_opaque_metadata() {
    let mut value = document().value()["tracks"][0]["notes"][0].clone();
    let points: Vec<_> = (0..128)
        .map(|tick| json!({"tick":tick,"value32":tick}))
        .collect();
    value["expression"] = json!(
        (0..12)
            .map(|_| json!({"type":"pressure","points":points}))
            .collect::<Vec<_>>()
    );
    value["vendor"]["opaque"] = "DO_NOT_RENDER_OPAQUE".repeat(12_000).into();
    value["input"]["sourceName"] = "Known name ".repeat(10_000).into();
    let before = value.clone();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    let render = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 6000.0),
                )),
                events,
                ..Default::default()
            },
            |ui| note_ui::source_details(ui, &value),
        );
        output.textures_delta.clear();
        output
    };
    let tree = render(vec![]).platform_output.accesskit_update.unwrap();
    let target = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Original and expression details"))
        .unwrap()
        .0;
    let _ = render(vec![egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: tree.tree_id,
            target_node: target,
            data: None,
        },
    )]);
    let tree = render(vec![]).platform_output.accesskit_update.unwrap();
    let labels: Vec<_> = tree
        .nodes
        .iter()
        .filter_map(|(_, node)| node.value().or_else(|| node.label()))
        .collect();
    // egui exposes each static Label and its TextRun child. Count displayed
    // labels once, but scan both roles below for accidental opaque content.
    let displayed: Vec<_> = tree
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == egui::accesskit::Role::Label)
        .filter_map(|(_, node)| node.value().or_else(|| node.label()))
        .collect();
    assert_eq!(
        displayed
            .iter()
            .filter(|label| label.starts_with("Lane "))
            .count(),
        8
    );
    assert_eq!(
        displayed
            .iter()
            .filter(|label| label.starts_with("tick "))
            .count(),
        64
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("64 of 1536 points shown"))
    );
    assert!(
        !labels
            .iter()
            .any(|label| label.contains("DO_NOT_RENDER_OPAQUE"))
    );
    assert!(
        labels.iter().all(|label| label.len() < 512),
        "Known source strings must also be bounded"
    );
    assert_eq!(value, before);
}

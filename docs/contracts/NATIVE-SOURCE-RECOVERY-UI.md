# Native copied-performance recovery controls

The Rust native **Recording recovery** window exposes the retained-source backend through explicit choices. This interface preserves the original recording, the immutable physical evidence image, the recovered working copy and all retained action revisions. A recovered checkpoint has an unknown final tail. It is not a normal Finish or an editable-note projection.

The storage contracts remain [Prepared source actions](NATIVE-PERFORMANCE-SOURCE-ACTIONS.md), [execution](NATIVE-SOURCE-ACTION-EXECUTION.md) and [attachment](NATIVE-SOURCE-ATTACHMENT.md). The [integration plan](../reviews/RUST-PERFORMANCE-ACTION-UI-PLAN.md) records the review and correlation design. No migration or recovery algorithm is added by these controls.

## Explicit workflow

1. In Saved performances, scan and inspect the exact recording. Open Recording recovery and explicitly refresh its existing choices. An existing action for the same stage/take directs you to that retained action.
2. Review a copied recovery. Confirming saves one immutable Prepared action and exact review bytes. Recovery has not started.
3. Read the saved action if its context changed, then review execution. A separate confirmation requests the evidence copy and recovered checkpoint. Partial or complete files can remain after an error or cancellation.
4. After Verified, explicitly Keep locally or Return to review. These durable disposition choices do not delete material, release a recording intent or settle a pending attachment.
5. To publish, enter a new recording-track name and an integral, nonnegative timeline frame in the currently open saved session. Zero must be entered explicitly. Schema-1 sessions require visible consent to their native-audio upgrade. Review and confirm the immutable attachment choice, then separately review and confirm adding that saved choice to the session.

Initial publication uses the full selected audio checkpoint at its retained sample rate. There is no automatic resampling, inferred alignment, extra tail, note conversion or fade edit. All target, range, track, clip and operation identities are frozen when the choice is saved.

## Restart, checks and conflict

Restart never resumes a copy or publication. Explicitly refresh and read an exact retained action. Reading uses catalog metadata and retained review bytes; it does not reopen the recording or authenticate current copy files.

**Check existing recovery** authenticates existing evidence and working artifacts. It does not create a second copy or append Verified. A recorded Started action with complete matching artifacts requires a separate **Review recording existing result** confirmation to append Verified. Missing or changed artifacts cannot fall back to the original recording.

**Check attachment result** reads historical commitment separately from its optional current session view or error. A committed receipt survives Undo and subsequent edits; it does not mean the clip is still placed. Read-only reconciliation never installs a document.

A saved attachment with an old target revision may report a conflict. Check its exact result and explicitly retire the uncommitted choice. Only successful atomic retirement plus a fresh detail read permits a new choice, with fresh identities and the exact retired predecessor. Owned bytes may already remain after conflict. Failed or uncertain retirement grants no replacement authority.

## Native guards and bounded presentation

All operations share the single session worker. Metadata List/Read preserve musical drafts and can run during audio; artifact work requires released audio and resolved capture ownership. Mutation requires clean musical drafts, no retained failed edit/navigation/close action, initialized canonical storage and a current exact review. Source, project, draft, track, worker, consent and placement changes invalidate confirmations; no automatic Save, retry, Refresh, recording or attachment follows.

Request IDs and exact historical reference triples are checked before releasing credit. A mismatched reference keeps credit. A correlated malformed body becomes protocol uncertainty, retaining its exact mutation identity without installing a document. The model's complete validated acceptance is required before an explicit Attach can install the independently loaded current view; stale success remains historical.

Cancellation addresses only the exact pending source-action token, retains worker credit until terminal delivery and is not rollback. Storage loss retains known primary results, retires the generation and requires explicit reconnect. A long nested current-document error is cropped for presentation without losing the primary commitment or retirement kind.

The list has at most 32 states and 576 KiB; a state is bounded to 64 KiB. Detail has at most 320 KiB including the exact review of at most 64 KiB. Eight compact observation cards, bounded messages and one independently pinned unacknowledged request prevent silent history eviction. No filesystem I/O occurs while constructing or painting the model.

Full-root OSRE archives retain these histories and copy files as inert evidence; restore does not adopt or execute them. Song OSAR, full-root OSRE, source parking, adoption and multiple placements remain separate scopes. Device acquisition, physical power loss, listening, sustained capacity and Windows need their own acceptance.

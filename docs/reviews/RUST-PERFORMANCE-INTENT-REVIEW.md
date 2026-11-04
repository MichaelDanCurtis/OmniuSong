# Durable performance intentions: independent architecture review

October 20, 2026, with the final source gate recorded October 21 UTC. Design and source review for the musician-facing output-recording packet after `3fc5570`. The current integrated source gate passes 750 tests, with no failures and three intentional ignored helpers. This review independently checked the frozen source and gate artifacts; it ran no Cargo, formatting, GUI, device, or live-catalog operation. The preceding native backend qualification does not qualify this new GUI. The disposition below accepts this bounded source implementation, not actual native UI behavior, M2 completion, or H09 usability.

The adopted split is sound: the existing `SessionService` persists a musician's exact intention before its side effect; the native engine remains the authority for real recording, callback Finish, native retirement, and normal sealing. The ledger must never convert a saved request into evidence that recording started, a source exists, or a seal succeeded. A restarted app offers retained choices for review; it does not resume recording or replay Start/Finish automatically.

## Concrete ownership and API

Keep one storage worker. The dependency direction session → engine → store is acyclic and lets the session layer validate the engine's exact `PerformanceStart`/clock types. The store keeps bounded opaque ledger bytes and CAS receipts; it must not depend on engine types or interpret UI state. Paint and input handlers use only in-memory DTOs and nonblocking submission.

The coordinator's schema-5 design is appropriate for this bounded packet: append-only `recording_intents(id, revision, payload, payload_sha256)`, immutable UPDATE/DELETE triggers, 32 distinct Start IDs, 16 revisions per ID, and 16 KiB per payload. The worst-case retained payload is 8 MiB. Limits reject new choices visibly without deletion or reuse of an old ID. Bound SQL byte lengths and types before materializing values, authenticate every historical payload/hash/revision, and reject gaps or incompatible transitions. The current bounded implementation deliberately fails closed on damaged history; per-card salvage is not delivered here.

Use the strict version-1 session payload frozen in [the UI contract](../contracts/NATIVE-PERFORMANCE-UI.md): full Start, captured project identity, deterministic journal path, optional descriptor digest, optional exact Finish, optional full `TakePublication`, and irreversible release. Start/output/GUI clock/project/path never change. Each optional value may be added once and is then immutable. Release cannot add or replace another field and is not permission to remove originals.

| API | Required behavior |
| --- | --- |
| `SavePerformanceIntent { expected_revision, intent }` | Validate shape, canonical store-root path and monotonic typed transition; append through the sole storage worker. Return authenticated revision, exact payload and digest. An exact current-payload retry returns that same revision; a different stale payload conflicts. |
| `ListPerformanceIntents` | Authenticate the bounded history and return current receipts. Perform no audio action, source repair, recovery, publication, or automatic retry. |
| `AttachPerformance { id, revision }` | Load the exact current stored publication. Reject an absent, stale, released or substituted choice. Execute that stored request only. On receipt replay, load the actual current project rather than installing the historical operation result. |
| `NativeStore::performance_attachment_committed(&TakePublication)` | Read-only historical reconciliation: false only for no operation, error for conflicting/damaged evidence, true only for the exact authenticated committed request/result and destination clip. It does not retry or open original take files. |

The last helper was explicitly delegated to this reviewer and is implemented in `crates/omniusong-store/src/performance_attachment_check.rs`; it is **not independently approved by this review**. The coordinator reports an independent source review without an outstanding finding. All six authored helper tests pass in the verified central workspace log: no-receipt/no-write behavior, later revisions and unavailable physical sources, existing-track name semantics, conflicting requests, rehashed valid contradictory results, and damaged receipts/read-snapshot cleanup. They were run centrally, not by this reviewer.

## Side-effect ordering and partial outcomes

**Start:** allocate the ID and GUI clock once, persist the full choice, and wait for the matching confirmed receipt before sending `StartPerformance`. An uncertain save retains the same bytes and expectation. A new clock or ID is a new human choice, never a retry. A `Starting` reply is not Ready. Activate the prepared key/pad controller exactly once after the authenticated real Ready, then apply newer cumulative faults before accepting input.

**Finish:** close new local notes and lease generation; flush the finite already-staged GUI events once with their original fields; persist the exact Finish ID/scope; send it only after confirmed persistence. Do not call the audition cleanup path: it clears staged events and requests Panic. Do not synthesize human NoteOffs for held voices. The native callback's recorded hard cut explains their termination. A storage stall may expire the live lease and interrupt recording; no status-based lease renewal or fabricated successful tail compensates for it. An existing pending Panic must resolve before choosing Finish; the saved scope is never rebased.

**Stop:** always bypass storage and ordinary command credit. Stop cancels the local Start/Finish continuation even if its storage write later commits. Bind every continuation to service generation, exact command/payload/revision, and the local card generation; Start ID alone is insufficient. A stale save completion must not start recording after Stop, resume notes, replace a newer card, or install a foreign project. Wait for the exact urgent Stop acknowledgement or typed actor retirement before releasing live ownership/new-Start guards.

**Attach:** construct the publication after explicit draft resolution, using the intended project's then-reviewed revision, fresh operation/track/clip IDs, and authenticated sealed audio bounds/rate. Persist the complete publication before execution. Finish never automatically publishes. Ingestion may commit before publication conflicts; that original remains valuable retained evidence. Unknown outcomes retain the full immutable request for exact retry. A successful replay returns the current project view, not the old attachment snapshot.

Release with no attachment is a local acknowledgement only. Release with an attachment requires read-only verification of its exact committed operation. A missing receipt is not proof that an unknown request failed. A known revision conflict also cannot satisfy the current release rule. Since this packet allows one immutable attachment slot and no superseding publication, **correcting a conflicted attachment choice remains unfinished**; do not present it as a complete conflict workflow. Originals and the exact old request must remain visible and retained.

Known pre-write shape, transition and CAS rejection must be distinguished narrowly from write/commit/read-back uncertainty. Do not exempt every generic `Rejected` error from the service's conservative mutation wrapper: a response projection can fail after a successful commit. SQL failure, missing acknowledgement and post-write verification failure remain potentially committed. Existing `RevisionConflict` and pure musical `EditRejected` behavior must remain intact.

The initial in-progress `session/src/lib.rs` added `FailureKind::Rejected` to the global mutation-wrapper exclusions while `DocumentView::prepare` still returned that kind after successful mutations. The coordinator corrected this before the integrated source review: only dedicated `IntentRejected` joins the existing known-pre-write exceptions. Generic post-commit `Rejected` remains uncertain. Intent history/read-back failures become Backend rather than acquiring the new exception.

## Source authenticity and restart boundary

The path is exactly the canonical opened root plus `captures/<32 lowercase hex Start ID>/take.sqlite3`; the take ID is the engine's deterministic performance ID. Before Start the path may not exist. Never trust an arbitrary persisted absolute path merely because it is under `captures`, and do not weaken generic hyphenated-UUID capture discovery to accept arbitrary names.

An attachment's name/ID alone does not prove a performance source. Initial source admission must compare the saved descriptor digest with the authenticated original, decode the strict owned v2 performance descriptor/profile, and bind its exact GUI clock and take ID to the saved Start. Prefer the already registered authenticated original when the stage has disappeared; keep committed-receipt reconciliation before source access. The initial service omission was corrected: first attachment now verifies the complete semantic checkpoint/Finish closure through `inspect_sealed_performance`, requires matching normal manifest/count/index/Finish choice, and rejects semantic faults, unresolved fault publication and recovered manifests. `has_registered_take` separates absence from failed owned-original authentication. `SealedJournal::inspect_snapshot` uses its held connection and never acquires a capture owner or creates an owner sidecar. These reads supply evidence, not authority to normally seal or resume.

On restart, list and display exact retained cards without invoking the engine. A saved Finish is not normal-seal authority in a new worker. Existing stages remain inspect-only and cannot be recreated/resumed. Performance-aware checkpoint inspection/recovery still needs its own service integration; generic recovery must continue rejecting owned performance markers. Missing descriptor/status means uncertainty, not an empty source. Do not fabricate a completed card from intent fields.

This local ledger is explicitly outside per-song archives. Attached audio references retain the whole six-stream original through the existing archive closure; unassigned stages and pending intentions need a separate portable retention/export workflow. Release or an archive operation must not imply that those local-only originals were backed up.

## App integration risks requiring tests

The existing `AudioUi::captured` path auto-attaches solo capture; recorded performance must use a separate explicit attachment route. The old audition cleanup/retirement text says nothing was recorded and clears live state; the durable recording card must survive independently and report a possibly retained prefix. `LiveUi::begin` creates a fresh clock and `activate` resets source/voice state: neither belongs on repeated status or exact retry. Existing `clear_local` discards staged input; counts/disclosure must distinguish GUI-local, parent queued, worker accepted and callback-retained originals. Until parent-discard telemetry exists, show it as unavailable/unknown, never zero.

The main audio guard must allow the exact pending intent persistence/review commands needed for Finish and release while still blocking foreign session edits. Stop remains usable during storage busy, disconnect, modals and close. Attachment needs the ordinary metadata/note/clip draft guards and exact target identity; a dedicated command bypass must not install an old result over a different draft. Close must offer an explicit interruption/retention choice rather than invoking audition Panic as a substitute for recorded Finish.

Schema 5 must remain compatible with the existing read-only waveform reader and fixtures; the coordinator confirmed `NativeReader` now follows the exported schema constant and version literals in affected examples are being removed. No old `0978` package evidence transfers to the new controls.

## Required acceptance

1. Storage/service: repeated exact CAS saves, stale differing payload, changed immutable fields, malformed/oversized history, cap exhaustion, SQL rollback and lost-ack replay. Assert unchanged project/history/source bytes for intent-only operations. Test known pre-write rejection separately from post-commit response failure.
2. Release/attachment: absent receipt refuses release; exact committed publication permits it even after later project edits; changed full request or rehashed wrong destination/result fails. Test source descriptor/profile/clock mismatch, absent stage with valid registered original, publication conflict after ingestion, and exact retry after a missing response. No release check may call attach or mutate the project.
3. UI with controlled service/engine: delayed save/Ready, Stop before a late save completion, fault before older Ready, exact one-time activation, finite staged-event Finish, lost persistence acknowledgement, stale card/service generation, busy close, retained original after actor retirement, and explicit attachment only. Reopen persisted cards without any engine command. Exercise draft guards across all three draft families.
4. Actual same-binary output-only workflow after source gates: Record → Ready → keys/pads → Finish → reliable Stop → review → explicit attachment → Undo/Redo → app restart → complete attached-original archive/restore. Independently inspect original PCM and GUI/control evidence, descriptor/Finish/manifest identity, intent rows and preserved prior catalog/prototype files. Headless tests and an enabled button do not establish H09 visual/usability acceptance.

## Integrated source review and finding disposition

The read-only review of the formatted implementation confirms the intended durable ordering: UI Start and Finish are queued only after an exact matching intent receipt; the store compares full latest bytes before CAS and verifies the complete retained row set before commit; the service normalizes CAS to the observed current revision and verifies the exact latest typed receipt after saving before returning a continuation. Stop marks the card cancelled and clears pending engine work, so a late successful Start/Finish save cannot reactivate audio. Failed submission retains the exact storage task rather than minting another ID.

`LiveUi::finish_recording_input` sets its input/lease gate before one final bounded transfer, leaves held/sustain originals intact, and uses neither cleanup nor Panic. Failed transfer requests reliable Stop; normal input and cleanup paths return while finishing. Ready activation is one-shot, status revisions are monotonic, and matching urgent Stop acknowledgement is required before a card is marked stopped. Audio retirement retains the performance card independently of cleared live controls. Attachment is explicit, and main's usual document/draft/note/clip context check preserves a newer local draft on a late document result. The performance dispatch bypass is restricted to its three owned commands, not arbitrary foreign mutations.

Two concrete integrated findings were sent to the coordinator; no code was changed by this reviewer:

- **Dirty-draft review trap — corrected and source-tested:** the first review-entry enablement used only audio state. Opening a retained unresolved attachment while a draft was dirty could block both saving/discarding that draft and retrying the attachment. Review entry now includes `can_start` (`audio_ui.rs:842`), preserving the existing clean/no-unresolved/no-modal/service-ready gate. Unreleased attachment reviews and retries require the original project; `command_target` (`performance_ui.rs:204`) binds the active matching stored choice, and main rechecks that target and every draft guard before submission (`main.rs:781`). Already-open card controls remain available. A restarted stored attachment has an explicit retry using its existing ID/revision and never starts the engine.
- **Staged WAL preflight — corrected and source-tested:** the first `guard_stage` checked ordinary confined directories/files and hardlinks, but `SealedJournal::open` opens SQLite before rejecting non-DELETE mode. The coordinator added an exact SQLite signature/rollback-header check and rejection of existing `-wal`/`-shm` before the staged file reaches SQLite (`performance_intents.rs:455` in the session crate). Read-only inspection confirms that ordering. The owner regression covering all three variants passes in the final workspace log, including unchanged source/directory bytes and no ingestion.

The added unstarted-choice escape is narrow: only an initial `IntentRejected` with no previous uncertainty, durable receipt or engine Start can dismiss the local attempt; a later known error cannot erase an earlier unknown outcome. AudioUi consumes that local dismissal to clear the pending LiveUi start. This does not release/delete a stored intention or original.

The unstarted-choice escape is at `performance_ui.rs:403`; the error exception is the dedicated `IntentRejected` at `session/src/lib.rs:408`, with the conservative global mutation wrapper retained at lines 990–1003. The source authenticity path is `session/src/performance_intents.rs:338`, `recording/src/snapshot.rs:96`, and `engine/src/performance_writer.rs:1346`. These references identify the frozen files in the following table; later edits require fresh review.

## Frozen central gate and source binding

The final receipt is `.runtime/rust-performance-ui/gate1-run.json`, recorded at `2026-10-21T05:01:44.305386+00:00`: 27,178 bytes, SHA-256 `ba20f7a737595002c5edd84ce824465a2ccfc7c7d1095ceceacb052076ee7771`. Independent read-only recomputation matched all **136 source snapshots and four log fingerprints**. Parsing the workspace log independently yields **750 passed, 0 failed, 3 intentional ignored helpers, 56 result groups**, including **194 app tests**. The completed receipt reports exit 0 for formatting, offline locked all-feature workspace tests, offline locked all-target/all-feature Clippy with warnings denied, and the offline locked default-feature all-target workspace build. The default build is complete; no package or native GUI result is inferred from it.

| Gate artifact under `.runtime/rust-performance-ui/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `gate1-fmt.log` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `gate1-workspace.log` | 83532 | `5961e43e7097d23607c0e8bfb01784926980e25a693901dbdc14188c42e5e9b5` |
| `gate1-clippy.log` | 612 | `7851e2b5dd866b422e4ba2748ada759a6627da29a770ce06cbf87652054003c1` |
| `gate1-build.log` | 613 | `fec826e4eb8e4c5cb6c49c49f4e45a8468d2e67e7cdc7cc73b416e7127de759e` |

Relevant reviewed source bindings, each included in that receipt:

| Path | Bytes | SHA-256 |
| --- | ---: | --- |
| `crates/omniusong-app/src/audio_ui.rs` | 64109 | `9a9f48966f0b7187bf81fae7bc6f220a6b3577cff9a6108f69297bd481001692` |
| `crates/omniusong-app/src/live_ui.rs` | 67735 | `6193dc78864e42aa411b2dd413b425287b8fe0c05f25f00d1e5ad147d96206b9` |
| `crates/omniusong-app/src/main.rs` | 170036 | `5a41adf83eadef22c15e4edc76d294cd778a933c8b896a990c4f1a281388bfbf` |
| `crates/omniusong-app/src/performance_ui.rs` | 58229 | `a9c23dafeb744208b4816cd3d296fd6251a758745fa4b80773e26a409e39c2f9` |
| `crates/omniusong-engine/src/performance_writer.rs` | 66080 | `1dbb395061f449012091a4e2bdf1c29a70499944b46c6d0cc02a315bdb82df54` |
| `crates/omniusong-recording/src/snapshot.rs` | 13309 | `b4faa3fb51cd5db2cb76bbfaad6d30fbd84be7a6a65e77228034cbe35d91446c` |
| `crates/omniusong-session/src/lib.rs` | 48589 | `e13eb64dcb8f59ae2606994d95049a4f98bbbaeab45a3ddb1f4c8b48a7ed774e` |
| `crates/omniusong-session/src/performance_intents.rs` | 47577 | `9a3e0ef9df0e8992c6338f2b8bfe6c80cc0b0ca6959e883ea7ef1d1a3ce98789` |
| `crates/omniusong-store/src/lib.rs` | 36006 | `3b6c342df306e75b7e6299b64d54ae26180691eb145fb4bab11c5e9e6aac8ab3` |
| `crates/omniusong-store/src/performance_attachment_check.rs` | 13422 | `9e48e6a50e50f0f2e28059a01edde830635ad0db1d0193e06ee6c0f102b15498` |
| `crates/omniusong-store/src/performance_intents.rs` | 13212 | `d8cfc422707caefe8be7001e71e64714b46c6a7d641d0e9ace2a5791cde29d07` |
| `docs/contracts/NATIVE-PERFORMANCE-UI.md` | 10005 | `93692faa037017e660516e1e613296eb0b8210b2efcf7768fd2626ad04c352cd` |

**Source disposition:** no outstanding substantive blocker remains in the inspected integrated paths after the recorded corrections; this bounded source implementation is accepted against the gate above. The self-authored attachment helper's independent review belongs to the coordinator, and its six passing central tests were verified here. Actual same-binary GUI, H09, restart and archive acceptance are not established by this source gate. Conflict correction, performance-aware recovery/discovery, verifying a reopened sealed source before a new attachment choice, unassigned-source export, finite parent-loss accounting, editable-note derivation, sustained recording and the full native-workstation goal remain explicit gaps. Retrying an already persisted attachment is distinct from creating a new choice from a reopened source.

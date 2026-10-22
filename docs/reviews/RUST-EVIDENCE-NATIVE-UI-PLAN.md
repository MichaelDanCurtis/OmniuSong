# Native OSRE controls: next integration slice

Read-only source investigation, October 22, 2026, accepted by the coordinator as the next bounded integration plan after the [qualified backend gate](../verification/rust-native-evidence-jobs-tests.json). This plan delivers no native control surface or native acceptance. The investigation made no source, application, catalog, device or package changes.

## Current integration points

- `crates/omniusong-app/src/main.rs`: `Action`, `StudioApp`, `start_service`, `submit`, `poll`, `complete`, `request`, `dispatch`, `retry_last`, `loading_ui`, and `show` own the sole service and all native admission. `submit` currently rejects every evidence command. Generic success arms merely acknowledge evidence replies; they provide no review state. `show` returns through `loading_ui` whenever `ready` is false, hiding all normal panels.
- `performance_source_ui.rs`: `SourceRequest::command`, `PerformanceSourceUi::{generation_changed,worker_lost,submitted,submission_failed,completed,cancel_requested}` provide a useful exact-request/epoch pattern. Do not mix OSRE state into saved-performance review.
- `performance_source_integration_review.rs`: `manual`, `held_drafts`, `preserved`, `no_continuation`, `answer`, and headless `frame`/`click` are the existing integration-test pattern. Its backend-command rejection matrix already contains evidence commands; keep rejection coverage while allowing only the new validated evidence route.
- `audio_ui.rs::AudioUi::blocks_session` already includes non-idle audio, retained sealed data, live control engagement, and performance capture. `main.rs` adds recovery, dirty-family, unresolved-edit, pending-choice and close guards.
- The existing session commands/replies already cover Run, Inspect, List, Read and Reconcile. `SessionService::cancel_evidence_job` accepts only exact pending Run/Inspect/Reconcile IDs and retains capacity. Nested `EvidenceJobReply.catalog_retirement` is already a service retirement signal. `spawn`/`reconnect` construct an unopened backend; automatic Initialize is a native `start_service` policy.

## Separate presentation module

Add `evidence_ui.rs`, `#[cfg(test)] evidence_integration_review.rs`, `mod` declarations, an `EvidenceUi` field on `StudioApp`, and `Action::Evidence(EvidenceUiRequest)`. No second worker, direct NativeStore handle, filesystem inspection, archive authentication, journal access, engine action or automatic command belongs in the module.

Proposed model fields:

- Exact export destination, restore archive/directory text, selected operation; immutable request prepared for confirmation, with one job ID/export ID minted only by an explicit local review action.
- `generation`, `lost_generation`, checked `view_epoch`, checked inspection/confirmation serials, and one `Pending { request, id, cancel_attempted, cancel_requested }`.
- Bounded latest archive-inspection summary plus `RestoreReview { exact_archive_path, whole_container_sha256, epoch, generation, inspection_serial }`.
- Explicit job-list snapshot, selected job ID, selected bounded `EvidenceJobView`, and bounded historical result/uncertainty cards keyed by exact job/request fingerprint. Field edits, A→B→A selection changes and generation changes invalidate confirmation rather than reviving an old review.

`EvidenceUiRequest` should encode only Inspect `{ path, epoch }`, List `{ epoch }`, Read `{ job_id, epoch }`, Run `{ exact_request, epoch, confirmation_serial }`, and Reconcile `{ job_id, expected_request_sha256, expected_history_sha256, epoch }`. Its `command()` produces the corresponding existing Command. `EvidenceAction` adds Submit, exact-ID Cancel and local review/confirm choices. `ui(ui, availability)` returns at most one action. Constructor, paint, field editing, selection, reconnect and result installation emit no request.

Use methods matching the source-review model: `new`, `generation_changed`, `worker_lost`, `run_request`, `reconcile_request`, `submitted`, `submission_failed`, `completed`, `pending_cancel`, `cancel_requested`, and pure bounded reply preparation. Reply preparation checks byte caps before cloning: public list/view/outcome bounds; archive manifest/entry bounds; strict IDs/SHA; exact lexical paths; request fingerprint; receipt operation and provenance. Virtualize at most 128 job rows; cap retained cards rather than retaining unlimited full views. Explanatory truncation must be marked; identity paths/request bytes are never shortened into a different identity.

## Exact native routing and guards

1. Add `StudioApp::handle_evidence_action`. Submit goes through `request(Action::Evidence(...))`. Cancel is urgent control of the existing pending evidence ID, action kind and command kind, via `service.cancel_evidence_job(id)`; it submits nothing and releases neither credit early.
2. Split the current blanket `submit` backend-only test: prepared source-action commands stay rejected. Evidence commands require an `Action::Evidence` whose exact generated command matches, with `after == None`, current confirmation/review or selected request/history fingerprints, and availability revalidation. Raw `Action::Performance`, Refresh, Recovery, a mismatched Evidence request, or an arbitrary continuation must not acquire credit.
3. In `request`/`dispatch`, route evidence before generic draft resolution and before generic continuations. List/Read preserve all metadata/note/clip drafts, unresolved failures, exact musical retry and its continuation, session/track/clip selection and recovery card. They need a healthy idle service, not `ready`. Inspect/Reconcile also preserve those fields but retain the existing audio/capture-owner barrier for artifact reads. No read implicitly saves/discards/opens a session.
4. A newly confirmed Run requires ready live storage, idle connected worker, no dirty draft, no unresolved musical choice/retained mutating retry, no pending save/discard navigation, no recovery attachment guard, released audio/live/capture/sealed-take ownership, and no close guard. Recheck these at dispatch and submit. A dirty guard may offer local resolution, but must invalidate the old confirmation afterward and require a fresh explicit confirmation; it cannot automatically resume the earlier Run after Save/Discard.
5. Export confirmation binds the complete immutable version-1 request, full-root scope, exact canonical origin, destination, job ID and export ID. Restore confirmation additionally requires a successful current Inspect result and binds archive path/SHA, destination, UI epoch, inspection serial and worker generation. A manually constructed or changed request cannot borrow another review. Repeated confirmation cannot mint another identity.
6. Add evidence-specific completion matching before generic `complete` arms. Wrong generation/sequence/kind/variant/identity keeps the real pending credit. A correct request whose UI epoch/selection is now old updates only its historical card; it cannot replace the current review, prepare confirmation, change the document/drafts, or run a continuation. Handle `attempted`, primary outcome, metadata persistence problem and optional catalog retirement as separate facts. Cancellation request is not rollback; preserve complete/partial uncertainty and label historical receipts as historical.
7. Handle evidence submission failure/disconnect before generic retry code in `submit` and `poll`; notify its model, preserve any preexisting musical retry, and never add evidence work to generic `retry`. `retry_last` explicitly rejects Evidence actions, including retry-safe reads; every new read/reconciliation is a fresh explicit action. Only List/Read/Inspect/Recon buttons refresh evidence.
8. On nested catalog retirement, retain the primary evidence card first, invalidate current confirmation, notify other source models of worker loss, and require an explicit fresh generation. Do not infer retirement from historical journal SQLite details or local archive parse failure.

## History when Initialize fails or the source is absent

Render the evidence/history surface in both `show` and `loading_ui`; its availability is independent of `ready`. Add an explicit metadata-only reconnect/start mode to `start_service`: replace the single disconnected service with `SessionService::reconnect(paths)` but submit no Initialize. It is the same capacity-one slot. List/Read/Reconcile then reach the existing source-independent backend paths. Do not open/create the missing NativeStore, run startup job work, or spawn a separate journal service. A normal subsequent explicit Initialize may open storage through that same worker.

One small prerequisite remains: the UI currently has `Paths.native` from CLI parsing but successful Library replies contain no canonical origin. For relative/symlink CLI roots, UI request identity and later source-absent reconnect would otherwise use the wrong bytes. Prefer returning the actual canonical native origin in Library after successful Initialize, retaining it in `StudioApp` and using it in subsequent service configuration/evidence requests. Preserve the exact already-known configured origin for metadata-only startup; never infer a replacement identity from a missing root. New Run stays disabled until a canonical origin and live initialized store are established. This is an explicit next-slice DTO change, not an implementation in this pass.

## Minimum headless gates

- Pure construction/paint/edit/select/reconnect emits no artifact or journal command; explicit List, Read and Recon each emit exactly one matching command on the existing service.
- Extend the current raw-command rejection matrix with mismatched Evidence actions, stale confirmation, path/SHA/epoch/generation changes, wrong request/history fingerprint, and continuations. Positive route accepts only the corresponding exact Evidence action.
- Every evidence read success/local failure/cancellation/disconnect preserves the full existing `Preserved` snapshot and sends no storage/audio continuation. New Run is rejected under every dirty, retained edit, pending choice, audio/live/capture/sealed-take, recovery and closing barrier, including direct dispatch bypass attempts.
- Exact cancellation is idempotent, stale tokens cannot cancel subsequent work, and both credits remain until correlated completion/disconnect. Completion before cancellation remains success with accurate labeling.
- Correct-but-stale results become historical cards only; wrong ID/kind/variant cannot consume current credit. Nested live retirement retains primary result and drafts; malformed archive leaves healthy worker usable.
- Failed Initialize/disconnected generation → explicit metadata-only fresh service → List/Read/Recon works with absent source using the retained exact origin; no Initialize, lazy store creation, automatic refresh or artifact retry is emitted.
- Headless light/dark and narrow layouts expose exact paths/SHA, explicit confirmation, cancel and uncertainty without clipping; bounded list/view/card state rejects oversized/malformed replies before installation.

## Minimum actual native qualification after source gates

Use the same built binary against an isolated current-root synthetic store and separate synthetic output paths. No native input, loopback, live user database or existing installed-app state is required.

1. Explicit Export → Inspect → review-bound inert Restore; independently read back request/start/terminal/history identity, whole archive SHA, numbered membership and complete receipt, with original catalog/raw trees preserved.
2. Change archive A to valid different B after review; Restore refuses completion for the old pin. Show local malformed-archive failure followed by a healthy explicit session/history read.
3. Exercise cancellation/lost reply/restart uncertainty; do not label cancellation as rollback. Explicitly List/Read then Reconcile the recorded job and preserve the original terminal plus appended observation.
4. Park only the isolated source root, reconnect metadata-first through the same single service and demonstrate List/Read; for restored output, also park only the isolated original archive and explicitly reconcile using numbered objects. Verify no replacement root/archive, new marker, adoption or retry was created.
5. Change path/selection during inspection and dirty local fields during reads; demonstrate stale cards and unchanged drafts, with no Run until guards and fresh confirmation are satisfied.

Capture exact source/binary hashes, command IDs/generation, bounded UI evidence and independent filesystem/catalog before/after comparisons. These checks qualify this OSRE surface only; they do not qualify hard power loss, full studio scale, adoption, recording, Windows or the rest of the workstation.

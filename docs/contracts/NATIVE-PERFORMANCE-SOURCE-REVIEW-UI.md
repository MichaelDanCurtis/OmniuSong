# Native performance source review UI

October 22, 2026. **Frozen first read-only slice; acceptance is tracked separately.** This contract defines implementation boundaries and required checks; it does not itself demonstrate implementation or recovery acceptance. Initial source inspection was against `1412c4ef8725d10e477823acda43da5db9d2bf37`. No Rust, catalog, device, build or UI execution was performed when drafting this contract.

The next musician-facing addition is **Scan saved recordings → select one → Inspect selected recording → review the observed source alongside saved requests**. Inspection has an explicit cancellation control. Every operation in this slice is read-only. It creates no recording, manifest, recovered copy, source-action receipt, adoption, publication, archive or editable notes. Those remain required later stages in the [performance recovery plan](../reviews/RUST-PERFORMANCE-RECOVERY-PLAN.md) and [implementation plan](../IMPLEMENTATION-PLAN.md).

The coordinator has completed the [bounded native sealing qualification](../verification/rust-native-sealing-ui.json), retained in commit `f55fdc2`: two actual normal seals with the optimized `c46a…` bundle, explicit attachment, archive/copy, restart and independent source/archive audits. That evidence remains separate from acceptance of this new UI. Earlier failed sources, sidecars and exact Start/Finish requests remain evidence to preserve.

## Existing boundaries to reuse

| Current source | Available behavior and constraint |
| --- | --- |
| [session `performance_sources.rs`](../../crates/omniusong-session/src/performance_sources.rs) | `ScanPerformanceSources` discovers exact 32-character lowercase nonzero hexadecimal stage IDs without opening SQLite. `InspectPerformanceSource { stage_id }` derives the owned path and returns a bounded `PerformanceSourceReview`; it does not recover or publish. |
| [session `lib.rs`](../../crates/omniusong-session/src/lib.rs) | One request credit, `RequestId { generation, sequence }`, exact completion-kind checking, nonblocking submit/poll, and `cancel_source_inspection(id)`. Cancellation holds the credit until the matching completion is consumed. |
| [session `performance_intents.rs`](../../crates/omniusong-session/src/performance_intents.rs) | `ListPerformanceIntents` authenticates all bounded history and returns current typed receipts. Start, Finish and publication fields are saved requests. Listing invokes no engine action. |
| [app `main.rs`](../../crates/omniusong-app/src/main.rs) | Owns the sole session service, `Active` correlation, reconnect, draft/navigation/close guards and integration. It currently receives performance-source replies only as generic status text and discards their contents. |
| [app `performance_ui.rs`](../../crates/omniusong-app/src/performance_ui.rs) and [audio owner](../../crates/omniusong-app/src/audio_ui.rs) | Existing recording card owns Start/Finish/attachment/release continuations. Its bounded automatic metadata listing remains separate. New source review must neither enter that card nor cause its engine continuations. |
| [app `recovery_ui.rs`](../../crates/omniusong-app/src/recovery_ui.rs), [generic store scanner](../../crates/omniusong-store/src/captures.rs) | Generic capture controls use canonical hyphenated UUIDs and different recovery semantics. The scanner currently lists 32-hex entries as invalid. Do not broaden its ID validator, open those entries through `InspectCapture`, or route performance review through `RecoverCapture`. |
| [recording budget](../../crates/omniusong-recording/src/budget.rs), [bounded inspection](../../crates/omniusong-recording/src/inspection.rs), [performance semantic inspection](../../crates/omniusong-engine/src/performance_writer.rs) | Share one cumulative governor, full-original hashes, strict descriptor/schema/path/owner checks and six-stream checkpoint validation. Typed cancellation/limit failures return no partial usable review. |

The new module must not import filesystem/SQLite readers, `CaptureReader`, `SealedJournal`, recovery helpers or native engine control. It may consume the typed review DTO and types needed for display. All source access stays on the existing session worker. No dependencies, storage migration, service command or inspection-limit changes are required for this first slice.

## Allowed commands and local choices

Use an explicit app action family separate from `Action::Performance(Box<Command>)` and `Action::Recovery`. The recording-intent dispatch allowlist must stay closed to this new family.

| Visible action | Exact implementation route | Result |
| --- | --- | --- |
| **Scan saved recordings** | `Command::ScanPerformanceSources` | Replace the bounded directory snapshot; do not inspect any row. |
| **Refresh saved requests** | `Command::ListPerformanceIntents` | Replace the authenticated current receipt snapshot; do not review/open a recording card or perform a continuation. |
| Select a source row | UI state only | Select its exact stage ID and invalidate the displayed-review association; no storage command. |
| **Inspect selected recording** | `Command::InspectPerformanceSource { stage_id }` | Review only the chosen exact source. No caller-supplied path, checkpoint, limits or recovery operation. |
| **Cancel inspection** | `SessionService::cancel_source_inspection(exact_request_id)` | Request cooperative cancellation of that pending inspection; do not submit a second command or free worker credit. |
| Expand evidence / collapse review | UI state only | Change presentation. Preserve the pending request, drafts and source bytes. |

Do not add disabled placeholder buttons for recovery/adoption/attachment/export in this slice. Explain once: “This review checks the saved original. Recovery and adding it to a session are separate later steps.” Existing explicit recording-card attachment controls keep their own contract and are not activated by this panel.

There is **no automatic Scan or Inspect** on app construction, initial loading, panel expansion, source selection, request-list completion, engine retirement, reconnect or restart. No loop inspects all saved IDs. Existing `PerformanceUi::take_command` may still perform its metadata-only `ListPerformanceIntents`; the coordinator may pass its successfully correlated receipts to the new panel as a passive snapshot. This fan-out must not call `PerformanceUi::completed` for a list owned by the new panel, select an old card, or enqueue audio. Explicit Refresh saved requests remains available independently of whether the audio process was started.

## Proposed module interface and state

Create `crates/omniusong-app/src/performance_source_ui.rs`. The public-to-parent types and method signatures are frozen for this implementation; any change requires coordinator integration review without expanding the allowed operations:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
enum SourceRequest {
    Scan,
    RefreshRequests,
    Inspect { stage_id: String, view_epoch: u64 },
}
enum SourceAction {
    Submit(SourceRequest),
    CancelInspection(RequestId),
}
struct SourceAvailability {
    can_submit: bool,
    generation: Option<u64>,
    unavailable_reason: Option<String>,
}
// Public-to-parent methods. All perform bounded in-memory work only.
impl SourceRequest {
    fn command(&self) -> Command;
}
impl PerformanceSourceUi {
    fn new() -> Self;
    fn ui(&mut self, ui: &mut egui::Ui, availability: &SourceAvailability)
        -> Option<SourceAction>;
    fn submitted(&mut self, request: &SourceRequest, id: RequestId);
    fn submission_failed(&mut self, request: &SourceRequest, message: &str);
    fn completed(&mut self, request: &SourceRequest, id: RequestId,
        result: Result<&Reply, &ServiceError>);
    fn cancel_requested(&mut self, id: RequestId, accepted: bool);
    fn observe_requests(&mut self, generation: u64, receipts: &[PerformanceIntentReceipt]);
    fn generation_changed(&mut self, generation: u64);
    fn worker_lost(&mut self, generation: u64, error: &str);
    fn pending_inspection(&self) -> Option<RequestId>;
    fn select(&mut self, stage_id: &str) -> bool;
    fn inspection_request(&self) -> Option<SourceRequest>;
}
```

`completed` receives the exact original `SourceRequest`, `RequestId` and typed `Result<&Reply, &ServiceError>` after the app/service have checked the completion. Borrow during routing, then retain only the bounded values needed. Retain a bounded local error notice on consumed malformed/mismatched data rather than panicking; `completed` returns no continuation. `SourceRequest::command` is a closed three-variant conversion and cannot return a mutation. Main rechecks this allowlist at dispatch; a source-review action cannot carry an arbitrary `Command`.

State consists of one directory snapshot, one request snapshot (at most 32 receipts), selected exact stage ID, checked monotonic `view_epoch`, one pending request, one last successful review, and bounded local scan/request/inspection notices. Each snapshot carries its service generation and an observation serial; neither field is a source freshness proof. The pending state contains the exact request, actual accepted `RequestId`, and `cancel_requested` flag. No pending identity is fabricated before `try_submit` succeeds. The last review carries its requested stage, view epoch and generation. No status-history vector, per-frame review clone or queue of replacement inspections is permitted.

The view epoch changes on source selection, clearing/collapsing the detailed review, or connection invalidation. Counter exhaustion disables further inspection in that view with a local notice; never wrap and match an old reply. Retain the last successful DTO until an explicit newer successful review replaces it, but display it only under its exact associated source, with “Previous inspection” whenever the selection epoch or generation no longer matches. Selecting another source must not display the old source's duration or manifest under the new row.

Directory and request snapshots form a bounded union of at most **160 distinct rows**: up to 128 directory candidates and 32 request identities. Deduplicate by exact stage ID. A request-only row says “Saved request · source not inspected,” not “missing.” A scan can be truncated and the filesystem can change; absence from a scan never proves absence of the source. A known saved ID remains explicitly inspectable even when it was beyond the scan bound. Use no free-text pathname entry and no guessed ID conversion.

## Correlation, late replies and cancellation

1. The coordinator integrates `Action::PerformanceSource(SourceRequest)` into `submit`/`complete`. On accepted submission, store the app `Active` first and notify the panel with the actual service ID. The panel marks busy only for admitted work. A Busy/Full/Invalid rejection produces no phantom pending operation and does not erase the selected source or prior review. No automatic retry follows any result.
2. Service generation, sequence and command kind must all match. The service already rejects stale or wrong-kind completions without returning its pending credit; the app must not clear `Active` or panel pending state on that protocol error. The panel also matches its own pending ID and exact request, so direct/injected completion paths cannot bypass correlation.
3. For a successful Inspect reply, check review version 1, exact requested `stage_id`, `take_id == config.take_id == performance-<stage_id>`, supported profile, and bounded expected shape before presenting it. Paths and hashes are backend-authenticated evidence, not UI-created authority. Compare a saved request's path/clock/descriptor separately as reconciliation facts; a mismatch is a visible conflict, not a reason to replace the request. The UI performs no filesystem canonicalization, hashing or reconstruction of a manifest.
4. The user may change source selection while inspection runs. That changes only the view epoch. A matching old request completion still releases its real worker credit, but cannot replace the current source's detail view or move selection back. Show “Inspection finished for the previous selection” and retain its exact association; the current selection still needs its own explicit Inspect. A successful Scan preserves the selected ID even if not in its returned subset; it never automatically inspects or switches to another source.
5. Handle `CancelInspection(id)` before the normal busy/action gate, because the service is intentionally busy. Call only the existing cancellation method. A `true` return means “Cancellation requested · waiting for the inspection worker,” with Cancel disabled for that same request. It is not a completed cancellation. Retain both app and panel pending state; Scan, Inspect, Refresh saved requests and all other storage submission remain unavailable until the matching completion or actual worker disconnection.
6. A `false` cancellation return means the exact request is no longer cancellable; continue normal polling. Never cancel a different pending request, mutation, list or scan. A stale Cancel from the prior generation/sequence cannot affect new work. Repeated Cancel is idempotent local feedback, not repeated dispatch.
7. `InspectionCancelled` ends that inspection with “Inspection cancelled. Original unchanged.” Retain any older review as a previous observation. If the worker instead returns success after cancellation was requested, show “Inspection finished before cancellation took effect” and process the exact result under the same view checks. Do not relabel success as cancellation or infer rollback. Other errors retain their actual classification.
8. A payload identity mismatch after an otherwise valid completion ends the consumed request with a local protocol notice and no installed review. It cannot quarantine a healthy catalog or leave a phantom UI credit. Disconnection marks old observations as previous-connection evidence, clears only that generation's completed/lost pending state, and leaves retry explicit. Reconnect initializes the service only; it does not replay a scan, inspection or saved audio request.
9. Source reads must not replace or clear the app's retained mutation retry. Exclude this action family from the general `self.retry` assignment/clear paths in submit failure, completion, disconnect and Retry last action. Its only retry is a fresh explicit read of the same chosen source/list. It supplies no `Active.after` mutation continuation and installs no `DocumentView`.

Cancellation is cooperative. Existing VM/row/hash/decode checkpoints bound accounted work; operating-system calls can still take time. The UI has no seconds-to-completion promise, forced worker kill or second worker. Do not add a GUI timer that declares cancellation complete or releases the single service credit. Whole-app close follows existing responsive close/draft rules; dropping the service requests cancellation without joining. It does not promise the background read has already ended.

## Read-only restart comparison

Render two visibly separate sections: **Saved requests** and **Observed original**. Implement the comparison as a pure helper over one authenticated request receipt and one successful review, with structured reason codes for tests. It returns display facts only; it must have no “can attach,” “can seal,” “resume,” “recovered” or source-adoption authority flag.

| Evidence | Exact permitted display |
| --- | --- |
| Requests have not been loaded, failed to load, or belong to the previous connection | “Saved requests have not been checked for this connection.” Preserve the older snapshot with its label. Do not call the source unassigned. |
| A successful current request list has no matching Start ID | “No saved Start request was listed for this original.” No automatic adoption, target guess or request reconstruction. |
| A matching receipt exists | Show its revision, original target identity, Start request, requested output, saved GUI clock, optional Finish request, optional publication request and released flag. These describe durable requests, not current audio activity. |
| Source is unsealed (`Recording` or `Interrupted`, no manifest) | “Original retained · no sealed manifest observed.” `Recording` is the stored journal state, not proof that a worker is recording now. A saved Finish remains a request. |
| `Sealed` plus manifest hash and `manifest_interrupted == false` | “Existing normal manifest observed.” The bounded reader authenticated an existing manifest. Show checkpoint/Finish/fault facts separately; do not claim attachment eligibility, reliable Stop or complete human input. |
| `Sealed` plus manifest hash and `manifest_interrupted == true` | “Recovered manifest observed · final tail unknown.” This observes an existing result; the panel did not recover it. |
| Complete checkpoint returned | Show verified checkpoint audio frames/duration, group and callback bounds, and six-stream summary. Call it “Verified checkpoint,” not “Recovered recording” or “All events saved.” |
| No selected complete checkpoint | “No complete performance checkpoint was verified.” Keep byte count and errors visible. No playable range or fabricated zero-length success. |
| Stored Finish and observed `finish_cut` both present | Compare only the exact Finish ID and full scope. Show “Saved Finish request matches observed callback cut” or a conflict. The stored request contains no accepted-sequence fence; show observed fence/cut fields separately and never derive them from the request. |
| Stored Finish present, observed cut absent | “Finish request retained · callback cut not observed.” Absence can reflect the checked prefix/profile; it does not authorize a new normal seal. |
| Cut present, saved Finish absent | “Callback Finish observed · no saved Finish request listed.” Do not append one from the source. |
| Existing attachment request present | Show its immutable project/revision/operation/source as “Saved attachment request.” Equality with a source hash is not evidence of committed publication or current project placement. |
| Released receipt with attachment, from successful current `ListPerformanceIntents` | The current list implementation additionally verifies its exact historical publication. It may say “Historical attachment verified when requests were loaded,” followed by “Current session placement and owned-file availability were not checked here.” Unreleased attachment requests have no such proof in this API. |
| Released receipt without attachment | “Recording choice released.” It does not prove a file exists or that recording/Stop succeeded. |

Exact source/request comparison uses `start.directory_name()`, `start.take_id()`, full canonical journal path returned by the service, full `gui_clock`, and the saved descriptor SHA when present. Compare saved Finish ID/scope only with a returned cut. A present saved publication manifest hash may be compared with the observed manifest as another fact. Any unequal present fact is a conflict; keep both values and the old request unchanged. Missing optional evidence is “not observed,” not equality or conflict. The DTO has no actual selected-output-device identity to compare with `start.output_device`; retain that as requested-output metadata only.

**Current DTO limitation:** `PerformanceInspection.unknown_tail` is initialized to `true` and is never cleared by its shared recovery-oriented semantic scanner, including for an existing normal sealed source. Preserve that value and say “This checkpoint review keeps the final tail conservative; it does not prove every GUI event was saved.” Do not suppress the observed normal-manifest fact or rewrite the flag to false. The review exposes manifest hash/mode but omits the manifest body/index/stream totals used by `reviewed_performance_attachment`; this panel must not recreate that full admission from partial fields. A later dedicated, bounded read-only reconciliation service can add an authoritative relationship DTO after a separate contract review.

The review identity includes inspection usage counters. Treat `review_sha256` as the identity of this observation, not as a stable content-only equality key across implementations/re-inspections. The physical journal hash and original-row hash remain distinct. For this slice, show changed observations without declaring a new recovery/adoption decision.

## Presentation, caps and retained evidence

Add a **Saved performances** section in the existing right-side retained-source area, with the generic capture section still available. Keep the transport/instrument and Stop reachable. Use a compact list and one detail card; hashes and full paths belong in an expandable **Source evidence** section. While an inspection is pending, keep its progress label and Cancel control visible in the section header even if the detail is collapsed. Do not auto-expand a failure into a blocking modal. A source row's title may use its abbreviated ID with an accessible full ID; the source has no authoritative song title merely because a saved request targeted that song.

Primary detail order is: observed state, checkpoint duration/frames and sample rate/channels, known fault/unknown-tail warning, then saved-request comparison. Duration is an estimate derived only from the verified checkpoint's exact `audio_end` and nonzero declared sample rate; retain exact integer frames beside it. Neither file byte size nor an old GUI durable counter supplies a playable duration. Display all six stream names (`audio`, `gui-1`…`gui-4`, `control`) with their declared units; never turn event counts into audio frames or claim the retained raw records are editable notes. The review is metadata only and does not audition audio.

Freeze these existing backend capacities without increasing them:

| Resource | Bound |
| --- | ---: |
| Directory enumeration | 128 examined entries plus one unexamined truncation sentinel |
| Scan or review encoded reply | 64 KiB each |
| Saved requests | 32 current identities; authenticated history up to 16 revisions × 16 KiB per identity |
| Source file | 536,870,912 bytes (512 MiB) |
| Physical rows / cumulative row visits | 262,144 / 4,194,304 |
| Cumulative accounted work bytes | 1,073,741,824 (1 GiB) |
| Chunk visits / decoded record visits | 524,288 / 2,097,152 |
| Groups / SQLite VM steps | 65,536 / 32,000,000 |

Show `examined_entries`, `skipped_generic`, `skipped_unknown` and truncation explicitly. A truncated list says “Some sources were not listed”; no “all recordings found” claim. A limit failure says “This original exceeds the current inspection limit. Original retained.” Do not automatically increase limits, reopen unbounded, create a copy, truncate the original or substitute a partial report. A 30-minute 48 kHz stereo float32 recording has 691,200,000 raw PCM bytes before journal overhead, above the current file limit. This UI therefore does not qualify the complete 30-minute workflow.

Presentation is bounded independently of the DTO: virtualized row list; at most 16 generic problem rows and a visible remaining count; one performance problem and first fault; six stream summaries; four GUI source summaries; eight usage counters; checkpoint-member count rather than all member records. Keep full typed DTO/receipts within their service bounds. Prepare human-readable previews on snapshot receipt, not repeated full serialization in paint. Bound displayed error/source text to 512 Unicode scalars per summary and paths to their existing 4096-byte contract, with an omitted-text indication. Truncation affects display only; compare full values and preserve typed data. No raw PCM, decoded event arrays, raw JSON, unknown metadata or tail bytes are copied into UI state.

`SourceBusy`, `SourceUnavailable`, `InspectionLimit` and `InspectionCancelled` remain local to the affected source/read. Busy says “Original is in use. Stop/release its owner, then inspect again.” Unavailable says “Original could not be inspected,” with the bounded reason; do not parse text to infer deletion or corruption. The backend may report hot sidecars through SourceUnavailable. The UI must not offer cleanup/repair or delete `-journal`, `-wal` or `-shm`, and must not open SQLite to inspect them itself. A typed `StorageUnavailable` still invokes the existing catalog-retirement/reconnect path. Never downgrade it to a source-local warning; never upgrade a malformed source into catalog quarantine.

## Draft, navigation, audio and retry integration

Scan/Inspect/Refresh saved requests are non-project actions. They may run with unsaved metadata, note or clip drafts and retained musical choices, like the existing generic read-only review exemption. They neither invoke Save/Discard/Cancel nor mutate the active document, revision, selection or pending navigation. Apply that exemption in both `request` and `dispatch`, and in `retained_action_allowed`, so a direct dispatch cannot disagree with the visible button.

Keep the existing audio admission guard for starting a source read: active live/playback/capture/finalization or a retained recording card must be resolved through its own controls first. The panel may show previous observations while audio runs, but cannot bypass owner/quiescence handling. Read-only source review may coexist with generic recovery's retained attachment evidence; it must not clear or resolve that evidence. The sole service still serializes all admitted storage work.

While a read is pending, session navigation keeps its existing busy and draft rules. An attempted Open/Create/Import/Undo/Redo/Close cannot silently discard a draft. In particular, busy Close and OS close must still offer the appropriate Save/Discard/Cancel or existing explicit close choice. Typing a newer local field while a read is pending cannot be undone by its completion. Source review itself adds no permanent navigation guard after its request finishes. Closing the source detail is only a local view change, not the recording card's persistent “Keep recording locally” action.

Stop/Panic remain queue-independent engine controls in every busy/error/cancel state. Do not disable the transport through a whole-parent-panel `ui.disable()` driven by inspection credit, and do not cover it with a review modal. A selected source, saved Finish, normal-manifest observation or cancelled read cannot grant reliable Stop acknowledgement or open the note gate.

**Separate existing UX issue:** the coordinator observed saving/Stop wording remaining after a normal Seal. `PerformanceUi` renders `self.message` alongside newer `PerformanceStatus`, so stale explanatory copy can coexist with the sealed badge. Record and fix that later through a dedicated presentation precedence review. Do not change normal-seal authority, infer quiescence from a manifest, or couple this source-review slice to an unreviewed recording-state refactor.

## Ownership and implementation sequence

| Owner | Exclusive files / responsibility |
| --- | --- |
| Coordinator | Own this frozen contract, every `main.rs` change, module declarations, app action/routing/guards/cancel wiring, receipt fan-out, documentation integration, dependencies/migrations, Cargo/build/package/native work and final acceptance. Optional narrow audio/performance receipt plumbing also stays coordinator-owned. |
| UI implementation agent | New `performance_source_ui.rs` only: bounded state, rendering, pure comparison, declared methods and its own focused unit/UI tests. No `main.rs`, `audio_ui.rs`, `performance_ui.rs`, `recovery_ui.rs`, engine, session or store edits. |
| Independent tests/review agent | New `performance_source_integration_review.rs` only: `use super::*` integration tests against the coordinator's app hooks and injected service/engine peers. It must not edit `main.rs` to register itself; coordinator adds `#[cfg(test)] mod performance_source_integration_review;`. No native processes/catalog writes. |

First freeze `SourceRequest`/`SourceAction`, method signatures and the comparison result before parallel implementation. The coordinator can add the new module and action scaffold while the UI agent writes its file. The independent reviewer owns separate fixtures; UI test helpers, if needed, are bounded `cfg(test)` accessors supplied by the UI owner, not cross-file edits. Integrate and inspect the complete diff before running the package gate. No source readers, worker-budget changes or new service APIs are delegated by this packet.

## Required meaningful checks

The following are acceptance requirements, not completed test results. Tests must assert observed commands/state/evidence and negative side effects, not only mirror enums.

1. **No automatic source access:** construction, first frames, Initialize completion, metadata-only request list, panel expansion, source selection, reconnect and restart emit no Scan/Inspect/mutation/engine command. Clicking Scan emits one `ScanPerformanceSources`; a reply does not trigger Inspect. Refresh emits only List. Button repetition while pending emits no replacement request.
2. **Namespace and caps:** mixed UUID/32-hex/unknown directory fixtures preserve each scanner's contract; performance rows never enter generic Inspect/Recover. Truncated scan plus a known saved request outside the subset remains explicitly inspectable by exact ID. Duplicate IDs merge only display rows. Malformed IDs/path-like input cannot become a command. Test 128+32 union, bounded details, omitted-problem counts and exact frame integers above floating-point-safe values.
3. **Correlation and credit:** injected worker tests cover wrong generation, sequence, kind, reply variant and wrong stage/take; selection A→B→A still changes the epoch and cannot accept A's old display association. A stale service completion preserves pending credit; a consumed but malformed source payload ends only its own panel pending state. No result changes project/draft/selection or a saved mutation retry.
4. **Cancellation:** hold an inspection with a bounded injected backend, click Cancel, verify exact token targeting and Busy until its completion, then verify credit becomes available. Cancel cannot target Scan/List/Edit/Attach. An old Cancel cannot reach the next request. Test success winning the cancel race, typed cancellation, failure winning the race, disconnect during cancel and nonjoining close. No sleeps as the only ordering proof; use explicit test gates/channels.
5. **Request/source facts:** matrix for no list, failed/stale list, no matching request, Start-only, Finish-only observation mismatch, matching full Finish scope, descriptor/clock/path/take conflicts, v1 without v2 Finish, unsealed source, observed normal/recovered manifest, no checkpoint, fault/pending cause and later corrupt/uncheckpointed tail. A normal-manifest DTO with `unknown_tail=true` must display both facts without “all events saved,” “attached,” “recoverable now” or seal authority. Unreleased publication is only a request; released+publication uses only the actual authenticated-list historical guarantee and never current-placement wording.
6. **Source-local failure versus catalog failure:** real existing service fixtures prove malformed/busy/sidecar/limit/cancel cases leave the catalog usable and all source bytes/sidecars unchanged. UI integration injects each typed failure and verifies no reconnect/quarantine or discarded project retry. `StorageUnavailable` still retires the generation and preserves drafts/review as prior evidence. Do not duplicate low-level hashing tests when the existing service tests already cover unchanged behavior.
7. **Draft/close/Stop regression:** dirty metadata/note/clip drafts survive each read, late reply, error, cancellation and selection change exactly. Read-only actions do not invoke a save dialog; subsequent navigation/Close still does. Retained musical and generic attachment retry identities remain unchanged. Inject active/busy engine states to show Stop remains available and reaches the urgent port while storage credit is held, and that source read admission does not bypass the audio guard.
8. **Presentation:** real egui/accessibility assertions exercise the named buttons, enabled reasons, busy/cancel labels, normal/recovered/unknown distinctions, stable row IDs and bounded evidence in both light/dark themes. Verify keyboard/text interaction does not emit live notes. Review narrow-window layout and scaling natively rather than accepting only headless labels.

The coordinator then qualifies the exact final optimized/default-feature Mac bundle. Use existing retained sources: inspect a completed normal recording; inspect the no-sidecar failed stage; demonstrate explicit refusal of a preserved sidecar-bearing stage without cleanup; show known-request-only and truncated/no-match wording as available. Capture the actual source selection, checkpoint/manifest/request facts, cancellation or honest completion-before-cancel race, source-local failure, return to the same session with drafts intact, and restart with no automatic Scan/Inspect. Exercise existing output/Stop while the source panel is visible; no new native input stream or fresh recording is required by this packet.

Retain bundle/source hashes, screenshots/accessibility evidence, exact request identities where instrumented, and independently compare original DB/sidecar bytes and intent rows before/after the read-only sequence. A native cancellation that loses to a fast completed read is not evidence of cooperative cancellation; deterministic headless cancellation and native responsiveness are separate checks. No recovered manifest, attachment or archive made by another workflow counts as this slice performing recovery. Mark unavailable native display/input/listening/platform checks separately.

## API limits and later work

There is no blocking backend gap for the narrowly labelled read-only panel. The coordinator must supply app routing and the small in-memory integration surface above. Existing source replies currently discarded in `main` must be consumed; cancellation needs its separate busy-safe route; global mutation-retry isolation needs explicit handling.

Full authoritative restart reconciliation is **not** already a session command. A future bounded `ReconcilePerformanceIntent` may expose normal-manifest/complete-prefix relationships, historical commitment for unreleased publication, owned-original availability and current arrangement facts separately. That contract must not replay Start/Finish or use an inspection budget/stored Finish as a native seal permit. Do not call mutation `AttachPerformance` merely to learn whether an old operation committed.

The durable source-action ledger, exact full-original evidence copy, explicit checked recovery of the reviewed checkpoint on a working copy, recover/attach conflict handling and retry-stable accepting choices, unassigned-source complete export, and editable-note projection remain required later packets. Source inspection/cancellation never deletes or mutates PCM, GUI/control originals, bad/unselected records, unknown metadata or tails, and it never cleans up a hot sidecar. Neither this contract nor its eventual read-only acceptance closes those later goal requirements.

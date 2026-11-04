# Native studio-evidence controls

October 22, 2026. The source gate and installed walkthrough have separate evidence. This contract extends the [durable jobs](NATIVE-EVIDENCE-JOBS.md) through the existing native Rust UI and sole session service. Song backups, full studio evidence and operator rollback remain separate scopes.

## Explicit actions

The Studio evidence window offers full-root Export, archive Inspect, reviewed separate Restore and retained Job history. Presentation performs no filesystem, archive, catalog or engine access. Construction, painting, editing fields, selection and connection changes send no artifact operation. Each worker request requires its exact explicitly offered UI action, current serial/epoch/generation and no continuation. Raw generic routes refuse.

Review export creates one immutable job/export identity. Review restore requires a current successful inspection and binds the exact input path, whole-container SHA, destination, inspection serial, UI epoch and worker generation. Confirmation is a separate click; accepted execution consumes it. Field changes, project/track/draft/context changes, terminal generation loss and reconnect invalidate it. A→B→A does not revive a review. Dispatch rechecks the complete context even before the next paint.

A new job requires initialized live storage with its canonical origin reported by the worker, no dirty draft or retained edit/retry/navigation, released audio/capture/recovery ownership and no close guard. Reads retain musical drafts and exact failed choices. List/Read can run without a ready catalog and while audio is active. Inspect/Reconcile additionally require released audio ownership. Every command shares capacity-one credit.

## Current proof and retained history

An accepted Inspect retires the prior inspection/restore confirmation, even for the same path; a failed new read leaves the old proof historical. Accepted Read retires its previous selected view. Accepted Reconcile retires the selected history because an observation may have been appended; another reconciliation requires explicit Read. A compact result cannot manufacture a full current view.

Correct late results after a changed context become historical cards. Wrong request/generation/kind/payload identity cannot consume another active request. Historical receipts explicitly imply neither new execution nor a current artifact check. The primary outcome, any separate governor stop, metadata-persistence failure and current catalog retirement remain independently visible. Archived errors do not retire healthy session storage.

Cancellation targets the exact pending Run/Inspect/Reconcile and retains both credits until correlation or disconnect. A completed operation can win late cancellation. Cancellation is not rollback; partial or complete output may remain. EOF keeps the exact job/request as uncertain, without generic retry, destination changes, cleanup or musical continuation. Journal reconciliation remains an explicit existing-artifact operation.

## Metadata-only recovery

A successful Library reply reports the actual canonical NativeStore root; the app retains these exact bytes for later requests and reconnect. A resolved Initialize-only retry clears after successful explicit initialization; retained musical mutations do not clear.

Connect history replaces only a disconnected service with the same unopened existing-only worker and submits no Initialize, read, execution or reconciliation. History is accessible while `ready=false`. Open studio catalog is a separate explicit action.

For a cold launch with a missing/renamed source, use `--evidence-history --data-dir <original-exact-canonical-path>`. This launch mode opens the history window and creates an unopened metadata connection without opening or recreating the catalog. Normal startup retains its earlier creation policy and is not qualified as missing-root recovery. Relative/raw alias configuration refuses with an actionable exact-origin message; canonical parent/header checks remain in the worker. The origin is never inferred from a new location.

## Bounds and verification

Reuse the compiled request/result/journal caps. Presentation holds at most 128 summaries within 64 KiB, one selected view within 256 KiB and eight result cards. Paths are exact control-free UTF-8, at most 4096 bytes; explanatory messages are at most 2048 bytes with visible shortening. Outcomes are bounded to 32 KiB. Inspection presentation separately checks the 2-MiB manifest and a derived manifest-plus-16-KiB DTO envelope before retaining only its compact summary. Identity fields are never silently shortened. Invalid/oversized replies remain local errors.

Scope and pending cancellation stay above the scrolling details. Light/dark and narrow headless checks establish bounded layout behavior, not actual display scaling or native accessibility acceptance. The 512-MiB file and 1-GiB work governor are unchanged; a large studio may refuse within those limits.

The [source gate](../verification/rust-native-evidence-ui-tests.json) must record exact inputs, tests and preservation. Actual package acceptance separately exercises explicit Export→Inspect→confirmed separate Restore, changed-input refusal, local malformed-archive failure, cancellation/uncertain restart, source/archive-absent history/reconciliation and unchanged original catalog/raw trees. Use an isolated synthetic studio and no native input, including loopback. Source tests and injected errors are not installed UI, physical power-loss, full-studio scale or Windows qualification. General catalog enrollment/recovery, adoption, scalable recording and the full workstation goal remain required.

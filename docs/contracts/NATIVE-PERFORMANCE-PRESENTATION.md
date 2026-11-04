# Native recorded-output presentation

October 22, 2026. Frozen display-only contract. This extends [recording controls](NATIVE-PERFORMANCE-UI.md); it grants no engine, storage, recovery, Stop or normal-seal authority. The [7473 source-review package](../verification/rust-native-performance-source-review-tests.json) remains a separate compiled/native boundary while its drive-open walkthrough waits. Do not replace that package for this source-only change before its native review can finish.

## Problem and allowed change

`PerformanceUi::ui` currently prints the last `self.message` before newer accepted worker status. `begin` leaves Saving text during Recording. `AudioUi::performance_status` requests owner release after Sealed; the existing `cancel` writes Stop-request text, which remains after `stopped`. A reopened saved request can also inherit action prose from a previous card.

Derive presentation at render time from the existing exact card/status/task facts. Do not change `observe`, `begin`, `finish`, `cancel`, `stopped`, retirement, task submission/completion, retry, operation identities, guards, input gates or any source. Keep the automatic post-Sealed Stop path unchanged. No new mutable phase cache, event, command, timer, state migration, dependency, engine or filesystem call is allowed.

## Pure interface

Add a borrowed `PerformancePresentation<'a>` with `heading: &'static str`, `body: &'a str`, `pending_notice: Option<&'static str>`, `storage_notice: Option<&'static str>`, `history_notice: Option<&'static str>`, `first_audio_error: Option<&'a str>` and `error: Option<&'a str>`. `PerformanceUi::presentation(&self) -> PerformancePresentation<'_>` is pure and can be private to the module. Every card branch uses static copy; the no-card body may borrow the existing initial/last-completed `self.message`.

Presentation creates no eligibility/authority flag. Repeated calls and no-event paint preserve the exact card, receipts, status revisions, task/engine queues, error history, guards and button eligibility. Existing errors retain their actual contents; first audio failure and current storage/transport error remain separate.

## Main precedence

Apply this order to a current card. Accepted status means the card's existing `observe` checks already accepted it; the display does not independently admit a status.

| Priority / existing facts | Exact heading | Body meaning |
| --- | --- | --- |
| `status.sealed.is_some()` and `card.attached` | Attachment operation confirmed | The exact attachment completed, possibly by replay; current song placement may since have changed. Do not imply current clip placement. |
| `status.sealed.is_some()` | Recording saved locally | A normal manifest was confirmed. This alone proves neither current owner release nor completeness of every GUI event. |
| Accepted Failed | Recording failed | Worker failure; inspect any retained stage before use. No complete final tail is promised. |
| Accepted Interrupted | Recording interrupted | Keep staged evidence; review the checkpoint/tail separately. |
| Accepted SealUncertain | Seal outcome uncertain | Preserve the exact source and choices. Do not infer success, rollback or restart. |
| `started_here && stopped` | Recording inactive | Active recording controls are closed. The old worker phase is only the last observation; a stopped card can represent either exact urgent acknowledgement or typed retirement. |
| `cancelled && started_here && !stopped` | Stop requested · confirmation pending | Input is closed; actual owner result remains pending. No complete tail or successful Stop is inferred. |
| `cancelled && !started_here` with a receipt | Saved recording choice | Request history only; source not inspected, and this window did not start/resume recording. |
| `cancelled && !started_here` without a receipt | Unstarted recording choice | Local/request outcome is retained. Do not claim a saved choice, no file, or rollback. |
| Accepted Finishing | Finishing recording | Worker Finish is in progress; full finalization is not confirmed. |
| Accepted Stopping | Stopping recording | Worker reported stopping; this is not the urgent acknowledgement. |
| Accepted Sealing | Saving recorded original | Worker finalization is in progress; no confirmed saved original yet. |
| Local `intent.finish` exists, older Starting/Recording or no worker status | Finish requested | Local input is closed; distinguish a reported cut from a still-unobserved callback Finish. Never claim every GUI event saved. |
| Accepted Recording | Recording keys and pads | Actual worker recording observation supersedes Start-save prose. |
| Accepted Starting or `started_here` awaiting status | Preparing recording output | Start requested; readiness/human input not yet confirmed. |
| No status, Start queued/active, no uncertain prior/retry/rejection | Preparing recording choice | Local Start intent is waiting/saving; output readiness unconfirmed. |
| No status, receipt exists | Saved recording choice | Stored request, not verified source or active audio. |
| Other unstarted card | Unstarted recording choice | Local exact choice is retained, without claiming persistence or source existence. |
| No card | Recorded output | Existing initial/last-completed local message; no inferred active recording. |

When a sealed original exists and `started_here && !stopped`, show **Output release is still pending. The saved original alone does not confirm Stop.** separately. After `stopped`, remove that pending notice without claiming an acknowledgement kind from that bit. First/captured errors remain visible even with a confirmed seal.

Render worker phase only as **Last worker status: <phase>**, never as an unqualified duplicate primary state. Preserve exact existing device, sample format, rendered/durable frame and callback counts, path/hash evidence and the unknown parent-loss disclosure. Do not turn stored Finish/attachment choices into source facts. When `card.attached` is shown in the original detail, use **Attachment operation confirmed**, not a claim of current placement. Otherwise the sealed-original badge is **Original saved locally · attachment not confirmed here**. An unacknowledged publication may already have committed; `!attached` cannot prove non-attachment.

`prior_unknown` is sticky history. If shown, label it **An earlier recording storage outcome was uncertain.** It must not falsely label a later exact successful observation as a fresh uncertainty. Do not erase the history to fix wording.

## Storage operations stay separate

Derive the operation from BOTH `Task.effect` and its exact command variant:

| Effect / command | Operation name |
| --- | --- |
| List / ListPerformanceIntents | Saved-choice listing |
| Start / SavePerformanceIntent | Start choice |
| Finish / SavePerformanceIntent | Finish choice |
| PrepareRelease / SavePerformanceIntent | Retained recording choice |
| Attach / SavePerformanceIntent | Attachment choice |
| Attach / AttachPerformance | Stored attachment |
| Release / SavePerformanceIntent | Return-to-session choice |

Queued is **<operation> queued for storage.** Active is **Saving <operation>.**, except listing **Loading saved recording choices.** and AttachPerformance **Applying stored attachment.** Retry is **<operation> retained for explicit retry.** A retry is not currently saving. Unknown effect/command pairs say the recording storage choice has an inconsistent shape, not that an operation is executing successfully. Valid state has at most one storage task across these lifecycle stages; if malformed combinations coexist, show current active/queued work before its retained retry without mutating any of them.

New status cannot hide a pending attachment/release storage operation or its error. Sealed source and failed publication are independent facts. No UI message issues a continuation, changes CAS, consumes retry, supersedes an uncertain choice or reopens input.

## Verification and delivery

The coordinator owns this contract, `performance_ui.rs`, test-module registration, Cargo, docs and integration. The independent test agent owns only new `performance_presentation_review.rs`, registered as a child module of `performance_ui`. Architecture reviewer is read-only. No concurrent Cargo/native/device work.

Use real egui/AccessKit text with complete before/after snapshots of existing card, task, engine and error state. Cover queued/active Start versus awaiting Ready; accepted Recording; queued/active/retried Finish versus actual Finishing/Stopping/Sealing; cancellation before Start and after recording; normal Sealed before/after automatic release Stop; typed retirement after old Recording; errors, saved attachment versus publication, release retry; reopened saved/released/Finish requests; old/invalid statuses; and no-event repeated paint. Verify body and error wrapping at narrow widths in both themes. Static AccessKit Label text uses `value` in pinned egui 0.37; interactive names use `label`.

Run meaningful app/workspace tests, formatting, warnings-denied Clippy and the default build. Source/software acceptance does not qualify the installed 7473 app or this new text natively. Qualify a later integrated optimized package only after the existing source-review walkthrough can finish, keeping both packages' source/signature and original-preservation boundaries explicit. No native input, recording, paid calls, uploads or Fuego edits are authorized for this display packet. Full milestone 2, capacity/parent-loss work, source-action/copy recovery/export, editing, MIDI 2/MPE, native inference/plugins, installers and Windows remain required.

# Next slice: normal recording commitment and current view

This is an implementation plan from the independent architecture investigation during the projection-consumer work. It describes work that is **not implemented or qualified by that gate**. The full Rust macOS/Windows workstation goal, MIDI 2/MPE and every remaining acceptance requirement retain their scope.

## Concrete problem

Normal `Command::AttachCapture` currently calls storage and then constructs the generic document/library reply. A later current-load, playback-preparation or library failure can obscure a successful durable attachment. Its historical receipt also contains the old result document, which must never replace a newer arrangement on replay. Dedicated recovered attachment and archive outcomes already distinguish commitment from current usability; normal attachment needs an equally explicit boundary.

The selected solution is a compact commitment reply followed, when still appropriate, by a separate ordinary current-document read. No full document, library, document encoding/hash or filesystem work follows successful storage return inside the attachment command.

## Shared session contract

Retain `CommandKind::AttachCapture`. Give the command a boxed versioned request, with these bounded fields:

| Field | Meaning |
| --- | --- |
| `version` | Initially 1; reject unknown versions/fields. |
| `origin_root` | Exact canonical root of the actual open store. It is runtime correlation, not portable project metadata or music authority. |
| `journal_path` | Lexical absolute UTF-8 locator; do not canonicalize or reopen a missing old stage before historical receipt lookup. |
| `publication` | Exact existing `TakePublication`, including stable operation identity and expected revision. |

Use a 64 KiB canonical encoded-request bound. Fingerprint and preserve the entire exact request: version, origin, lexical journal locator and publication. The persisted store receipt attests only the existing publication request; it does not attest the locator or supply a newly invented filesystem proof. A retry must retain the whole request, not reconstruct its origin or path from current UI state.

The boxed reply has version, request fingerprint, `store_invoked`, an outcome and independently validated catalog-retirement information. Keep it within 16 KiB, with existing 96-byte identity bounds and diagnostics bounded to 1,024 bytes. Do not add a result-document hash that would require another full postcommit serialization.

| Outcome | Required facts |
| --- | --- |
| `Committed` | Exact operation/project IDs, previous and checked adjacent committed revision, and `FreshCommit` or `RecordedReplay`. |
| `Rejected` | Typed bounded problem for a proven pre-invocation rejection or an existing failure contract proving no project commitment. |
| `Uncertain` | Typed bounded problem when invoked storage cannot prove the final project disposition. |

Keep typed cancellation/limit details distinct from actual live-catalog retirement. “Not committed” never means ingestion retained no original. Source-independent historical commitment never means a current arrangement, playable source or authenticated derivation.

`Committed` requires `store_invoked = true`. `store_invoked = false` permits only a proven pre-invocation `Rejected`; an invoked `Rejected` requires a typed existing guarantee of no project commit, and an ordinary invoked error is `Uncertain`. These facts describe this attempt: a pre-invocation refusal on retry cannot erase a preceding uncertain commitment or prove that the operation never committed. EOF is client uncertainty and must not invent whether the store was invoked.

## Backend execution

Validate and fingerprint before invocation; require exact origin against the actual open store. Reserve the bounded response and conversion capacity before invoking storage. Call the existing storage attachment method once. On success, construct only the compact commitment from already-returned validated fields. Do not call `load`, `list`, `DocumentView::prepare`, perform filesystem work, hash/encode a document or check a governor/cancellation afterward.

An exact historical retry keeps its operation ID, publication bytes and expected revision. It does not acquire source authority, reinstall an old document, rebase onto a newer arrangement or generate a replacement operation ID. Retain the existing normal-ingestion export authorization and its stated legacy limits. Do not claim that ingestion becomes cumulatively governed merely because the following project mutation is governed.

Exclude the typed route from a generic mutation mapper that automatically makes every failure uncertain. Keep conservative uncertainty for invoked failures unless the existing storage contract proves a more precise result. Validate fatal retirement independently of the musical outcome.

## Service and native correlation

Use the already established archive envelope rules: exact transport ID, worker generation, kind, origin and request fingerprint. A foreign/mismatched reference retains the active service credit. A matching reference with an invalid body consumes an explicit protocol/uncertain result, installs nothing and still handles independently valid retirement. Validate IDs, version and adjacent revisions before accepting a commitment.

The native app records the compact commitment and disables attachment retry **before** considering a view update. Preserve a distinct view state: not requested, loading, ready, failed or context changed. A current-view failure changes only that state and cannot erase the saved fact.

Only if worker generation, origin, selected project and metadata/note/clip draft snapshots still match may the coordinator use the sole service credit to submit an ordinary `Open`. That reads the current document; it must not install the historical receipt snapshot, including after Undo or branch changes. A changed local context suppresses automatic replacement and preserves every draft. A later fatal current-read failure may retire its worker while retaining the prior commitment fact.

On EOF/lost acknowledgement, retain the exact request and uncertain state. Explicit retry uses a new transport ID with unchanged musical publication identity. Do not delete stages, replace retained originals or silently reattach after uncertainty. This slice does not add durable native projection/archive admission or interactive cancellation.

## Bounded work ownership

The coordinator owns shared DTO decisions, command validation, backend routing, worker protocol, integration and all execution. A session implementation agent may own a dedicated `capture_attachments.rs` module after those interfaces are agreed. An app implementation agent may own dedicated audio/recovery commitment/view state helpers; shared `main.rs` integration remains coordinated to avoid overlapping edits. An independent architecture reviewer checks persistence/disposition, correlation and draft preservation. Use device-free synthetic fixtures; no microphone access, personal uploads, paid calls, new weights or installed-app replacement follows from this plan.

## Acceptance matrix

| Case | Evidence required |
| --- | --- |
| Fresh ordinary and genuine projection-project attachment | Real storage commitment transported compactly; current document read separately. |
| Current-load or library failure after commitment | Saved fact remains confirmed; editor/drafts remain intact. |
| Replay with original absent | Exact recorded commitment without source opening. |
| Replay after Undo/later edits | Current state read without reinstalling the older placement. |
| Operation reuse/stale revision | Precise refusal; no rebasing or new operation identity. |
| Ingestion succeeds, publication refuses | Retained owned original; no claim of zero filesystem effects. |
| Lost acknowledgement after actual commit | Uncertain first; exact retry yields one recorded commitment. |
| Foreign generation/origin/fingerprint | No installation; original active credit retained. |
| Matching reference, invalid receipt body | Protocol uncertainty; independently valid fatal retirement retained. |
| Draft/selection/context change during execution | Commitment retained; no automatic replacement. |
| Later fatal current-read failure | Correct worker retired; earlier commitment remains visible. |

Combine genuine store/session execution, deterministic fault controls and headless coordinator/draft checks. Bind a stable source/build/test packet and protected-store preservation. Qualify the resulting workflow in a named native binary using separate synthetic data before claiming native acceptance. Actual hardware, listening, long recording, Windows and the remaining full-goal requirements retain their independent gates.

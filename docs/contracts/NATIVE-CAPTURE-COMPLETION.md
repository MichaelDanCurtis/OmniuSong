# Normal recording completion and current editor view

This contract implements the [normal recording completion plan](../reviews/RUST-CAPTURE-COMPLETION-PLAN.md). It governs normal `AttachCapture`, including the older retained-stage attachment route. It does not change durable performance-intent or reviewed performance-source attachment protocols. Implementation and execution evidence must be recorded separately; this document alone proves neither.

## Exact request and compact result

Keep `CommandKind::AttachCapture`, with `Command::AttachCapture { request: Box<CaptureAttachmentRequest> }` and `Reply::CaptureAttachment(Box<CaptureAttachmentReply>)`. The request is version 1, exact canonical studio origin, exact lexical absolute UTF-8 journal locator, and the complete existing `TakePublication`. Validate bounded normal paths without filesystem access; do not canonicalize or open the journal before historical lookup. Canonical encoding of the entire request is capped at 64 KiB and fingerprinted. IDs are at most 96 bytes, paths 4,096 bytes. Unknown fields or versions refuse. Freeze the complete request before its first submission in both native routes; retry keeps all its bytes and musical identities.

The existing store receipt attests publication, not the session locator/origin. The session fingerprint binds those runtime correlation fields without turning them into portable musical authority. No old document is installed from a replay receipt.

The versioned reply is capped at 16 KiB. It contains the exact request fingerprint, `store_invoked`, one outcome and independent catalog retirement. Diagnostics are bounded at 1,024 bytes. A committed outcome carries only exact operation/project IDs, previous and adjacent committed revision, and `FreshCommit` or `RecordedReplay`; it carries no document or document hash.

| Outcome | Invocation and disposition |
| --- | --- |
| Committed | Storage was invoked and returned its authenticated commitment. |
| Rejected before invocation | This attempt did not invoke storage; it cannot establish the disposition of an earlier uncertain attempt. |
| Rejected after invocation | Only an exact existing typed `StoreError::Conflict` or `EditRejected` no-project-commit guarantee. Retained ingestion originals may still exist. |
| Uncertain | An invoked failure lacks a proven project disposition. Wrapped catalog/rollback failures stay uncertain, including a nested conflict. |

Keep typed cancellation/limit, SQL origin and catalog retirement distinct. Compute fatal/unusable retirement from the complete store error, including cleanup. Source/private SQL faults do not retire a different catalog connection. Preserve the actual diagnostic domain; do not infer fatality from prose. EOF is client uncertainty, with no invented `store_invoked` value.

## Backend and sole service

Validate/fingerprint and reserve the compact result box, owned bounded identities and diagnostic/conversion buffers before invoking storage. Check the exact actual open store origin. Call existing `attach_capture` once. On success inspect only the returned compact identity/revision/replay fields and construct the reserved response. Do not load/list, prepare a document, touch a source/filesystem, encode/hash a document or check a governor after storage success. Normal ingestion keeps its existing authorization and legacy limits; this session boundary does not govern the whole ingestion operation.

Bypass the generic mutation-error mapper. Independently validate exact transport ID, worker generation, kind and whole-request fingerprint. A foreign reference retains the sole pending credit. A matching fingerprint with an invalid body, including an unsupported reply version, consumes credit as protocol uncertainty, preserving independently valid retirement. Version is body validation, not reference matching. Retain the exact retry for typed rejected/uncertain replies and transport loss; a valid committed result clears attachment retry. Queue completion before retiring a fatal worker.

## Native commitment and view

Retain a separate latest attachment record containing the exact request, confirmed commitment or uncertainty and editor-view state: `NotRequested`, `Loading`, `Ready`, `Failed`, `ContextChanged`. Confirming a commitment disables attachment retry and releases its audio/recovery guard **before** considering any editor read. Later view failure, EOF or retirement cannot erase the saved fact or re-enable attachment retry. An unconfirmed attempt cannot be silently replaced by a different request; a pre-invocation rejection on exact retry cannot erase prior uncertainty.

After commitment, submit a separate ordinary `Open` only if worker generation, exact native origin, selected project/revision and metadata/note/clip draft snapshots still match. It carries no mutation continuation. Validate those conditions again on completion, plus returned project ID and library origin, **before** installing either library or document. A changed context preserves all drafts and the commitment. A view error updates only view/retirement state; its generic Open retry must never become an attachment retry. Replaying after Undo/later edits reads the current saved project; it never reinstalls the historical result document.

## Required evidence and limits

Use device-free temporary fixtures for genuine ordinary/project-projection publication and real session-worker tests. Exercise absent-journal/original replay, later edits/Undo, postcommit library/current-read failure, wrong origin, operation reuse/conflict, retained ingestion after refusal, lost acknowledgement after an actual commit, foreign references, matching malformed bodies with valid retirement, and draft/selection changes through both stages. Pure/constructed controls must remain distinct from actual storage/worker behavior and physical fault injection.

Preserve protected catalog/original/binary records. Bind the accepted source, build, test results and independent review. A source gate does not qualify an installed app window. Native qualification must use a named binary and separate synthetic studio before any new native guarantee. Durable projection/archive admission, interactive cancellation, sustained recording, actual input/listening/latency, MIDI 2/MPE stages, plugins, inference/choir, installers and Windows retain their full goal requirements.

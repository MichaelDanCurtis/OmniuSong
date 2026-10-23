# Governed project archive session integration: independent source review

The reviewed implementation has bounded source acceptance with no unresolved source-authority blocker. It connects the three existing governed store routes to the sole session worker, preserves compact completion facts and separately carries failure phase, original problem, governor stop and catalog retirement. Final source-stable test/preservation evidence remains a separate gate. This review does not qualify durable native archive admission, active projection consumers or a new application package.

I reviewed repository source only and wrote the assigned design/review documents. I did not run Cargo, tests, formatters, SQL, applications, devices or audio; I did not open or hash the default catalog, protected objects or installed binary. The [session design](RUST-PROJECT-ARCHIVE-SESSION-DESIGN.md) and [underlying governed archive review](RUST-GOVERNED-PROJECTION-ARCHIVE-REVIEW.md) define the broader boundaries.

## Execution and commitment

`crates/omniusong-session/src/project_archives/runner.rs` admits a strict versioned request, constructs one governor and prepays bounded correlation and response conversion before invoking storage. It requires an already opened store and exact canonical origin, with no lazy open/create. Export additionally checks an ordinary nonsymlink canonical parent. The three branches call the existing bounded export, identity restore and copy methods with the same budget.

Malformed input and an initial governor/reservation stop return pre-effect outer errors. They do not hash or construct an unpaid fingerprinted result after the stop. Once that work is reserved, missing-store, origin, parent and later preflight failures are compact typed outcomes with `invoked=false`. `invoked=true` means the store method was called, including exact recorded replay; it is not a claim of fresh execution or commitment.

The success converters in `project_archives.rs` extract only existing receipt fields and project identity/revision. They do not load a project, scan a library, reopen/hash an archive, serialize a full returned document or check cancellation after store success. Export/identity variants encode durable/committed completion directly; copy retains its explicit committed-versus-recorded completion. Cleanup failures remain bounded warnings on success. A later user-requested load or list result cannot change that already returned compact fact.

The real list behavior remains explicit: `NativeStore::list` returns a guarded `ProjectSummary.problem` for a nonempty projection registry, rather than granting a healthy source-aware summary. It does not by itself fail Initialize. The worker requires an actual present store, not a healthy project summary. The design was corrected to match that implementation; this review claims neither an executed postcommit library error nor a failed-Initialize fixture.

## Exact replies, errors and reusable generation safety

The finite request fingerprint includes version, canonical origin text, operation and every exact path/project/copy identity/pin, with a fixed domain separator. `lib.rs::try_poll` checks generation/sequence, command kind and request reference before consuming credit. A foreign reference retains the exact pending owner. For a matching reference it extracts independent retirement, validates the body, then consumes credit. An invalid body becomes Protocol with outcome unknown and installs no proof/document; valid retirement remains effective. The pending operation cannot enter generic retry because the new command is explicitly non-retryable.

`ProjectArchiveReply::retirement_error` retains a valid explicit retirement core even if another field or its message is malformed. It also conservatively retires for an explicit live-catalog primary IOERR/CORRUPT/NOTADB diagnostic when a malformed matching body omitted its separate retirement DTO. Input, private-output and owned-source SQL codes cannot trigger that fallback. Normal conversion computes retirement independently from the original recursive Store failure before selecting/truncating its primary problem. A source failure plus actual live cleanup failure therefore has separate transport facts; ordinary private cleanup warnings never become catalog cleanup failures.

The worker queues its one terminal completion and then retires that backend. The client records the corresponding terminal state. On EOF, it now caches the failure rather than recomputing uncertainty from an empty retry slot; repeated polls cannot turn an accepted non-retryable mutation into an apparent known rollback. This is in-memory uncertainty retention, not durable restart history.

The distinct project-archive token is correlated to the exact pending RequestId, shares the one worker credit and is reset only while credit is free before the next submission. Cancel does not release the owner or promise rollback. Drop preserves the accepted mutation token; it keeps the established read-only inspection cancellation behavior. Older command families still use their prior cancellation chain and handlers.

## Bounded context and serialization

Requests are strict, bounded absolute UTF-8 lexical paths with existing ID/hash validation. Their encoded cap is 65,536 bytes; replies have an explicit separate 262,144-byte cap. Message formatting writes into a bounded UTF-8 sink without first materializing a complete error string. Encoding counts through a bounded writer before allocation. Cleanup has four retained issues plus an explicit omitted count, preserving exact paths rather than truncating them into different identities.

Reply decoding checks the raw cap and strict syntax first, then returns the parsed body for ordered reference/retirement/semantic handling. `from_json` alone is not acceptance. String enums use explicit string decoding, avoiding map-shaped unit variants. Direct derived serde use is not the advertised bounded decode boundary, and this slice still uses in-process Rust channels rather than installing a new serialized worker transport.

Context validation matches the actual store's incremental facts. Early invoked failures may contain only canonical root, without unearned source/operation/hash fields; initial export can still have root as destination before resolving its output. Normal export source is the root's `studio.sqlite3`, not the project or output pathname. These initial partials are limited to NotStarted with no staging or placements. Later phase/action/path/hash constraints reject incompatible bodies, and private staging association requires the exact bounded namespace and canonical UUID spelling. Association is diagnostic correlation, not permission to open/delete the path.

One consequential correction during review concerned RecordedReplay. The first replay check precedes acquisition; the second runs inside the immediate transaction after private acquisition. A concurrent exact copy admitted between those checks can therefore yield a recorded refusal with an associated stage and observed hash. Validation now permits either both acquired facts or neither, always with zero confirmed placements. The new representation test is explicitly constructor-only; it does not claim execution of that concurrent race.

## Native preservation and review corrections

`crates/omniusong-app/src/main.rs::submit` rejects this command family before assigning a native action or submitting worker credit. Existing backup controls continue to use legacy commands, and the new reply arm cannot install an editor document or run a continuation. The headless coordinator test covers all three raw routes, an injected continuation, unchanged document/name draft, no queued work and a subsequent paint. It is not actual-window/device acceptance or proof of every musical draft combination; the unconditional early guard is the source basis for preserving those other fields.

Review findings addressed in the final source include:

* ordinary-parent preflight and runner visibility, while preserving the initial response-reserve outer-error exception;
* actual early store context fields instead of fabricated requested fields;
* impossible placement counts and later phases lacking their observed stage/hash;
* raw-bounded reply decoding that preserves the independent retirement core;
* explicit live-fatal fallback when a matching malformed body omits retirement metadata;
* the acquired second-check RecordedReplay context, without weakening exact copy identity;
* accurate guarded-summary wording and persistent EOF uncertainty.

None of these changes removes an archive/source guard or adds a new source-authority token.

## Evidence and remaining limits

The coordinator reports successful compilation and three public SessionService integration cases using genuine device-free performance-writer output, recovered originals and actual authenticated projection publication. I inspected that fixture source: it covers both performance record profiles, all three compact archive operations, exact publication remapping, preserved originals/opaque data, copy replay after input removal without reinstalling the old document, and wrong-origin refusal followed by an exact good request. These are software-rendered recovered fixtures, not normal native seals or hardware capture. Later store loads/catalog comparisons in those tests are independent assertions, not operations in the backend completion path.

DTO tests and channel/token tests distinguish constructor/code-matrix controls from physical failures. They cover strict variants/limits, source-domain diagnostics, cancellation/late completion, foreign versus malformed correlated replies, Drop, initial refusal, recorded refusal and sticky EOF. Mixed source/main failure transport and the raced acquired-replay representation remain labeled constructed controls. The coordinator separately reports four schema-3 OSRE cases; that separate test work is not independently audited or promoted into native OSRE acceptance by this review. No complete gate total or protected-store preservation claim is made here.

Identity restore still has no expected input SHA parameter, export still has no expected saved revision parameter, and no bounded source-aware Inspect command was added. No document/consumer authority follows from this compact receipt. Legacy guarded archive routes and source-aware reader/list/attachment qualification remain separate. Durable exact-request admission, unresolved restart/reconciliation, native review/confirmation/warning handling and the exact-binary editing/history/archive/audition workflow remain required. Lower recording error-finalizer limitations, fixed-control/I/O responsiveness limits and existing archive budgets are unchanged. This slice qualifies no installation, native window, device input/output, listening, hard power loss, long-capacity, Windows, AI inference or MIDI 2/MPE transport, and does not complete the full product goal.

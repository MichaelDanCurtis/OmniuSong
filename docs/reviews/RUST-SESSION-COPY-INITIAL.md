# Independent background session and restore-copy review

October 20, 2026. This review covers the portable background session service, its native UI integration, atomic session edits and typed restore-as-copy lineage. It is a bounded M2 persistence/UI review, not acceptance of audio, MIDI, models, plugins or Windows. This lane edits only independent tests and this review; the coordinator runs Cargo and owns the integrated gate.

## Starting evidence

The prior `native-takes-workspace-tests.log` reports 152 passed, zero failed/ignored. The previous rebuilt Mac inspector receipt, `docs/verification/rust-native-recording-ui.json`, records schema-2 synthetic recording inspection, tempo save/undo/redo, restart and source/owned-object/opaque-field retention. Its binary SHA is `c69854edac39c1b8bf75fae667c095619f9c331c687a55e234027346ffdb4be4`. That earlier binary does not qualify this background-worker build. Its four generated frames are not microphone/controller capture or playback. The final evidence below must come from this slice's fresh gate.

## Adopted boundaries and dispositions

The implementation uses main-store schema 4 with four explicit lineage tables: immutable proof JSON, per-proof typed operation mappings, per-project proof ownership, and live copy admissions. OSAR exports version 2 and retains a strict version-1 reader. Introducing new tables under the old strict archive version, or commandeering an opaque project metadata field, would have changed existing interpretation; both were avoided.

Copy restoration rewrites only root `ProjectDocument.id` in current/history/operation-result documents and the project/operation identities of explicitly supported typed requests. Ordinary edit requests are identified by their required operation identity before a strict take-publication wrapper is considered. Unknown request extensions, UUID-looking vendor strings, note/clip/stream/take identities, clocks, `nativeImport.sourceId`, arbitrary-precision numbers and exact original import bytes retain their previous meaning. The independent test compares every retained request/result/history snapshot against this precise rewrite, rather than searching for strings that resemble IDs.

Every new copy has a fresh project and fresh global operation IDs. A canonical proof binds source archive SHA/length, source/target project identities, the exact initial copied result, a sorted mapping digest and explicit sorted ancestor proof hashes. Older proof bytes and mappings are inherited unchanged as historical namespace. Only the new live admission reserves a global copy-operation identity. An ancestor's historical copy ID may therefore equal an unrelated local edit ID; the independent fixture explicitly exercises that intended separation. Live copy/edit/publication IDs reject cross-kind collisions.

Lineage validation checks bounded canonical proof receipts, mapping counts/digests, live operation hashes, complete transitive ancestry, cycles, a unique immediate source-project edge and mapping continuity across successive copies. The active proof owns all other retained proofs. Version-2 archives reject unrelated lineage rows. Limits include 256 proofs, 100,000 mappings per proof, 1,000,000 total lineage mappings and the shared 256 MiB archive metadata budget. These are corruption/consistency checks; hashes do not authenticate a malicious author who can reconstruct an entire archive.

The complete closure includes current state, both sides of retained history, every operation result, exact original import references and every immutable proof result. Objects are durably installed before the sole project/history/operation/lineage publication transaction. A final SQL failure rolls back those visible rows; retained staging files or immutable orphan objects remain recoverable. An exact retry first validates the retained receipt, mappings, full reference closure and owned originals, then returns its original copied result without consulting a missing external path or overwriting later edits. The session backend separately loads the current document before returning a replay to the UI.

The session transport uses capacity-one synchronous request and response channels plus one outstanding admission credit held until a valid completion is consumed. Busy/invalid submissions retain the command without advancing the admitted sequence. Completion generation, request sequence, command kind and reply variant are correlated; rejection does not clear the genuine pending request. This is an in-process trusted backend boundary, not authentication of an untrusted process.

Known operation failure is delivered as a completion. Disconnection is separate and retains only commands with safe resolution semantics; an edit/copy retry retains its durable operation ID while receiving a new transport request ID. Create/import/export have no blanket retry. `UpdateSession` saves name and tempo in one history entry and receipt, including undo/redo and exact retries. A replay refreshes actual current state rather than installing its older operation-result snapshot.

The UI creator performs no store work. Storage, scanning, archive work and document previews run on the session worker; repaint/poll uses nonblocking calls. Editing/navigation controls are disabled while busy, close remains available, and document/draft context guards reject stale installation. Dirty navigation retains its requested export/inspect/restore action through Save/Discard/Cancel; a saved continuation keeps the same copy intent. Dropping the service does not join a held worker. This proves responsiveness while the process remains alive, not durable queue completion after process exit; closing never proves that an accepted action rolled back.

## Independent regressions and gate status

`crates/omniusong-store/tests/archive_copy_review.rs` contains ten isolated regressions:

- Atomic name/tempo validation, original-result replay, durable undo/redo and opaque metadata.
- Typed-only remapping across all current/history/request/result rows, independently recomputed mapping/proof digests, exact imports, assets and source bytes.
- Distinct copy intents, disjoint fresh operation maps, reopen, later edits and source-free exact retry.
- Copy-of-copy ancestor bytes, historical/live namespace separation and identity restore of the complete lineage.
- Cross-kind global ID conflicts and changed archive fingerprint rejection.
- Damaged mapping, canonical proof and retained live-operation receipt rejection on source-free retry.
- Missing ancestor association and final SQL publication rollback across all ten metadata tables.
- Concurrent identical admissions through independent store connections producing one project/map/receipt.
- A sealed take retained only by a publication receipt after redo truncation, including typed wrapped-request remapping and byte-identical object reuse.
- A sealed take retained only by an immutable ancestor proof result after direct-save/undo/branch truncation. Export, identity restore and descendant copy must retain it; source-free replay and export reject when its owned original is removed.

The first proof-only fixture used a snake-case SQL string probe against camel-case native JSON. Its failure was a test assertion error, not missing retention. It was replaced with typed document/reference extraction in both proof-only and operation-only fixtures before requalification.

`crates/omniusong-session/tests/service_review.rs` contains six independent regressions for capacity-one admission/responses, invalid and stale correlation, known failure versus disconnect/retry, held-worker nonjoining drop, actual storage-backed atomic save/undo/redo with current-view replay, and recovery from a nonfatal backend operation error. Injection factories require `test-hooks`; production defaults do not enable that feature. Root-owned UI tests separately exercise paint/close, dirty actions and stale drafts. Only two explicitly authorized prior schema-version assertions were updated from 3 to 4.

All fixtures are synthetic UUID directories under ignored `.runtime`. The final accepted `.runtime/rust-migration/session-copy-tests.log` and `session-copy-workspace-tests-accepted.log` report 176 passed, zero failed/ignored, including all ten copy tests and six independent service tests. The initial app gate failed two accessibility action lookups during the modal sizing frame; its corrected helper and ten retained/new UI tests pass in the accepted run. The reviewer read the actual accepted log and summed its 23 result groups. The coordinator confirmed completed exit 0 for `cargo fmt --all -- --check`, `cargo test --workspace --all-features --locked`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, and the fresh Mac build. Logs are `session-copy-format.log`, `session-copy-tests.log`, `session-copy-clippy-accepted.log` and `session-copy-build-accepted.log` under `.runtime/rust-migration`. No independent Cargo was run in this lane.

## Remaining limits

Strict take verification and archive acquisition/validation are linear filesystem/database work on the storage worker. There is no verified cache or realtime performance claim. Bounded queues do not make an action cancellable or durably queued; process death is resolved by SQLite atomicity and stable-id retry where implemented. Staging/orphan recovery is retained on disk but lacks a complete recovery UI and retention policy.

The library response is capped for display, but this slice does not establish a large-library latency/memory benchmark. Source acquisition permits ordinary external aliases and fingerprints actual acquired bytes; it is not a filesystem sandbox against hostile same-user parent replacement. The dated `docs/verification/rust-native-archive-copy.json` now records authentic OSAR-v1 input SHA `34107a52140dd4ba5d8416b6f6089f87981d3fa99a51dfce5d112cfe73cc0408`, OSAR-v2 output of 37,220,352 bytes with 36,846,248 payload bytes, copy-of-copy, unchanged ancestor proof bytes and source-free retry after later edits. This is a synthetic archive-storage result; it establishes no capture or audible result. Actual rebuilt UI use requires its separate observed-binary receipt. Real capture/listening, device clocks, playback, plugin/model execution, Windows filesystems/UI and new OS permissions remain unqualified.

No unresolved production correctness finding was identified in the reviewed code paths as of this static pass. The reviewed persistence/session slice is accepted within the tested boundaries above. This is not full-goal acceptance. The boxed `SubmitError` adjustment and final Clippy simplifications are included in the accepted source/gate: error fields and ownership of the rejected command remain unchanged. As of final review, the coordinator observed a responsive loading window while its storage worker was still inside SQLite open; that does not establish the actual new archive UI flow, and no TCC/permission cause is inferred. Actual rebuilt-window archive use remains pending its separate receipt.


## Reviewed source snapshot

SHA-256 snapshot at 2026-10-20 19:13:06 UTC. This includes the boxed submission-error API. Evidence hashes identify reviewed bytes; gate disposition must be matched to these sources. No documentation self-hash is used.

| Path | SHA-256 |
| --- | --- |
| `Cargo.toml` | `89839f215791612645a0127ffc2fc59da753afde312f249d6b74c9e6d3452dbd` |
| `crates/omniusong-core/src/contracts.rs` | `51033d4144237ded4ceecb5b00ca88205ea467d12bbc209c19e1877f7e14baa3` |
| `crates/omniusong-core/src/native.rs` | `e5c65a632a7e9520ade21c8daf3b8ae786769e83af543a24a15db83b7a53d549` |
| `crates/omniusong-store/src/lib.rs` | `eff9588a53a8965027f5787f87cf347e041addc4f78b1ab58576617d88ff0042` |
| `crates/omniusong-store/src/operations.rs` | `ad8be8959e6ac62a1dd6346459dfb18bedfcf477da73c96c6e553daf3ad36146` |
| `crates/omniusong-store/src/takes.rs` | `999183c1b01c940264b231b9b61fce36b76554db94c6c032ea24d25033c2c77a` |
| `crates/omniusong-store/src/archive.rs` | `90fa06d44acdb55f717d8ac29317bcf187c4cc3f1467438682845e17c8b18aee` |
| `crates/omniusong-store/src/archive_copy.rs` | `ee948957a4d6bceccfd8a15d7e6ac9f253612754668e6e031b80c3378d559c1d` |
| `crates/omniusong-session/Cargo.toml` | `3abddf01fa39822066f252066e1e322e8df7d2a5961b2a0f573ea4cf73def090` |
| `crates/omniusong-session/src/lib.rs` | `2c8342d1fc72fadc3219aab030b266bf64f58f4d5d3a3c566d4b815095bc0bf4` |
| `crates/omniusong-app/src/main.rs` | `abb6276760686275fdee61d7a3866d12450b4f2a87803b3f2a9e660cef9c027c` |
| `crates/omniusong-store/tests/archive_copy_review.rs` | `863f7e27efdcd313615026c1cab5057aec4d67e46b2a0354eea4e4e627fdd9a2` |
| `crates/omniusong-session/tests/service_review.rs` | `accecf6b0c309f62751b84592b5930eff26a58ae47aa2f65e93fbcebb72fa4e4` |
| `crates/omniusong-store/tests/operation_review.rs` | `1a2ca68cec01c8e464bca9c78f9b6fe0b4cdcd8fb5cd96eac3f5561abde6287d` |
| `crates/omniusong-store/tests/session_edit.rs` | `1c1208315c0982bad6b9a2068fbe8736d924be34884a74e965c16e914c34aad1` |
| `crates/omniusong-store/examples/qualify_archive_copy.rs` | `7a4740dc21117a687926b6e12b3de9b3c642d5325e90367f760bec5fa767ebcb` |


## Reconnect incident and terminal-generation follow-up

A later external-drive disappearance/reconnection exposed a malformed development catalog: its header declared 45 pages but only 41 remained. This does not retroactively change the earlier 176-test gate. Independent raw-page/backup auditing is recorded in `.runtime/rust-migration/drive-reconnect-93681b4b-65d8-40d2-b017-98aa53b2b633/independent-audit.md`. All four current documents survived, but one history row and three old edit receipts did not. No complete-ledger or zero-data-loss recovery is claimed, and no physical/TCC/application cause is inferred.

The coordinator adopted the conservative policy: three complete projects restored with exact retained closure, plus a fresh-ID revision-0 snapshot of the fourth project's current musical/opaque data, with explicit incomplete-history evidence and the exact original backup embedded as bounded hex. Independent byte/row comparisons verified the candidate, exported archives, original take, source JSON, import BLOB, old proof evidence and identity-restored roundtrip. No lost request was synthesized or admitted. The original was retained unchanged and promotion used a copied verified candidate; the separate recovery and promotion receipts identify those artifacts. The forensic audit contains their exact hashes.

The service now queues a matching known `StorageUnavailable` completion before retiring its worker. Correlation is checked before the client marks the generation disconnected; the known cause remains available after producer EOF. Safe retry intents and the UI draft survive. Reconnect/initialization is explicit and separate from manually retrying the same durable operation ID. An independent committed-edit/terminal-acknowledgement fixture verifies that retry has no duplicate effect/history. Missing/malformed external inputs and conflicting operation content remain ordinary nonterminal errors.

Review identified and closed a check/open race: reconnect's first metadata precheck still called CREATE-capable `NativeStore::open`. The new `open_existing` excludes CREATE, makes no missing directory, and rejects uninitialized version 0 before and under the migration lock. The service reconnect path calls it. Owner tests cover missing directory/database, uninitialized files and populated-history reopening; independent tests also check missing/empty directories and Unix symlink preservation. The case-specific recovered-snapshot banner exposes the history limitation without interpreting diagnostic content as source-reference authority.

The returned-store-error classifier deliberately recognizes only typed SQLite IOERR/CORRUPT/NOTADB primary codes. Reconnect path preconditions are a separate terminal guard. Wrapped late archive errors and errors already reduced to summary text are not code-classified; ordinary filesystem errors/other SQLite primary codes remain outside this rule. A stalled OS/SQLite call has no new deadline, although paint/close does not join it. This is bounded generation retirement, not automatic repair or comprehensive physical-failure detection.

The coordinator confirmed completed exit 0 for `cargo fmt --all -- --check`, `cargo test --workspace --all-features --locked` (**191 passed, zero failed/ignored**) and `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`. Logs are `.runtime/rust-migration/disconnect-{format,workspace-tests,clippy}.log`. This reviewer read the test log directly (24 completed groups), including all eleven independent service tests. No Cargo ran in this lane. Actual new-bundle UI use, native build completion, capture/audio/inference/Windows and full-goal acceptance remain separate from this bounded gate.

Final frozen-source snapshot at 2026-10-20 19:52:25 UTC. These hashes supersede earlier identities only for the follow-up code; the historical 176-test snapshot above remains dated evidence. The executed case-recovery operator source/binary were separately preserved because later input-read hardening has a different source identity.

| Current source | SHA-256 |
| --- | --- |
| `crates/omniusong-store/src/lib.rs` | `7bff7d65b5f0ed3f14ee4c428799381a68baea922c13d087c366992691b45b06` |
| `crates/omniusong-store/tests/existing_open.rs` | `7817a27a70e43c61073458f75e1460a6a393f5b3b2cffa7281f9ac2a85e6d773` |
| `crates/omniusong-session/src/lib.rs` | `d4f9d138b966fe55b9d6a5bca10b049508ee862ab4eabacc0116a2f081ee114d` |
| `crates/omniusong-session/tests/service_review.rs` | `ec73b47d0b9d0494bbcb33d2a234e9f7f4b01c4e9c61b18081a3e3f556d1bef6` |
| `crates/omniusong-app/src/main.rs` | `213ddde8581f327a0ea0673cc7fef8bf53b6eb8ac55a19bc1afa9af0ab92aa52` |
| `crates/omniusong-store/examples/recover_disconnected_preview.rs` | `b26f8c8479df6168ffc88f58682034aa0b241b1e4d22fcb008051b7ae9101c00` |
| `crates/omniusong-session/Cargo.toml` | `3abddf01fa39822066f252066e1e322e8df7d2a5961b2a0f573ea4cf73def090` |
| `Cargo.toml` | `89839f215791612645a0127ffc2fc59da753afde312f249d6b74c9e6d3452dbd` |

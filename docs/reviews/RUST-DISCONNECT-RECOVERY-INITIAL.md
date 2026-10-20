# Main External disconnect: read-only recovery review

October 20, 2026. This investigation examined the coordinator's preserved raw copy, the completed UI archive and the preserved immutable take. It did not open the original damaged store, repair any database, change production source, run Cargo, change mounts or permissions, or execute recovered SQL. The only written artifact in this lane is this review.

## Disposition

The native store is physically shorter than its own valid SQLite header and references four absent table pages. This is **data-page loss**, not merely a damaged secondary index. All four current project documents survive, including the latest copied session at revision 8, but one history row and three operation receipts do not survive in the preserved file. A complete restoration of that latest session's original historical bytes is therefore not established.

The completed version 2 UI archive is a viable baseline for a **new** recovery destination. It passes the independent checks below and contains the original synthetic recording session at revision 5. The revision 8 current document is a separate salvage candidate whose content can be preserved without claiming its missing history survived. Reconstruction of the observed late edit/undo/redo sequence is possible as a labeled derived candidate, subject to the provenance and validation conditions below.

## Preserved evidence

The coordinator's receipt is `.runtime/rust-migration/drive-reconnect-93681b4b-65d8-40d2-b017-98aa53b2b633/preservation.json`. All damaged-store inspection used its `original-native-store/` copy. Before and after hashes matched.

| Artifact | Bytes | SHA-256 | Observed status |
|---|---:|---|---|
| Preserved `studio.sqlite3` | 167,936 | `3cec762a9455ed3b53525529e0ddf8e8d631e4a92931f4c445bb9cdc00b234a4` | Standard readonly schema and integrity queries fail `database disk image is malformed` |
| Completed `.runtime/rust-migration/native-ui-backup-v2.omniusong-native` | 163,840 | `00c6ab268772bddd03c198694799c02d8754c4909762f79c41c31b51903e627e` | SQLite integrity succeeds; archive snapshot/index/payload checks succeed |
| Preserved take `takes/679914498bf02ced1570f8b58ba441eb1f958e8dec54d2931a4eab9f3657ff6b.sqlite3` | 40,960 | `679914498bf02ced1570f8b58ba441eb1f958e8dec54d2931a4eab9f3657ff6b` | SQLite integrity succeeds; byte-identical to archived take |

The raw copy contains these two store files and no captured rollback journal/WAL. That absence is an observation about preserved evidence; it does not establish when a journal was deleted or whether an unobserved journal existed at the time of disconnection. SQLite journals can be essential recovery state. [SQLite file-format documentation](https://www.sqlite.org/fileformat2.html#hot_journals)

The coordinator separately reports all 30 original source files and 43 source snapshots unchanged. This lane did not repeat reads of those original inputs.

## Page-level findings

The damaged file has 4,096-byte pages, rollback-format read/write version 1, native store user version 4, and 41 physical pages. Its header advertises 45 pages. The change counter and version-valid-for field both equal 21, so the header's page count meets SQLite's validity rule. Four advertised pages—42 through 45, totaling 16,384 bytes—are absent. This is a direct byte-level observation; the hardware/filesystem cause and timing remain undetermined. [SQLite header-size rules](https://www.sqlite.org/fileformat2.html#in_header_database_size)

A bounded in-memory parser walked only existing B-tree and overflow pages, with cycle and bounds checks. It decoded complete records without filling missing pages or inventing payloads:

| Structure | Root/pages | Surviving evidence |
|---|---|---|
| Schema | Root 1, leaves 35–36 | All 37 schema rows: tables, indexes and triggers |
| Projects | Root 2, leaves 12, 13, 20 | Four complete documents |
| Edit history | Root 4 | Seven complete rows; right child 42 absent |
| Operations | Root 16 | Two complete rows on 40–41; children 43–45 absent; operation index has five entries |
| Original import | Root 8, existing overflow pages | One exact 11,529-byte source JSON BLOB; hash matches recorded source digest |
| Native take registry | Root 18 | One complete manifest/physical-object registration |
| Copy proofs/maps/ownership/admission | Roots 24, 26, 29, 31 | One complete row in each table |
| Legacy assets | Root 6 | Empty |

The four current project rows are:

| Project | Revision / cursor | Name and tempo |
|---|---|---|
| `2df3b9be-9c77-4560-9c45-7be5385d503d` | 5 / 1 | Native UI verified song, 117.5 |
| `aa3c40ed-277a-4b37-8bf0-101842d3de28` | 0 / 0 | Aria piano · local model preview, 104 |
| `eb9b8562-4e5d-47c6-adae-e9194fcab668` | 5 / 3 | Synthetic native recording inspection, 106.25 |
| `670fcc05-94ad-47ea-bb67-41d98bf3e44d` | 8 / 4 | Native archive restored copy, 119.25 |

The copied project's sequences 1–3 survive. Its fourth history row is on absent page 42. Its original mapped publication operation survives; the three later operations do not. The two surviving operation rows are the original publication and its copied mapping. Copy proof `fe1556be4bb05355bf7c90571b00fa62b0db4391a1e7c679e2ab0cd675876896` and live copy admission `3ba30a34-9ea5-491d-ad50-009e1e4dace6` survive. These facts do not verify the missing later request/result bytes.

The system SQLite CLI independently read the immutable readonly copy with `.recover --ignore-freelist`. It exited 0, emitted no stderr and produced 75,153 bytes of SQL, SHA-256 `79f4b5a61f23bf0983424cdc66096bf567f9a00f5dffffa2e4164d08218cfb98`. Its INSERT counts agree with the raw traversal. This lane captured that output in memory only and did not execute it. A successful `.recover` extraction is salvage, not semantic recovery: recovered output can omit or alter records and can violate constraints. [SQLite recovery limitations](https://www.sqlite.org/recovery.html)

## Completed backup checks

The completed archive advertises and contains 40 physical pages, OSAR application ID `0x4f534152`, archive version 2 and one project. It contains three history rows, one operation, no import/legacy-asset rows, one take and one take chunk; lineage tables are empty. The project is `eb9b8562-4e5d-47c6-adae-e9194fcab668`, revision 5, cursor 3, tempo 106.25.

Independent checks established:

- `PRAGMA integrity_check` returns `ok`; `foreign_key_check` returns no rows.
- Current payload, all before/after snapshots, and operation request/result SHA-256 values match.
- The counted metadata is exactly 13,877 bytes. Recomputing the documented archive index yields `2dfc1fcc925f0cbce23cbecabb5efa03e181657d7efca793ecf54e6865248964`, matching the archive receipt.
- The take chunk's sequence, length and digest match. The combined 40,960-byte physical file matches its registered object digest and the preserved immutable take byte-for-byte.
- Archive and preserved-store hashes remain unchanged after inspection.

A future restore must still run the native archive/manifest/core validators against a new staging destination. These read-only checks did not execute a native restoration or claim full musical interpretation validation.

## Recovery choice and provenance policy

Preserve three distinct artifacts: untouched raw evidence, a salvage candidate containing only extracted surviving rows, and any derived reconstruction candidate. Do not modify the preserved copy, replace the original store, pad absent pages, lower a cursor silently, or discard unmatched evidence. A new candidate may rebuild indexes from surviving rows, but a clean SQLite integrity result alone will not repair its missing cursor/history/receipt invariants.

The coordinator has observed the late action sequence: `UpdateSession { name: "Native archive restored copy", tempo: 119.25 }` from revision 5 to 6, Undo from 6 to 7, and Redo from 7 to 8. The reviewer found their operation IDs in the surviving index. That supports **derived reconstruction**, not proof of the original missing serialized request/result hashes.

For a derived candidate, require:

1. Validate and retain the exact surviving copy-proof result at revision 5, the latest revision 8 JSON, index evidence and observed action sequence separately. Pin the application/core versions used to reconstruct.
2. Generate the fourth history row and three typed requests/results through the pinned Rust model and canonical serializer. Correlate each surviving operation ID to its original rowid/action using independent evidence. If that mapping is ambiguous, allocate new recovery action IDs instead of guessing old admissions.
3. Check `before5 -> update6 -> undo7 -> redo8`, cursor behavior, expected/committed revisions and canonical checksums. The generated final document must equal the surviving revision 8 document as a complete value, including opaque metadata, not just name/tempo. Compare all independently surviving canonical bytes/hashes where available.
4. Attach a durable typed recovery receipt binding source file hash, missing pages, extraction hash, candidate content hashes, pinned code, evidence references and every generated row's origin. Use `reconstructed` for generated history/receipts and distinguish surviving-but-unverified rows from independently hash-verified rows. Newly computed hashes are never labeled original recovered hashes.
5. Require native document, request, history, lineage, reference, full take-file and archive round-trip validation. Undo/redo, replay and restart must be tested against the new candidate. Missing historical asset/operation roots remain an explicit recovery limitation unless recovered from separate evidence.

Current schema 4 operation receipts and OSAR v2 exports do not carry per-row reconstruction origin. Ordinary replay would report a reconstructed row as an ordinary committed receipt, and adding an unrecognized SQL audit table would not make it survive export. Therefore the reconstructed candidate should remain **quarantined with its bound audit receipt** until an explicit recovery provenance contract is implemented across store, replay, archive/copy and UI. Do not seize or overwrite an inherited opaque JSON key to conceal this distinction.

For immediate ordinary use without that new contract, prefer a validated backup restored into a new store and a separately labeled fresh-project recovery fork of the surviving latest snapshot, with explicit loss-of-history provenance retained with that fork. Preserve exact original snapshot bytes as evidence. This gives the user their latest musical content without manufacturing historical admission guarantees. Both options preserve the original evidence; the derived candidate additionally offers a path to restoring the known edit sequence after provenance support is in place.

## Accepted case-specific fallback plan

After the triage above, the coordinator proposed a bounded manual incident tool. This lane accepts the following design; execution and resulting candidate qualification are separate coordinator work:

- Import the reviewed `.recover` SQL only into a new quarantine directory and copy the exact verified immutable take there. Never replace or mutate original evidence.
- Export the three complete projects through the existing native archive validator, then restore them into a separate new store. Require the damaged copied project's ordinary export to fail because cursor 4 exceeds its three surviving history rows; do not adjust that row to make the gate pass.
- Create a fresh revision-0 project from the exact surviving revision-8 JSON, with a visible recovered name and a collision-safe explicit manual recovery notice. `ProjectDocument::imported_copy` adds a new `nativeImport` receipt and preserves a prior `nativeImport` value under `previousImport`; this is an additional intentional provenance change and must be listed in the preservation comparison.
- Embed the exact source JSON string/hash, missing history sequence and three operation IDs, surviving copy proof/map/admission evidence, and exact completed OSAR bytes in a reversible encoding with SHA/length in that notice (the implemented candidate uses hexadecimal). The archive is 163,840 bytes, so this specific bounded receipt fits the native document ceiling. Preserve any preexisting foreign notice value rather than overwrite it.
- Do not insert inherited live copy admissions or synthesized old operations into the new project's ledger. The preserved old proof/admission data is explicitly historical evidence inside the notice.
- Verify every non-whitelisted document field unchanged. The allowed differences are new root ID, revision 0, recovered name, the declared `nativeImport` provenance wrapper and the manual recovery notice. In particular, track/note/clip/expression/source identities and values must remain unchanged.
- Round-trip the new project through native export, restore and copy; verify the exact embedded source JSON and decoded archive bytes/digests survive, alongside the separately referenced immutable take. The encoded bytes remain literal evidence, never an inferred asset path or executable instruction.

Because the evidence bytes live inside the preserved document metadata, this manual fork does not require new dependency inference or silent OSAR schema changes. Its receipt must say that musical content was preserved with the listed provenance/identity changes and that prior history/three operation receipts were lost. This is not a general production recovery format, an automatic repair feature, or a claim to have recovered the missing original ledger bytes.

## Independent candidate review and reversible promotion

The coordinator implemented the manual operator example `crates/omniusong-store/examples/recover_disconnected_preview.rs` and generated `docs/verification/rust-native-disconnect-recovery.json`. This lane read the source, receipt and actual candidate `.runtime/rust-migration/recovered-native-preview-20261020/recovered-store` independently. The coordinator is changing its input helper to enforce the 2 MiB limit during the read, rather than relying on metadata and an after-read check; that hardening does not change the already inspected bounded input bytes.

Observed candidate database SHA-256 is `d7ac97ef63f9f4a85678289f698d9009b83613db1cbc0fb669166b60861d20ae`. Integrity is `ok` and the foreign-key check is empty. New project `b50ee3d5-ca2d-4d5f-b5af-c9a6ab9955ae` has revision 0, cursor 0 and name `Recovered · Native archive restored copy`. Its payload is 347,378 bytes, SHA-256 `02a2ffe5bcc541ab5b87e7e10838edafd24c367e79b62949f890983d8bbe9fda`.

Independent comparison found exactly the five allowed changed root keys: `id`, `revision`, `name`, `nativeImport` and `omniusongOperatorRecovery`; no other root value differed. Existing prior provenance/notice values are preserved. The notice says `historicalLedgerComplete=false`, `reconstructedOperationCount=0`, missing sequence 4 and the three missing IDs. It retains three surviving history rows, one operation and one each of the proof/map/admission rows as evidence. The new fork has zero edit rows, operation rows, import rows, live copy admissions and lineage associations.

The notice's exact source JSON hashes to `04aceba28b170eda514355b77e92d12a9cf19072e6a90e00377af50a4717fcc3`. Its hexadecimal archive decodes to exactly 163,840 bytes, identical to the completed backup and matching `00c6ab...627e`. The owned take remains exactly 40,960 bytes with hash `679914...ff6b`, a regular non-symlink file with one link. The example successfully rejects ordinary export of the incomplete source copy, validates the completed backup, exports/restores the three healthy projects, and exports/restores/reopens the fresh snapshot. The other independent reviewer owns the complete row-by-row healthy-project comparison.

No candidate fidelity blocker was found in this review. A reversible promotion fits the authorized recovery objective and does not require another confirmation merely because the existing failed default store must move. Preconditions: the other row-level validation gate passes, and the old app/process is confirmed closed. Copy the validated candidate into a fresh unique sibling staging directory; verify its complete regular-file tree hashes and database integrity before moving the default store. Rename the original default directory to a unique disconnected-evidence name without overwriting any path, then rename the verified staging directory into the default location. Retain original, forensic copy, validated candidate, independent archives and a promotion receipt naming their paths/digests. If either rename fails, keep the app closed and leave every path diagnosable; never erase evidence to force completion. Start a fresh app process, then verify the recovered name/tempo, healthy projects and reopen behavior.

This approval is for reversible recovery of this specific development incident. It does not qualify automatic repair, removable-drive durability, or reuse of a connection that experienced the disconnect.

## Original storage-resilience gap assessment

These were the source findings before the follow-up hardening reviewed below, not an attribution of this corruption to SQLite or to the interrupted export. The later audit explicitly narrows which gaps are now addressed:

- `NativeStore::open` creates the directory/database as needed and sets `foreign_keys=ON` and `synchronous=FULL`. It does not distinguish an expected existing store from a newly initialized store after a removable path disappears, and it does not verify an enrolled volume/file identity. A remount pathname alone is not an identity guarantee.
- `NativeBackend` retains its `Some(NativeStore)` after backend errors. Initialize reuses that handle; Refresh also reuses it. There is no explicit storage-unavailable/quarantined state, connection-generation replacement, or volume/database identity revalidation before resumed writes. A returned backend error must retire the unsafe storage generation while preserving any uncertain durable intent ID.
- Native store startup does not run a structural integrity/readiness gate before returning a writer. Present per-document/asset validation and archive checks protect their own boundaries, but do not establish that the whole live database is structurally sound after device reconnection.
- The service collapses storage failures to a message plus `outcome_unknown`; it does not preserve SQLite extended I/O/corruption codes or expose a recovery-specific UI state. A missing device, malformed database and ordinary validation error require different admission/retry behavior.
- There is no qualified independent durable recovery checkpoint outside the removable store's failure domain. Transaction receipts inside the same damaged file cannot replace an external checkpoint. Use consistent SQLite snapshots or complete validated native archives, with recoverable generation retention and explicit freshness.
- `synchronous=FULL` is not an explicit macOS `F_FULLFSYNC` policy. SQLite documents `fullfsync` as a separate, default-off flag on macOS. Evaluate and verify it for store, journal/archive and directory durability, alongside actual filesystem/device behavior; changing a pragma is not evidence that this failure is fixed. [SQLite fullfsync](https://www.sqlite.org/pragma.html#pragma_fullfsync)

The export source path uses a readonly transaction; the observed interrupted export did not intentionally mutate the source store. These bytes alone cannot determine whether absent tail pages reflect filesystem/device behavior, flush semantics, an earlier incomplete write, another process, or stale descriptor effects. SQLite's durability model depends on filesystem and storage guarantees. Avoid treating an automatic switch to WAL or any single synchronization setting as a proven remedy. [SQLite atomic-commit assumptions](https://www.sqlite.org/atomiccommit.html)

A follow-up implementation gate should inject I/O failures, disappearing/replaced roots and reconnect generations in isolated synthetic stores. It must demonstrate prompt UI responsiveness, no creation of an accidental empty replacement store, no reuse of stale database handles, preserved uncertain intent identity, explicit corrupted-store quarantine and validated recovery to a new destination. Physical disconnect tests need their own pre/post hashes, sidecar capture and independent checkpoint; prior synthetic transaction tests do not qualify removable-drive survival.

## Follow-up production source audit: bounded retirement and explicit reconnect

This independent review read the frozen store, session service, app and existing-open tests without running Cargo or changing production files. No additional blocker was found in the implemented **direct typed SQLite failure** path. This is acceptance of that limited behavior, not acceptance of general storage-disconnect resilience.

The reviewed source bytes are pinned here:

| File | SHA-256 |
|---|---|
| `crates/omniusong-store/src/lib.rs` | `7bff7d65b5f0ed3f14ee4c428799381a68baea922c13d087c366992691b45b06` |
| `crates/omniusong-store/tests/existing_open.rs` | `7817a27a70e43c61073458f75e1460a6a393f5b3b2cffa7281f9ac2a85e6d773` |
| `crates/omniusong-session/src/lib.rs` | `d4f9d138b966fe55b9d6a5bca10b049508ee862ab4eabacc0116a2f081ee114d` |
| `crates/omniusong-session/tests/service_review.rs` | `ec73b47d0b9d0494bbcb33d2a234e9f7f4b01c4e9c61b18081a3e3f556d1bef6` |
| `crates/omniusong-app/src/main.rs` | `213ddde8581f327a0ea0673cc7fef8bf53b6eb8ac55a19bc1afa9af0ab92aa52` |

`NativeStore::open_existing` passes `SQLITE_OPEN_READ_WRITE | SQLITE_OPEN_NO_MUTEX` without `CREATE` and does not call `create_dir_all`. A missing file cannot be silently provisioned through the SQLite open call even if it disappears between a precheck and open. Version zero is rejected before migration and rechecked under the migration transaction. Existing initialized older catalogs still use the ordinary migration transaction; this API is not a readonly forensic open. The new tests cover missing directory/file, empty and foreign version-zero files with unchanged bytes, and an initialized catalog's exact document/history reopening.

The service classifies only `StoreError::Database(rusqlite::Error::SqliteFailure(...))` with primary result byte 10, 11 or 26, including their extended codes, as `StorageUnavailable`. It does not infer corruption from an error message. The worker queues that request's known terminal completion and stops before receiving another request, dropping its backend on the worker. The client first matches generation, sequence and command kind; a stale or mismatched terminal packet cannot retire the genuine pending request. On a matching terminal result it clears pending state, retains a safe retry command and the exact known cause, and rejects later submissions to that generation. Producer EOF does not replace a queued terminal completion with a generic disconnect.

The app leaves the document and draft intact, stops polling the retired generation, and keeps the known failure visible. After initialization has succeeded, reconnect uses a new service generation and the existing-only open path. Reconnecting submits only Initialize; replaying a durable edit or copy is a distinct explicit action using the retained operation ID and payload. A failed initialization does not overwrite an already retained durable retry. The intended first-run path still permits provisioning before any store has successfully initialized; that in-memory distinction is not a persistent storage-enrollment identity. The manual recovery banner requires the exact root notice format/version and incomplete-history flag, renders a clear disclosure, and does not modify the document or interpret nested opaque metadata as recovery evidence.

The coordinator's `.runtime/rust-migration/disconnect-workspace-tests.log`, SHA-256 `2f6f6a478c06cae6e5a10e05fccc311915b8f8d936feb98b9f084dfdc061a46f`, reports 191 passing tests, no failures, and completion through the workspace doc-test sections. Relevant suites contain 3 existing-open tests, 7 service owner tests, 11 independent service tests and 13 app tests. The new independent tests include stale terminal correlation, a committed edit followed by terminal acknowledgement failure and same-ID replay without a duplicate undo step, missing-store reconnect, symlink-file rejection, and ordinary-error non-retirement. These are synthetic tests. This reviewer read the central log; the coordinator owns the command execution and final gate status.

### Remaining concrete limits

- **Wrapped error origin remains a production gap.** `archive.rs` wraps failures inside its export/restore operation bodies as `StoreError::Invalid` with diagnostic text; `archive_copy.rs` does the same for inspection and the late copy body. A native-catalog I/O/corruption error arising inside those wrappers therefore loses the typed cause and is not retired by the new classifier. Conversely, an external bad archive should not retire an otherwise healthy catalog merely because it contains SQLite corruption. Preserve structured cause and resource origin through those wrappers, then add injected native-catalog versus archive-source failure cases before claiming coverage of these paths. Early direct catalog failures in copy replay can still reach the new classifier.
- **The policy is deliberately narrower than all storage failure codes.** Ordinary `io::Error`, recording-layer errors, `SQLITE_CANTOPEN`, `READONLY`, `FULL`, `BUSY`, constraint failures and errors already converted to strings are not generally terminal under this classifier. The explicit reconnect precheck separately treats a missing/nonordinary/symlink database file as storage unavailable. A complete policy still needs typed origin and phase information; broad message matching would not close this gap correctly.
- **Path identity is not enrolled or revalidated.** Existing-only open prevents creation, but a valid replacement database at the same path, a changed ancestor/root mount or a symlink ancestor is not detected as a different storage identity. The service's leaf-file symlink check is a precheck, not an atomic no-follow open guarantee. Startup does not perform full structural integrity validation. These remain distinct from the verified no-creation property.
- **A stalled OS operation is not bounded by this state machine.** Retirement happens after a backend call returns a typed error. The two-second SQLite busy timeout is a lock-wait policy, not a deadline for filesystem open/read/sync or device stalls. The UI uses nonblocking service channels, but this review does not qualify cancelling a blocked storage operation, detecting a blocked generation, or safely opening a second writer while the first remains blocked.
- **Power-loss/removal durability and recovery checkpoints remain unqualified.** No new physical disconnect, power-loss, filesystem-flush, volume-replacement or independent checkpoint qualification was performed here. No change to `synchronous=FULL` establishes those properties. A responsive UI and a retired known-failed connection do not prove durable survival of accepted work.

The manual recovery candidate and its promotion acceptance above remain separate evidence from this production hardening. The banner makes that candidate's missing historical ledger visible; it is not an automatic recovery or repair implementation.

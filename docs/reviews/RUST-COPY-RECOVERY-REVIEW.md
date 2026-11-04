# Bounded copied recovery review

October 22, 2026. Contract: [bounded copy recovery](../contracts/BOUNDED-COPY-RECOVERY.md). Scope is fresh-action Rust recording/engine libraries. No source-action execution command, native UI, live migration or restart reconciliation is admitted by this review.

The coordinator owns the contract, acquisition anchors, physical-copy protocol, public integration and final checks. A separate architecture agent implements recovered-only finalization and reviews the outer protocol. An independent review agent owns the storage integration tests and reviews both modules. A separate engine agent owns six-stream admission and its integration cases. Agent reports were inspected; acceptance also requires the linked coordinator-run packet.

## Findings resolved before acceptance

- Opening a legacy Journal is already a derived mutation and its scans are unbounded. The new path never converts a bounded original reader into that writer. A separate private finalizer runs against the working copy with the original shared governor; interruption/index/manifest/state changes commit in one IMMEDIATE transaction.
- Original-row authentication alone excludes some derived metadata. Writable admission now requires the exact canonical journal-v1 schema, validates the current complete snapshot, authenticates interruption/index data and checks the entire recovered postimage. Unknown columns/triggers remain byte-preserved but refuse mutation. Schema rows are decoded within a fixed count/value bound and sorted in Rust.
- Raw copy hashing initially omitted its separate WorkBytes charge. Read, hash and write work now charge separately before their respective operations. The one sticky governor survives all readers, copies, SQL connections, semantic passes and post-commit proof. No limit reset or compiled ceiling increase occurs.
- A reopened pathname could identify a different working object. Bounded acquisition anchors file/owner identities before SQL; copying rejects changed identities; finalization retains the old file handle and compares device/inode plus owner identity across reopening. Sync uses the authenticated handle, followed by final identity/canonical-path/sidecar checks.
- A caller could put artifacts inside the original stage. The parent must be existing, canonical and ordinary, outside the source directory. Action directories and files are exclusively created; existing and incomplete actions refuse without overwrite or cleanup.
- Clippy identified a 160-byte error value. Artifact paths are now boxed together; the typed cause, boundary and every retention assertion remain unchanged. Independent review accepted this integration refinement.
- Cancellation/limits can occur after commit. The outer error retains artifact paths and a conservative RecoveryMayHaveWritten boundary before writable acquisition. Independent VM-limit cases require observing an actually committed recovered result through a separate read-only SQL oracle while the public operation reports refusal. No such error claims rollback or publishes success.

## Evidence gates

The [coordinator packet](../verification/rust-native-copy-recovery-tests.json) records source fingerprints, complete commands/log hashes, test totals and preservation snapshots. Independent storage cases cover full and shortened selections, bad/unselected tails, original SQL storage types/schema/sequence, sidecars, changed/replaced/aliased files and owner locks, unsupported schema, invalid requests, retained partial artifacts, cancellation and budget uncertainty. Engine cases exercise actual synthetic six-stream writer sources, both profiles, capture faults, retained Finish evidence, incomplete tails and exact pre-write semantic admission. A retained Finish still yields only recovered mode with unknown tail.

The first engine test attempt incorrectly used a new unknown table to exercise writable-schema refusal; the earlier read-only preflight correctly rejected that fixture before copying. The corrected case uses an extra ordinary BLOB column in an existing table and independently verifies its exact storage type/value in all three images. This was a test-fixture correction, not a relaxation of either reader or writer admission. The initial failure/log is retained separately in the packet.

## Remaining boundaries

File identity is implemented and tested on Unix; other platforms refuse copy recovery before artifact creation. Ordinary bounded read-only readers keep their previous non-Unix behavior. Parent directories and cooperative ownership are trusted; this is not a hostile-directory sandbox or power-loss guarantee.

The 1 GiB reservation covers the evidence database plus a working database capped at 512 MiB. It excludes filesystem allocation, empty lock/directory overhead and SQLite rollback sidecars, including retained failed journals. It is not a whole-directory disk quota. Work/row/VM limits can reject files well below the size ceiling. These small synthetic fixtures do not qualify sustained or thirty-minute recording/recovery capacity.

The low-level primitive has no durable Prepared authentication, executable service, restart/replay, outcome ledger, attachment, evidence archive or UI. The engine wrapper adds fresh source/working six-stream semantic equality and deterministic action/operation/review binding; it does not add normal-seal authority or callback-retirement proof. Existing actions always refuse and remain for future explicit reconciliation. Keep all native, listening, actual-input, installer and Windows acceptance gates separate.

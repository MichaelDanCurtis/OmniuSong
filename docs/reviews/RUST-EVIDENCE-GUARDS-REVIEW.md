# Reviewed evidence and error-domain review

This is a Rust library prerequisite for native studio-evidence controls. The installed 6ab792 app, live schema-6 catalog and originals remain separate; no new job or archive UI is claimed.

## Reviewed extraction

The new required-digest API validates the canonical lowercase SHA before creating output. It compares the digest of the same held, fully authenticated container read with the reviewed value before completion publication. It does not add a second read or replenish the governor. A valid but different archive is refused too. The unreviewed low-level API remains explicitly available for its original callers.

The final input anchor now follows object proof and completion serialization, immediately before marker writing. Independent fixtures cover archive/path substitution, changed output objects, surplus membership, canonical manifest and raw whole-container digest including footer. Exact catalog cells and raw source trees are independently compared before/after; extraction inserts no active identities.

Failure is not rollback. Tests inject cancellation/ordinary errors after the completion marker and retain exact complete inert evidence. A mismatch precedes the marker, but late failure can coexist with it. A future job must authenticate existing artifacts to reconcile; calling create-new restore again, changing destination or removing evidence would be incorrect.

## Resource origin and sticky stops

Typed export preserves the original StoreError and an explicit Evidence/LiveCatalog/BackupTransfer domain. Live connection PRAGMAs and required-link reads are annotated at their callers. Private snapshot connections, archived-catalog deserialization, historical replay and post-write archive validation remain local. SQLite Backup initialization/steps are mixed because a returned failure can involve either source or destination; destination-handle decoding is not proof of its physical cause.

The finalizer retains an already observed operation error/domain when the final governor check also refuses. The sticky stop remains separately typed and takes display/legacy-conversion precedence. The future session job can therefore preserve cancellation as its primary result while independently retiring a potentially failed catalog connection. The original error is boxed to keep the Result error small, without suppressing Clippy or changing public accessor semantics. The first size-lint refusal and final gates are retained in the verification packet.

Actual SQLite tests corrupt a rehashed archive's schema btree and observe a local CORRUPT error followed by healthy queries/replay on the same store. A test-only corrupt image on an isolated NativeStore's owned connection observes the LiveCatalog origin, retains its code through a later cancellation and leaves its physical catalog/originals exact. A real source write transaction produces a mixed Backup failure; rollback then allows normal export. These are synthetic library faults, not physical I/O or service-retirement acceptance.

The independent reviewer inspected the integrated production/test changes and exact call-site domains, with no implementation ownership, Cargo, UI or live database access. The [source gate](../verification/rust-native-evidence-guards-tests.json) supplies actual final commands and stable source/log fingerprints. Earlier focused tests passed eleven reviewed-restore cases and three actual SQLite-origin cases. The final source-stable gate passes **1,046 workspace tests / zero failures / seven intentional ignored helpers / 66 groups**, **47 optimized checks** (43 archive, three typed-failure and one six-stream session integration), formatting, all-target/all-feature warnings-denied Clippy and the default all-target build against **193 inputs**. Before/after verification retains every table/catalog byte, **22 project IDs / 403 prior rows / 80 protected files** and the exact installed 6ab792 binary.

A final independent read-only audit confirms all **206 unique referenced files** match their SHA/length records, all **193** pre/post source fingerprints are identical and current, and raw logs yield the stated totals. It also verifies the retained 168-byte error-size lint failure and boxed correction, corrected late-completion documentation, and exact preservation snapshots against the accepted schema-6 restart baseline. No source blocker was found. The auditor ran no Cargo, UI/device action or live database connection.

## Next integration boundary

The [native evidence-job contract](../contracts/NATIVE-EVIDENCE-JOBS.md) requires durable exact request/start records, separate primary/retirement outcomes, correlated cancellation, read-only existing-artifact reconciliation and guarded explicit native actions. Those service/job/UI components and an original-absent inert-output verifier remain unimplemented. OSRE preserves its declared catalog/three-tree scope; application job history is a separate local namespace and cannot be restored as authority.

No package, migration, native input, archive job, source adoption or cleanup is performed by this packet. Native UI and full-studio scale, thirty-minute capacity, hard power loss, Windows and the complete workstation release remain unqualified.

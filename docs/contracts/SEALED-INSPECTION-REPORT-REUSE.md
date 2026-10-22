# Authenticated sealed-report reuse

The read-only inspector may reuse the selected-prefix report produced by its own manifest verification, on the same connection and held read transaction. The report is internal, never caller supplied, persisted as authority, or used to grant native normal-seal permission. Public reader and manifest formats remain unchanged.

For a normal seal, the selected scan authenticates every declared ordinal, header, payload and index and compares the report with the exact manifest. Exact table cardinality excludes extra chunks. Every declared stream, including empty streams, must also match its durable prefix. This replaces a second complete scan; it does not remove those checks. Recovered seals retain interruption-receipt and selected-prefix-index verification and may retain an unselected damaged tail.

Inspection then reuses that same authenticated report. Full original-row hashing and whole-file SHA-256 remain separate complete passes. The engine still rereads authenticated chunks and verifies six-stream checkpoint, event, clock, Finish and audio semantics. Moving owned non-checkpoint chunks into a group avoids copying PCM; it does not change group ordering or verification.

One unchanged inspection governor covers all passes. Each finite-F32 audio traversal charges its full payload length before validation, in addition to the read/hash charges. Repeated validation charges again. Cancellation and work/row/chunk/record/group/VM refusals remain sticky and return no successful partial review. There is no persistent validation cache and no increased limit.

Regression evidence must compare reports, manifests, physical SHA and original-row SHA with independent direct-scan oracles; reject forged durable fields, extra/missing/reordered/corrupt normal chunks and forged interrupted indexes; retain opaque recovered tails; and prove cumulative charging and source preservation. Real retained long recordings are inspected on isolated copies before repeating the check in the installed app.

Diagnostic usage and the exact review hash may change because less work is performed. Previously stored review bytes remain immutable. Historical Prepared admission continues comparing source facts rather than usage counters; it must never silently rewrite an earlier review.

The 512 MiB file and 1 GiB cumulative-work ceilings remain unchanged. Passing the retained 106- and 122-second recordings does not establish thirty-minute capacity. Thirty minutes of 48-kHz stereo F32 alone is 691,200,000 bytes, before journal metadata, and needs a separately bounded segmented/progressive design and native qualification.

## Inspection notices

The UI retains one bounded inspection notice with its originating stage. Changing selection labels it as belonging to another original; it never relabels the result as a review of the new selection. A prior successful review remains explicitly separate. Cancellation targets the exact pending request, with worker credit held until its correlated completion. No per-source cache or recording mutation is introduced.

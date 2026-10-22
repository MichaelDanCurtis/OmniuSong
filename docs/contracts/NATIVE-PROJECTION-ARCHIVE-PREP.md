# Projection archive preparation

This Rust prerequisite adds streamed registered-source collection and canonical archive operation remapping. It does not deliver complete projection archives. All nonempty projection admission guards remain closed in OSAR export, identity restore, copy, lineage, NativeReader/list and unmigrated attachment paths. The full workstation goal remains open.

## Streamed source roles

`RegisteredSourceSet` consumes documents one at a time, retaining only exact take/manifest/stream references and their ordinary-recording or performance role. Any performance reference upgrades that take's role; it does not turn unrelated audio recordings into performance sources. Repeated references deduplicate retained state while traversal continues to charge cumulative work and record visits. Schema-1 lookalikes remain opaque.

The collector borrows its original inspection governor. Failed additions poison the set so ignored errors cannot admit a partial closure. `open_collected` consumes the set, resolves every exact reference against registration metadata and owns the held source readers. The holder retains the opening governor borrow even when empty; authentication cannot rebind prepaid work to a fresh account. Existing source-aware publication/edit/history callers use this same collector through their existing `open` convenience path. No budget ceiling is raised.

Collection is not source authentication, clip geometry validation or publication authority. The future archive walker must visit current state, all history sides, every operation result and all ancestor proof results; that walker and binding memo are not implemented by this slice.

## Exact operation syntax and copied identities

`ArchiveOperation` privately parses exact canonical edit, take-publication and performance-publication requests within the existing 65,536-byte wire ceiling. It validates the recorded operation/project/revision triple and remaps only the typed identities. Remapped encoding is bounded too.

Ordinary edit extensions are flattened. A canonical edit with `scope` and `edit` remains an edit even if opaque fields are named `projection_id`, `request`, `lanes`, `kind` or `version`, or an old operation ID uses a now-reserved prefix. Those fields retain their exact typed JSON values during copy. A malformed bare projection cannot gain projection authority by being mistaken for an edit.

A copied projection request derives its operation ID from the destination project and unchanged projection ID. Source pins, review digest, frame selection, lane choices, timing policy and schema-consent fields stay unchanged. Ordinary edit/take operations retain their existing fresh identity allocation. The mapping caller checks destination operations, copy admissions, pending mappings, source operations/copy admissions and the outer reserved request ID. Version-1 source archives do not require missing lineage tables.

These checks do not reserve a projection, authenticate its original music or validate its initial candidate. Existing guarded archive request validation explicitly refuses typed projection publication until complete source closure is integrated. Existing generic note/clip postimage validation remains active. Request hashes, result hashes and immutable lineage still use their existing format; no archive, catalog or musical schema migration occurs.

## Qualification and remaining integration

Focused cases exercise source-role union, opaque legacy fields, cumulative stops, failed-collection refusal, conflicting registration references, empty-holder governor identity, canonical requests, strict versions/fields, exact remapping, actual SQLite collision checks and bounded output. A public ordinary-project regression exports, restores exact identities, copies, replays and reexports while preserving flattened projection-looking extensions. Existing real synthetic performance publication tests exercise the refactored source holder separately. Exact counts and preservation belong to the verification packet after execution.

Complete archive support still requires the [shared semantic design](../reviews/RUST-PROJECTION-ARCHIVE-DESIGN.md): full streamed snapshot traversal, one authenticated binding memo, typed original publication/result/history linkage, private archive SQL error isolation, final expected postimage comparisons, typed completion/cleanup outcomes, and the separate schema-3 inert OSRE roundtrip. The coordinator selected authentication of final installed source paths for the later restore route, consistent with existing diagnosed staging/orphan retention. No staged reader may silently follow relocation or gain new authority.

Activate export, identity restore and copy together after those checks pass. Then integrate guarded readers/attachments, durable exact-request native admission and a named-binary walkthrough. This source-only prerequisite opens no native app or device, installs no bundle and qualifies no inference, listening, MIDI transport, sustained recording or Windows behavior.

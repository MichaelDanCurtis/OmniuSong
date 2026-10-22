# Held performance bindings and complete candidates

October 22, 2026. Stage 3 prerequisite of the [integration plan](../reviews/RUST-PERFORMANCE-PROJECTION-PLAN.md), following [schema-3 ownership](NATIVE-PERFORMANCE-PROJECTION-OWNERSHIP.md). This library contract creates no store publication, archive adoption or native conversion authority.

## Held original and cumulative work

`AuthenticatedPerformance` borrows one bounded sealed reader. Private fields retain the authenticated source inspection, descriptor/profile and exact source pin. Construction performs complete existing snapshot and six-stream semantic inspection and agrees the selected complete checkpoint with the sealed manifest. Caller-supplied inspection records cannot construct a proof. The context cannot be cloned or deserialized.

`project` rederives selected control events using this held original, its original frame/stream/checkpoint locations and the existing lifecycle. Different interpretations reuse the authenticated inspection while consuming the same cumulative reader governor. `build_binding` derives once, compares the exact expected review digest, constructs all groups and checks the final source identity. `validate_binding` strictly decodes a retained record, then derives the full review from the actual held original and compares every typed field. Final cancellation/work and source-identity checks occur before successful return. No work/file ceiling is raised and no budget is reset per interpretation.

The existing free `project_sealed_performance_bounded` function remains a request-shape check followed by context construction and projection. Its review format, lifecycle and original-byte compatibility remain unchanged. This is procedural reconstruction of saved performance events, not audio transcription or AI inference.

## Portable encoding versus source authentication

`check_retained_binding_encoding_bounded` checks only internal portable consistency. It strictly decodes the complete review DTO, reproduces its exact compact typed JSON and domain-separated review digest, checks the owned audio take reference, and verifies the complete original-note/source/instrument/lane bijection. Unknown fields, alternate JSON layout/whitespace, missing or extra lanes, substituted IDs/values, duplicate original identities and malformed digests refuse. Bounded prepayment covers decoding, allocation, copies, canonical serialization and hashing under the supplied governor.

A coherently rehashed false review can pass this encoding check. This returns no original-authentication token and cannot activate a project. Held `validate_binding` must reject any such review that differs from actual rederivation, even if all stored hashes, note IDs and values agree internally. The distinction remains explicit for future historical-commit receipts: catalog consistency alone cannot authenticate an absent original.

## Complete lane choices and stable identities

Portable `ProjectionLaneChoice` owns strict camelCase fields: source, original instrument, new track ID and display name. Choices cover exactly the nonempty selected `(source, original instrument)` groups, sorted by source then Keys, Bass, Drums, Pad. Names are nonblank, contain no controls and fit 160 Unicode characters and 640 UTF-8 bytes. Source handles remain 1–4; strict native identifiers fit 96 ASCII characters. All 16 possible groups and all existing aggregate project/review/note limits remain in effect.

A derived document note ID is `pn-` plus the lowercase SHA-256 of the domain `OmniuSong.performance-projection-document-note.v1\0`, followed by two fields. Each field is framed by its unsigned 64-bit little-endian byte length, then its bytes: exact projection ID, then original voice-identity SHA-256 ASCII. Projection IDs distinguish separate explicit interpretations. Track destinations and project IDs do not change this original identity.

`build_binding` retains the exact compact review, owned original audio reference, complete original values/voice identities and their lane grouping. It accepts no caller-authored notes or note IDs. Lanes retain original instruments even if a musician later changes the current editable patch.

## Pure complete document construction

`ProjectDocument::append_performance_projection` takes a complete binding and exact lane choices and returns a new candidate. Core shape validation grants no source authority: the future store must derive the binding itself. The constructor appends one registry entry and every new ordinary notes lane together. Lanes start muted, unarmed and unlocked with conventional gain, exact 16-bit velocities (including zero), no fabricated expression and procedural provenance. Existing tracks, source PCM, opaque metadata, project identity and revision remain exact. No note array is installed into an existing lane.

Schema 1/2 requires explicit projection-schema promotion, which refuses any old same-named opaque registry key. Schema 3 remains separate from SQLite schema 6. Existing or deleted reserved lane/note identities cannot be reused. Duplicate projection identity, incomplete/reordered destinations, invalid names, track capacity or complete metadata overflow refuse without changing the original document. Preserve-loop retains the loop; extend-loop rounds the maximum note end upward to complete 960-PPQ four-beat bars, never trims and retains native timeline bounds.

Current note values, names, instruments and tempo may later change or notes/lanes may disappear. Immutable original binding/source ownership remains. Those edits do not claim the current music still matches the original interpretation.

## Remaining required integration

Nonempty registries remain refused by active store and OSAR admission. A pure candidate is not a committed operation. Next implement registered-source opening with one authentication pass per distinct source, typed portable publication, same-transaction document/history/receipt commitment, exact replay distinct from current usability and authenticated retained-history transitions. Generic Save must preserve an admitted registry exactly. Keep source-aware archive current/history/result/copy proof, inert OSRE closure and guarded NativeReader activation as separate qualified paths.

Durable exact-request session admission, root/generation/current-context correlation, native preview/confirm/retry, editable history/archive/restart and explicit original-versus-derived audition remain required. No new bundle, UI, native device action, regular project promotion, listening, actual inference or Windows qualification is delivered by these library methods.

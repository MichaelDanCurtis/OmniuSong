# Performance projection document ownership

October 22, 2026. Stage 2 of the [integration plan](../reviews/RUST-PERFORMANCE-PROJECTION-PLAN.md), following the [authenticated read-only kernel](NATIVE-PERFORMANCE-PROJECTION.md). Musical schema 3 is distinct from SQLite schema 6. This contract does not authorize projection publication, archive adoption or native conversion controls.

## Explicit format and compatibility

Musical schema 3 requires a root `performanceProjections` object with version 1 and an `entries` array, including when it is empty. Schemas 1 and 2 retain a same-named unknown field exactly and do not interpret it as ownership. Explicit promotion from either older schema refuses any preexisting key, including null, rather than overwriting user metadata. Promotion changes only the schema and installs the empty registry; revision/history remain the store's responsibility.

`supports_native_takes()` recognizes musical schemas 2 and 3. The explicit original native upgrade still targets schema 2. Schema 3 retains the existing native recording payload, 8 MiB whole-document metadata limit, native timeline limits and ordinary notes/editor/render paths. Readers must use the capability rather than exact schema-2 equality. Legacy import remains schema-1-only.

## Strict ownership envelope

Owned outer records use camelCase keys and reject unknown fields and unsupported versions. A registry entry has a stable projection ID, version 1, an audio-stream `TakeReference`, a retained review envelope and original lane/note bindings. It contains no project or operation ID; future archive copies preserve these musical/source identities while remapping their separately owned project/operation records.

The retained review has version 1, `canonicalJson` and `reviewSha256`. Its JSON string preserves the exact typed serializer's bytes and key order. Core checks only the bounded common envelope: JSON object/version, embedded `review_sha256`, and embedded request source take/manifest agreement. Core does not authenticate the kernel's domain-separated digest, full review DTO, canonical serialization, source lifecycle or musical derivation. A plain hash of the complete JSON bytes would be a different identity and is not substituted for the kernel digest.

The shared performance layer and future publication/archive consumers must deserialize the full strict DTO, verify exact typed serialization and its existing digest convention, and recompute from the exact held owned original. A structurally valid core entry, caller-authored JSON or supplied digest does not grant any of these authorities.

Each original lane binds a local source handle, original instrument (`keys`, `bass`, `drums` or `pad`), track ID and nonempty original-note list. Each original note binds its strict note ID, original voice-identity SHA-256 and exact `NoteValues`. Initial typed publication must match the complete review and requested original lane choices. Subsequent musical edits may change current note values, current instrument/name or remove notes/tracks without rewriting the original interpretation.

Limits are 64 entries, 16 nonempty lanes per entry, 20,000 original-note bindings across the registry and 8 MiB of retained review JSON across the registry. Lane `(source, original instrument)` pairs are unique within an entry. Projection IDs, reserved lane IDs and bound note IDs are unique across the registry. Original voice-identity hashes are unique within an entry but may recur in another explicit projection of the same source. Existing whole-document limits still apply after JSON string escaping and ordinary metadata are included; a kernel review within its own bound is not guaranteed to fit a project.

## Editing and source retention

An absent bound note or lane is legal. A present reserved lane must remain a notes track. A reserved note ID may appear only in its original lane, with any valid current musical values; a matching numeric pitch or note array position does not establish identity. `add_note` rejects all reserved original note IDs, including after deletion and attempts in another lane. Future lane-creation operations must reject a missing reserved lane ID. Undo restores a removed note through authenticated history instead of creating a new performance identity.

The registry retains its typed source independently of any audible original clip and current derived notes. `referenced_takes()` includes both recording clips and registry sources. Store file verification must group all such references while separately checking every clip/current-original window. Removing notes or the audible original never silently drops the physical original from eventual archive closure.

## Staged admission boundary

Until typed publication and shared archive semantic validation are implemented, active store paths and song OSAR export/restore/copy/replay paths refuse every nonempty registry. Empty schema 3 remains usable with ordinary notes and native recordings. Guard current metadata and candidate/target snapshots, operation replay/results, every history side and copy-lineage proof, including when the current project is older or empty. History navigation validates the stored before/after pair in forward order, rather than treating Undo as a fresh promotion; a forged Redo cannot bypass opaque-key collision refusal. Generic schema-2 Save must not erase unsupported current ownership. Header/snapshot-hash OSAR inspection retains its existing bounded scope; an inspection summary does not prove that all hidden history/proof snapshots have been semantically validated or authorize adoption. List reports an unsupported session as a problem instead of presenting it as healthy.

Historical exact replay is checked before fresh target/source execution. Fresh normal and recovered attachment preflight current metadata capability before source ingestion, then repeat their complete revision/source/history checks at commit. Metadata preflight opens no original and grants no publication or source-authentication authority. A later concurrent failure can still leave an explicitly retained owned object; this is not a rollback guarantee.

The full-root OSRE format remains inert forensic preservation. It retains raw catalog and source bytes under its existing declared closure and must not convert a restored registry, receipt or source identity into active authority. It does not claim musical projection revalidation. The separate active song archive/copy boundary remains closed to nonempty registries at this stage.

## Qualification and remaining delivery

Qualify strict versions/fields/bounds, lossless old opaque metadata, collision-safe promotion, edited/deleted bindings and reserved-ID reuse refusal. Exercise empty-schema-3 native recording ownership through actual device-free journals, save/history/reopen and OSAR identity/copy restoration. Independently forge structurally plausible nonempty registries and hide them in current, history, operation results or copy proofs; active publication/adoption must refuse while preserving source/catalog state.

This format stage adds no SQL migration, native conversion UI, new bundle, device input/output, inference, calibrated timing, long recording or Windows acceptance. Atomic recomputed publication, durable exact-request uncertainty, complete projection OSAR/OSRE/copy closure and actual native conversion/edit/history/archive/restart/audition remain mandatory stages of the full Rust workstation goal.

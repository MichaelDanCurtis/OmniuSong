# Existing evidence reconciliation review

October 22, 2026. This packet adds Rust library verification of existing artifacts, without creating a job, native archive control, package, migration or active restored identity. The installed 6ab792 app and live schema-6 studio remain separate.

## Recorded export

`reconcile_evidence_export` authenticates only the recorded existing OSRE path and compares its export ID and exact historical origin string. It never calls fresh export or consults that origin. Bounded UTF-8 path/provenance admission and metadata credit precede name cloning and inspection. Missing, partial, wrong-ID and wrong-origin archives refuse unchanged. Real rehashed archived SQLite corruption remains local, with subsequent healthy queries and valid replay using the same live store.

## Completed inert extraction

`inspect_restored_evidence` takes the recorded destination, exact original archive pathname and reviewed whole-container SHA. The original archive and live source may both be absent. Header, canonical manifest, original entry-indexed numbered objects, content digest and footer reconstruct the complete OSRE bytes. Every object and known catalog reference is validated. Strict receipt fields and lexical input/output paths are checked independently: Path equality would normalize some aliases, and a marker pathname lies outside the content hash.

Metadata-only directory/type/privacy checks and names enumeration precede anchored root/objects descriptor creation and all data opens. Enumeration itself may use directory handles; it does not open an accidentally selected regular-file SQLite member. Sequential locked data handles avoid a 2048-descriptor requirement. The process lease outlives them. Retained file stamps and full directory identity/mode/link/mtime/ctime checks detect substitutions and transient membership changes within the existing Unix trusted-directory/cooperative-writer boundary. This is not an atomic snapshot guarantee against arbitrary external writers.

Completion JSON is bounded to 16 KiB, manifest JSON to 2 MiB. Input and canonical buffers are reserved before decoding; names, request text, metadata, catalog allocation/deserialization, reads and both hash streams receive cumulative credit. The reconstructed complete archive is observed against the original file ceiling, rather than permitting individually bounded objects to bypass it. Cancellation and every limit remain sticky, including the final checkpoint.

## Independent evidence

A separate Astra architecture review specified the exact historical path parameter, metadata-only wrong-type preflight, full directory stamps, entry-index numbering, reconstructed-size ceiling, pre-allocation credit and the distinction between present proof and an earlier acknowledgement. A separate Sol reviewer found and followed corrections for component-normalizing paths, unbounded export arguments, name allocation credit and a confounded SQLite test oracle. No reviewer ran Cargo, UI, devices or live-database connections.

The corrected wrong-objects and wrong-native-catalog tests hold real SQLite transactions while separate processes probe exclusion before and after refusal. Raw tree/hash oracles run outside the active transaction, so their descriptor closes cannot fake lock preservation. Independent framing/checksum oracles reconstruct raw containers. A fully rehashed marker/manifest/object set with one required original omitted must still fail the actual archived catalog's reference closure. The strengthened real six-stream recovery/choice/attachment fixture uses reviewed restore, both reconciliation APIs and original-archive-absent verification while comparing exact raw trees and histories.

The [verification packet](../verification/rust-native-evidence-reconciliation-tests.json) records the final root-run commands, stable source fingerprints and preservation comparison. Focused reconciliation tests passed **18 / zero failures** before that combined gate. The final gate passes **1,064 workspace tests / zero failures / seven intentional ignored helpers / 66 groups**, **65 optimized checks** (61 archive, three typed-failure and one actual six-stream integration), formatting, all-target/all-feature warnings-denied Clippy and the default all-target build against **198 stable inputs**. All live tables/catalog bytes, **22 project IDs / 403 prior rows / 80 protected files** and the exact installed 6ab792 binary remain unchanged.

A final separate read-only audit verifies **210 unique current referenced files**, all **198** source fingerprints, raw command/test totals and exact preservation against the previously accepted schema-6 baseline. All eleven named regressions appear in workspace and corresponding optimized logs. No source or evidence blocker was found. The audit opened no live database and ran no Cargo, UI or devices.

## Remaining delivery

Success proves matching complete artifacts exist at the recorded location now. It does not prove the earlier caller observed acknowledgement or a durability step. Missing/invalid completion remains incomplete, with no marker synthesis, repair, deletion, adoption or create-new retry.

Next implement bounded durable request/start/terminal/observation records with their own metadata ownership, then integrate exact cancellation and distinct primary/catalog-retirement outcomes through the sole session service. Startup loads metadata only. Explicit guarded native Export/Inspect/inert Restore and restart reconciliation require their own installed-app walkthrough. Full-studio scale, thirty-minute recording, source adoption/attachment controls, physical/listening checks, MIDI 2/MPE, inference/plugins, installers, Windows and the complete goal remain unfinished.

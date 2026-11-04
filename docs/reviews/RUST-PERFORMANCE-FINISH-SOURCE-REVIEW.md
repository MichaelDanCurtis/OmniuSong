# Performance Finish prerequisite — independent source and artifact acceptance

October 20, 2026 (final gate recorded October 21 UTC). **Accepted for the bounded library prerequisite:** an admission-owned Finish fence, a complete callback cut, explicit v2 originals, interrupted recovery and complete archive restoration. No remaining source blocker was identified in this reviewed scope. This does **not** accept native saved-performance recording, normal sealing, worker commands, UI or release readiness.

The governing [Finish contract](../contracts/NATIVE-PERFORMANCE-FINISH.md) and [architecture proposal](RUST-PERFORMANCE-LIFECYCLE-ARCHITECTURE.md) retain the later integration requirements. This lane edited its independent tests/reviews, read source and isolated evidence, and ran read-only Python/SQLite/hash checks. It ran no Cargo command and opened no native device, GUI or live catalog.

## Source disposition

`LiveAdmission::begin_finish` captures its own accepted sequence; callers supply scope and ID, not a watermark. Same-ID retry retains the fence and does not extend the deadline again. Admission/Panic close permanently. The bounded deadline CAS preserves an already longer live deadline, rejects the expiry latch and never removes a first fault (`live.rs:246,792`). The request is sampled before the callback boundary. Each invocation still disposes at most four batches; healthy Finish requires the complete same-scope applied fence and an empty ingress queue (`live.rs:1257`).

The tap reserves the full possible 71-packet invocation before event application. The renderer preserves full logical F32 PCM, counts held/releasing voices, hard-cuts voices and sustain, and emits Finish before End. The callback ACK is published only after a complete retained End (`live.rs:1332`, `performance_tap.rs:593`). Later valid buffers are silent without advancing the recorded endpoint; malformed later buffers and late asynchronous faults remain faults. No human NoteOff, release velocity or physical timing is invented.

The review identified one concrete publication race: progress originally sampled mutable voice counts before acquiring a fresh Finish ACK. The owner corrected it to acquire the immutable cut first and derive the terminal frame/applied fence and zero counts from that record (`live.rs:575`). A deterministic owner regression deliberately supplies incompatible older counters after ACK; final tests pass. Explicit `Finishing`/`FinishedCut` validity matches the contract. `finish_ack` independently vetoes late fault/cancellation (`live.rs:828`). The existing native worker still creates ordinary audition pairs; recorded worker commands and these new states need a negotiated wire-profile change when integrated.

The writer checks the explicit profile, source/scope/applied fence, complete PCM, single terminal ordering, zero post-cut counts and absence of known/pending capture fault. It rejects subsequent callbacks (`performance_writer.rs:143,376`). The scanner rejects contradictory rehashed terminal groups and retains the preceding valid checkpoint with all later original rows preserved (`performance_writer.rs:1246`). V1 creation and fixed descriptor/first-Begin byte goldens remain exact; v1 Finish, mixed/null/future owned profiles and generic recovery/sealing of either owned marker reject. This is owned codec evolution, not a journal-schema migration. A valid v2 Finish checkpoint still recovers with an interrupted manifest and unknown tail.

No public normal-seal capability was added. Actual native stream pause/drop, renderer/error-closure destruction, owner drain, final fault veto and the normal-seal authorization/commit boundary remain required. The producer wrapper is dropped before renderer destruction acknowledgment; offline destructor assertions do not qualify actual native closure lifetime.

## Executed gates and retained failure history

The coordinator's [gate2 receipt](../../.runtime/rust-performance-finish/gate2-run.json) binds **123 source snapshots and four logs**. This review independently rehashed all 127 records against current files; all sizes and SHA-256 values match. Parsing the complete workspace log gives **666 passed, 0 failed, 3 ignored helper entrypoints, 53 result groups**. All final commands exited zero:

- `cargo fmt --all -- --check`.
- `cargo test --offline --workspace --all-features --locked`.
- `cargo clippy --offline --workspace --all-targets --all-features --locked -- -D warnings`.
- `cargo build --offline --workspace --all-targets --locked` with production/default feature selection.

The final gate includes 83 engine unit cases and 23 new integration cases: eight codec, seven renderer and eight independent Finish tests. Independent cases cover sixteen accepted batches, exact originals/PCM/cut, empty fence, late-fault ACK veto, retained Finish without End, five forged packet variants, four generically valid rehashed terminal contradictions, v1 refusal and post-cut callback refusal. The independent TLS allocator case exercises the actual final 16,384-frame/eight-channel callback with 64 held voices, four batches and all 71 packets, then a 2,049-frame silent callback: **zero allocations and zero deallocations in both measured sections**. Setup, buffers, admission and draining remain outside those sections. This measurement does not prove arbitrary driver/error paths are real-time safe.

The [first workspace gate](../../.runtime/rust-performance-finish/gate1-workspace.log) is retained. It failed only two inherited assertions that equated the old 70-packet ordinary callback with the now 71-slot maximum reservation. The reviewed correction asserts 70 actual packets when Finish is absent and 71 maximum when its optional record is included. No production behavior was changed for that repair. The final full gate reran both tests and all new cases; no source delta occurred during it. Earlier focused 83-unit/23-integration results are preliminary evidence, not substituted for the final run.

## Actual isolated qualifier audit

The coordinator executed [qualify_performance_finish.rs](../../crates/omniusong-engine/examples/qualify_performance_finish.rs) to completion, producing [proof1.json](../../.runtime/rust-performance-finish/proof1.json). Its resource log records **0.99 seconds wall time and 16,596,992 bytes maximum RSS** for this small synthetic run. No latency, throughput ceiling or long-recording result is inferred.

The production renderer/tap/writer processed **2,560 stereo frames at 48 kHz**, in four callbacks of 512, 256, 1,024 and 768 frames. There are 35 tap packets, sixteen complete applied batches, sixteen exact NoteOn originals across the four declared local sources, and one Finish at callback four/frame 2,560. Each source has four originals. All observation ticks remain exact above 2^53; the declared GUI clock ratio is 1,000,000,000/3, separate from the 48-kHz output-frame clock. Final cut counts are sixteen voices and zero release tails; the matching End has zero voices/tails and applied sequence sixteen. The source record list contains no synthetic NoteOff. The following native-shaped buffer is silent and does not extend recording, as asserted by the executed qualifier.

Independent read-only Python inspection established:

1. All 25 chunk checksums, the selected index, exact v2 checkpoint member list/hash and all five raw-stream envelope digests agree. The descriptor and manifest hashes recompute correctly. Local/Uninterpreted envelopes carry no alignment or channel, exact source/session identities and exact callback-applied frames. All sixteen retained full batches and source records equal the receipt originals.
2. The 20,480 PCM bytes reconstructed from journal chunks equal the WAV data bytes exactly. The RIFF sizes, IEEE-float format, stereo/rate/block alignment and `fact` frame count are valid; all samples are finite and the output is nonzero. The codec profile is 2 throughout this take.
3. The type-tagged original-row digest independently recomputes to the pre-recovery review value, `727ca8e391ec05169f2d3ea88870069f3d684c230da4f0f392dfe645fa347612`. Explicit selected recovery/replay retains the Finish and an interrupted manifest with **unknown tail**. No normal manifest is claimed.
4. Every row in all **seven** journal tables is equal across the recovered stage, owned store object and restored object: `chunks` (25), `interruptions` (1), `recovery_index` (25), `sealed_manifests` (1), `sqlite_sequence` (1), `stream_prefix` (6), and `take_meta` (1). These are six application tables plus SQLite's sequence table. Stage and owned physical file hashes may differ after SQLite backup; their exact rows agree and the owned/restored file bytes agree.
5. The archive's one checksummed object chunk reconstructs the exact 143,360-byte owned journal. The 258,048-byte archive contains one history row, one operation and one take. The publication/Undo/Redo/restored project revisions are **0 → 1 → 2 → 3 → 3**; Undo restores initial tracks, Redo restores attached tracks, and the restored document equals the final source document. All ten source/restored catalog tables compare equal. Only the isolated fixture catalogs were opened.
6. All fourteen fixture/proof files had identical hashes before and after this audit. The separate [coordinator artifact audit](../../.runtime/rust-performance-finish/proof1-root-audit.json) agrees with these results.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Final gate receipt | 24,688 | `9fffadd5bd0b07b1ddf1ef9c60ff0085a8184a28dcf637c9210da7619a87c9ae` |
| Executed qualifier receipt | 58,544 | `559d109d8e3d989adb380503150a1a48657067f9ff45a4f048531fb8e3626d29` |
| Logical F32 PCM | 20,480 | `89a22320425fdcd2f52f6f45cc28d29ede5a31da3e0726fa6841faaa94b2d4b3` |
| F32 WAV | 20,536 | `90f821d1b6138fd459c8f545b7c3601dec7d8efca1c796feb599bb7dac7b5e63` |
| Recovered original stage | 143,360 | `bc53b5fcd7565c204655a12d14af88358bf455775ed5d3e3319e536148a921e9` |
| Owned/restored take | 143,360 | `7d04674630d07d5704fd1bd7e9d7ddf45a71d6c8c87be43f620a9b1d367a3981` |
| Complete native archive | 258,048 | `69e3fffb82344c6d1e1121e0baa81ae06a28cb72000b6277134a433b811284a2` |

## Reviewed source identity and remaining gates

The complete binding is the 123-entry gate receipt; key final inputs are:

| Source | Bytes | SHA-256 |
| --- | ---: | --- |
| `engine/src/live.rs` | 67,816 | `9d7a271022eff6407a9e00de7b709d49f614f1554b5e4a9d1c97520d865b7c01` |
| `engine/src/live_types.rs` | 18,256 | `6b18c0bc6bd5b1420ade3ec94c4f01f81305199fc983a595e2656011f430a92f` |
| `engine/src/performance_tap.rs` | 49,597 | `3c3ef126006c863db37e1f9ecda5a792354df2ecbf03025592d7f4d009f0a0be` |
| `engine/src/performance_records.rs` | 54,968 | `cd00a1befd5387e4e2fab7e5b84c68e6c40ae10672a577b7841ecc0d488ea57f` |
| `engine/src/performance_writer.rs` | 60,579 | `627ef0d783bdca355949995e8533e72f8b99f4df4024bdf7248da0c9c475fae6` |
| `engine/tests/performance_finish_review.rs` | 27,252 | `e9e9be7ef2b98d908f84fb5820397970a1d3bcaac19193414cb0da789dfee271` |
| `engine/examples/qualify_performance_finish.rs` | 18,323 | `de0d3a2637bce675283348d24c0c49ad3b6e719056ce4060cca97c75f0253dae` |
| `docs/contracts/NATIVE-PERFORMANCE-FINISH.md` | 13,072 | `d5b0fce20f0cfbcfefc1b23bab1d5827581b51eff967e9ee50bb0173382ba756` |

There is **no new native package, signature or UI acceptance** in this packet. The qualifier deliberately creates no native error closure and claims no normal-seal authority. The actual saved-performance worker must still preserve earlier parent-enqueued batches before its healthy fence, expose the early stage identity, remain responsive to Stop/EOF, prove real callback/error-closure teardown, drain the persistent writer and obtain private normal-seal authority with exact retry semantics. UI/recovery/attachment, explicit derived-note provenance and human performance remain required. Physical/listening, latency, thirty-minute stability, MIDI 2/MPE, inference/plugins, installers, Windows and the full workstation goal remain separate, open gates.

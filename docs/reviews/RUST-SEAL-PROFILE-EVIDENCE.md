# Seal profile and package evidence audit

October 22, 2026 UTC. This is a bounded artifact audit by the author of the statement-reuse change, not independent approval of that code. Only this review document was written. No Cargo, formatter, application/device/UI operation, source change, recovery operation or packaging/signing mutation was performed.

**Disposition:** the inspected source, profile, test and package records are internally consistent and their current raw-file identities match. The final overflow-checked optimized helper measured **2.602231833 seconds** for equivalent four-phase storage work on the retained 39.434667-second source. The exact optimized package is identified below. Its fresh native Record/Finish/normal-seal/attachment/playback/history/archive/restart workflow remains **unqualified**.

## Diagnostic failure and retained original

The [native diagnostic receipt](../../.runtime/rust-seal-fix/diagnostic-native.json), adjacent `01-diagnostic-finish.txt`, `02-diagnostic-outcome.txt` and `03-diagnostic-kept-local.txt` retain this new run's actual boundary:

- Stage `54ceabeb8390487bbe9b83cd7e58d509`, project `44107bb9-122d-4454-88fb-4cef2f6fb0ff`.
- 1,892,864 durable/rendered frames, 3,697 complete callbacks, 48 kHz: 39.434666667 seconds.
- The visible first failure is exactly “Recorded output sealing deadline expired; worker retired and original retained.” No normal seal or attachment is recorded. Keep locally and app exit are recorded.
- Intent revision 1 has no Finish; revision 2 retains the exact Finish identity/scope; revision 3 retains that Finish and releases the local review without attachment. No new authority follows from these stored choices.
- The diagnostic executable was SHA-256 `cbb6cae38a914f409df8af37b6dddff706e940a59cf8b7bab6de9e0f9898ed11`, 94,072,496 bytes. Its exact bytes remain at `dist/OmniuSong Native.previous-20261021T235644.app/Contents/MacOS/OmniuSongNative`. The former current-bundle path in the old receipt is a replaced alias, not a claim that the optimized executable has this digest.

The new diagnostic identifies this run's deadline retirement. It does not retrospectively identify the earlier 18-second generic retirement. The ten-second sealing deadline is unchanged.

The source is `.runtime/native-studio/captures/54ceabeb8390487bbe9b83cd7e58d509/take.sqlite3`, **32,436,224 bytes**. A fresh streaming physical hash matches every receipt. Its directory still contains exactly `take.sqlite3` and `take.sqlite3.owner.lock`; the owner file is empty. Read-only SQL confirms the source has no sealed manifest. The audit opened it and profile clones through read-only immutable SQLite URIs after checking physical identities; no source or sidecar was modified.

## Common identities and complete comparisons

All four profiles have identical source-before/source-after identities, directory names, counts, Finish cut/fence/scope, and these identities:

| Identity | SHA-256 |
| --- | --- |
| Physical source | `9606c000cce047980fea6df8af8006d29ea41a45d750e6c60ee1fb79a2e0668e` |
| Descriptor | `3c170e15a5381d04135bde184f98360355155a12f9b057d907005cbd06d2032c` |
| Complete original rows, v1 | `880bce04c02c07b7d4c7429eccb13bb4e8c0bd4dd1511c0a14fb0ad6a26f3b73` |
| Semantic checkpoint | `2b170e275b3589dda8e6ce8a52d67b82bdfb4ec2bad58bd0c06bb139ebe49e78` |
| All-chunk index | `e69a570679eaba7684fdc477053079bd261b7280263752bee3402c797d7153d8` |
| Complete-prefix receipt | `6efc12a0d586181bd76026fe4af576eb716a068dd093e330dfc08de0f36cc9be` |

The source and each clone have **7,556 physical chunk rows**, six stream-prefix rows, 20 schema rows, one take-meta row and one sequence row. Interruptions/recovery-index rows are zero. Each fresh clone adds one normal manifest. All receipts report `allSourceChunksCompared=true`, `originalUnchanged=true`, and no device, catalog, native authority or native lifecycle proof.

The audited helper source, `crates/omniusong-engine/src/performance_writer/seal_cost_tests.rs`, checks every source ordinal, full header/payload equality after sealing, original-row identity before and after, complete-prefix/manifest agreement, physical source bytes and directory names. These checks precede writing each successful receipt.

This audit additionally compared **every physical chunk row and field** in each of the four retained clones against the source, including exact header/payload bytes; compared schema, stream-prefix, sequence and original descriptor rows; and recomputed the v1 original-row digest and all-chunk index with a separate read-only Python encoder. All matched. Each retained clone's current physical SHA/length and stored manifest identity also match its own receipt. The shared pre-seal clone identity is `0710b2463940aca1e7621a76b921a1fda3241ea84ae4de87cbabf0039531c43c`, 32,436,224 bytes. Final clone/manifest hashes differ between runs as expected for fresh manifest identities; each final clone is 32,444,416 bytes.

The earlier preservation records were also checked: all **69 files / 41,947,235 bytes** in `before-files.json` and all **five immutable takes** in `before-store.json` still match their recorded SHA/lengths. This is raw-file preservation evidence, not a fresh catalog-row or physical-durability qualification.

The separate `bounded-diagnostic-source.json` agrees with the source, descriptor, v1 and checkpoint identities and reports no semantic problem/capture fault. Its recorded usage is 298,369,160 work bytes, 454,557 row visits, 15,112 chunk visits, 38,451 record visits, 155 groups, 7,584 physical rows and 2,001,019 VM steps. That inspection is evidence-only and confers no recovery, attachment or normal-seal authority.

## Measured phases and build distinction

Each timing is a single retained observation on this same source, not a distribution or wall-time guarantee. Adjacent logs show the named helper test passed once for each profile.

| Receipt under `.runtime/rust-seal-fix/` | Owned inspection | Held semantic scan | Complete prefix | Checked seal | Four-phase total |
| --- | ---: | ---: | ---: | ---: | ---: |
| `seal-cost-baseline-39s.json` — preserved original debug helper | 3.827114959 | 3.998127625 | 2.967589708 | 16.675031875 | **27.467864167** |
| `seal-cost-reused-39s.json` — reused statements, debug | 3.407877917 | 3.949549375 | 2.552964708 | 15.162225459 | **25.072617459** |
| `seal-cost-release-39s.json` — initial optimized profile | 0.321795959 | 0.436704041 | 0.257765000 | 1.523752959 | **2.540017959** |
| `seal-cost-release-overflow-39s.json` — final overflow-checked optimized profile | 0.333956500 | 0.452227667 | 0.262104666 | 1.553943000 | **2.602231833** |

Values are seconds. Source opening, source semantic preflight, copying and final complete comparisons are excluded from these totals. Copying/preflight times are separately recorded; no final-comparison duration is claimed.

Statement reuse alone did not bring the debug measurement under the deadline. The much larger observed reduction accompanies optimized code generation; these runs do not isolate a single compiler/library cause. The **2.54-second** profile predates explicit release overflow checks and must not stand in for the final **2.602231833-second** profile. The final measurement and release tests fulfill the repeat-measurement requirement preserved in [NATIVE-MAC-BUILD.md](../contracts/NATIVE-MAC-BUILD.md); the frozen contract was not edited by this audit.

The adjacent logs name release helper executables `omniusong_engine-cb90bfba3ff5aa6a` and `omniusong_engine-3401eefa33094f51`. Their current SHA-256 values respectively match the independent review's `5466ff09dcd0f415991dcbcdbab70826eafbe33a45cc8f96876bcc77477ae590` and `fbcdc4adaaf7e61db4179d5ad55696a029a0c6dc5f9e1556c4db4fe1d5b8d908`. These are manual test helpers, separate from the default-feature application. Profile receipts do not embed their build command/profile/source fingerprints; their adjacent logs, retained executables and source-review records must remain associated with them.

## Source gate and actual package settings

The [gate receipt](../../.runtime/rust-seal-fix/gate1-run.json) records successful format, offline locked all-features workspace tests, warnings-denied all-targets/all-features Clippy and offline locked all-targets build. All four recorded log SHA/lengths match current files and all exit codes are zero. All **150 source/configuration/contract fingerprints** still match. The gate script checks that its selected source snapshot did not change during execution.

Recounting every `test result` line gives **820 passed, zero failed, five ignored, 62 result groups** in both `gate1-workspace.log` (unoptimized test profile) and `release-workspace-tests.log` (optimized release profile). The latter includes the final overflow-check configuration and is distinct from the default-feature package. `hash-recording-tests.log` separately totals 113 passed, zero failed, one ignored across 14 groups. No tests were rerun during this audit.

The [release package receipt](../../.runtime/rust-seal-fix/release-package.json), package log and seven retained owned-target Cargo compiler-artifact records agree. Each original JSON record's SHA/length matches its receipt. All seven application-owned targets (`core`, `ipc`, `recording`, `session`, `store`, `engine`, app) report optimization **3**, overflow checks **true**, debug assertions **false**, debuginfo **0** and `profile.test=false`. None enables `test-hooks`; session lists only its `default` feature and the other six list no optional features. The unique selected target is the app's actual binary/source path, with executable `target/release/omniusong-app`.

The package invocation is `bash scripts/build-native-mac.sh` with release selected by default. The receipt records no inherited target/profile/Rust flag overrides among its enumerated build-environment fields. `Cargo.toml` retains thin LTO and one codegen unit with explicit release overflow checks; actual Cargo-reported settings are verified above rather than inferred from the manifest alone. These records establish the owned Rust target profile/features, not bytewise equivalence between all-features test binaries and the app or identical diagnostics in every dependency.

## Current executable, signing and image identity

Fresh physical reads match the package receipt:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Cargo `target/release/omniusong-app` | 20,603,456 | `cd33f88d7075c1a557fe7b1328f080ab2762aac46923befa18080186fd4aac8d` |
| Current `dist/OmniuSong Native.app/Contents/MacOS/OmniuSongNative` | 20,483,792 | `c46a0b7585d21eb94add1750e16900d8690f0b5c6525f217b6affeaaaf5588e2` |

Read-only `codesign --verify --strict --verbose=2` succeeds on both originals. The bundle is ad-hoc signed under its preview identifier; this is not distribution signing or installer qualification. Different whole-file digests after copying and re-signing are expected, but that expectation alone is insufficient image provenance.

Parsing the actual untouched Mach-O originals confirms that both signature blobs begin at byte **20,443,568**; Cargo's signature is 159,888 bytes and the bundle's is 40,224 bytes. Before that boundary exactly **eight bytes** differ, all within these three fields:

- `__LINKEDIT.vmsize`: offset 1,712, width 8;
- `__LINKEDIT.filesize`: offset 1,728, width 8;
- `LC_CODE_SIGNATURE.datasize`: offset 3,612, width 4.

Normalizing only those declared ranges in the complete pre-signature prefixes yields byte-for-byte equality and SHA-256 **`758f8c5fb0ff5b967a939ea7d192d60a9e1d7279b29f26bc728bcdd6ef25206d`**, length 20,443,568, exactly as recorded. The retained `cargo-macho-load.txt`/`bundle-macho-load.txt` diff confirms only those three load-command value differences, apart from the printed file label. This checks the entire image prefix, not only an instruction-section digest; no other pre-signature image-byte mutation was found.

**Unauthoritative intermediates:** `.runtime/rust-seal-fix/code-identity/cargo` and `code-identity/bundle` are earlier signature-removal copies retained from a failed naive comparison. The coordinator confirmed their provenance. Their headers have a removed signature load command and they are neither the untouched original prefixes nor the recorded selectively normalized result. Do not use their unequal SHA values (`bc0f01d5…` / `4260358c…`) as a package mismatch or as the image-identity proof. The successful comparison above independently read the actual original Cargo and signed bundle files; neither original was modified by this audit.

## Remaining native boundary and gaps

The retained `release-startup.sample.txt` names optimized app PID **3370** and Mach-O UUID `25BA6FFF-221F-3816-B13F-9A41209B76F2`, matching the current bundle load-command record. At 00:04:16 PDT its `omniusong-sessions` thread is sampled in `NativeStore::open_mode` → SQLite file open → macOS `__open`; the main thread is in the UI event loop. This is startup/file-open evidence, not recording or sealing evidence. The coordinator reports the process awaiting a macOS drive-access prompt. The stack alone does not prove the prompt's state, and this audit used no UI/device control to resolve or qualify it.

No fresh optimized native recording, Finish, explicit attachment, same-generation playback/status/Stop, Undo/Redo, archive/restore or restart success is present in the audited package/profile records. No timing extrapolation qualifies longer takes or thirty-minute reliability. Physical input, listening, MIDI/model/plugin, installer and Windows acceptance remain separate. The actual optimized workflow remains pending; no source cleanup, recovery action, skipped authentication or deadline change is implied by this evidence.

No raw-file/source/clone/package identity mismatch was found. The preserved profile requirement and the unauthoritative signature-removal intermediates are explicitly distinguished above. This review is frozen at the audited records and does not independently approve its author's hash implementation.

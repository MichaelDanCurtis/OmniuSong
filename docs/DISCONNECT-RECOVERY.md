# October 20 development-store recovery

The external drive disappeared during a native backup UI walkthrough. After reconnection, the development catalog reported `database disk image is malformed`. All 30 original prototype project/audio files and all 43 files in the preceding Rust source snapshot still matched their retained hashes. The physical cause of the catalog damage is not established.

The catalog advertised 45 pages of 4,096 bytes but contained 41. Independent raw-page review found all four current project documents, seven of eight history rows and two of five operation receipts. The absent pages held one history row and three later edit receipts for the copied recording session. The completed earlier OSARv2 backup and the complete 40,960-byte immutable recording object passed integrity and checksum checks.

## Preserved evidence and working recovery

- [Preservation and candidate qualification](verification/rust-native-disconnect-recovery.json) identifies the raw catalog, immutable object, source snapshots, complete backup, missing operation IDs and executed operator source/binary.
- [Independent findings](reviews/RUST-DISCONNECT-RECOVERY-INITIAL.md) and the ignored forensic directory's `independent-audit.md` distinguish complete survivors from missing evidence.
- [Reversible promotion](verification/rust-native-disconnect-promotion.json) records the verified working copy and the separately retained damaged directory. The original file is unchanged; no repair was applied to it.

The forensic directory is `.runtime/rust-migration/drive-reconnect-93681b4b-65d8-40d2-b017-98aa53b2b633/`. It contains an unchanged raw store copy, the SQLite CLI's salvage SQL, a separate salvage database, audit reports and the exact operator source/binary used. The damaged former default store remains at `.runtime/native-studio.disconnected-85df7f15-09a0-4598-931a-0a1d7a3319d6/`.

SQLite's recovery output is a salvage candidate requiring separate validation. Its documented limitations include absent or altered data and violated constraints. See the [SQLite recovery guidance](https://www.sqlite.org/recovery.html). A successful command or `integrity_check` alone does not establish complete project/history recovery.

The operator used existing Rust archive validators to export and restore the three complete sessions into a new store, comparing current documents, history, operation receipts, import bytes and source objects independently. Ordinary archive export correctly rejected the incomplete copied session.

That session's exact surviving revision-8 JSON was retained as a **new revision-0 session**, named **Recovered · Native archive restored copy**, with a fresh project identity. Its music, take references, expressions and unfamiliar metadata match the surviving document. The explicit changes are name, project ID, revision, import provenance and the manual recovery notice. The old three edit receipts and missing history row were not recreated or admitted under their old identities.

The case-specific `omniusongOperatorRecovery` notice carries the exact source JSON and its SHA-256, the known missing IDs/sequence, surviving history/operation/copy-proof evidence, and the complete verified earlier archive as bounded hexadecimal bytes with its length and digest. Existing data under that extension key is retained as `previousValue`. Because the evidence is embedded, ordinary metadata-preserving export/copy retains it without searching opaque fields for filenames or invented source references. The native interface identifies this as a recovered snapshot with incomplete earlier history.

At promotion, the verified working catalog had four projects, four complete retained history rows, one original publication receipt, one original import and one native take. The recovered snapshot starts its own history and has no live admission from the incomplete old copy. Both the raw damaged store and the validated candidate remain separately available.

## Bounded operator reproduction

`crates/omniusong-store/examples/recover_disconnected_preview.rs` is an operator tool pinned to this incident's exact input hashes, project identity and known semantic inventory. It is not an automatic or general-purpose production repair API. It requires a new output directory and receipt; it never overwrites the original catalog or silently fills missing history.

The current example also bounds input reads before allocation. The executed version that produced the actual candidate is preserved separately: its source SHA is `523d0f1caa5759f431588b96d2b1634cb294fe997a0a3f78eb4ad6da7306ad4e`. The embedded notice identifies that executed source. Later example hardening has its own source identity; it does not retroactively change the candidate's provenance.

## Reconnect behavior and remaining limits

The session worker reports typed fatal SQLite corruption/I/O/not-a-database failures and retires that storage generation after queuing its known failure response. The UI retains its draft and only offers safe explicit retry intents. Reconnecting starts a new generation and initialization; replaying a saved action is a separate explicit step.

`NativeStore::open_existing` uses read-write flags without `CREATE`, creates no missing directories, and rejects an uninitialized database. The service uses it for existing-storage reconnects, in addition to its ordinary-file checks. A disappearance between the path check and SQLite open therefore cannot create a replacement catalog. Deliberate first-run provisioning remains a separate path.

This scope does not qualify all removable-drive failures. Some late archive operations wrap underlying errors without preserving typed main-store versus input-archive origins; those cases are not classified by parsing error strings. Persistent volume/database identity validation, general recovery selection, independent failure-domain checkpoints, explicit platform durability policy, actual power-loss/drive-removal fault qualification and Windows behavior remain work. No physical recording, playback, latency or inference result follows from this recovery exercise.

## Actual recovered-studio verification

The rebuilt 191-test bundle completed native export, inspection, restore as copy, paired name/tempo Save/Undo/Redo, export and second-generation restoration. After a saved full process exit and relaunch, **Recovery archive second generation** reopened at revision 4 and tempo 122.5 with the same source take, range, placement, fades and incomplete-history disclosure. A malformed backup was rejected without changing that session, and subsequent valid inspection succeeded. The [actual UI receipt](verification/rust-native-disconnect-ui.json) records the binary, archive hashes, accessibility captures and screenshots. The [independent final UI/data audit](reviews/RUST-DISCONNECT-UI-FOLLOWUP.md) confirms the resulting rows, lineage, embedded evidence and source identities.

The working catalog now contains the initial four sessions plus two UI test copies. The original four project/history/receipt/import rows remain exact against the separately retained promotion candidate; all 30 original prototype files and the immutable take still match their recorded hashes. Both copy generations retain the exact opaque recovery notice, source JSON and embedded complete archive. This extends the portable snapshot evidence without recovering the missing old historical rows.

The observed light theme gives the gold recovery warning low contrast. After an ordinary inspection failure, the error is actionable and controls are enabled, but stale background-work status remains until another action. Both are recorded UI follow-ups; complete accessibility and failure-message qualification remain open.

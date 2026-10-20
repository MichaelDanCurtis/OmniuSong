# Independent disconnect UI follow-up

October 20, 2026. Read-only audit of the actual Mac native archive/edit/restart walkthrough after the reversible catalog promotion. This lane wrote only this review: no Cargo, production-source changes, UI operation, database mutation or dependency work. Python SQLite connections used `mode=ro&immutable=1`; the live database's complete-file SHA was stable before and after the audit.

The coordinator's `docs/verification/rust-native-disconnect-ui.json` matches the independent findings below. Its observed actions/process close/relaunch are corroborated by saved accessibility output/screenshots, the resulting exact database rows, and the two exported archives. This reviewer inspected that evidence and storage; it did not repeat the UI actions or rerun Rust validation.

## Disposition

The evidence supports this bounded native flow: export the recovered snapshot, inspect and restore as a new session, save name/tempo together, Undo/Redo, export that copy, restore a second generation, save again, reject a malformed external archive without replacing the current session, and reopen the saved second generation after relaunch.

No data-preservation or lineage regression was found. The two UI issues below remain. This is not complete recovery of the lost historical ledger, a general repair facility, physical durability qualification, or full workstation acceptance.

## Exact original preservation

The live catalog's four original project rows were compared column-for-column with `.runtime/rust-migration/recovered-native-preview-20261020/recovered-store`. All four payload strings, revisions and cursors match exactly. Their four history rows, original publication request/result receipt, import BLOB, asset table and take registration also match every column. No earlier project was silently edited by the new UI copies.

| Original project | Revision / cursor | Exact payload SHA-256 |
| --- | --- | --- |
| `2df3b9be-9c77-4560-9c45-7be5385d503d` | 5 / 1 | `48e1c791c6ad4534c4ffa7467a1471be0c82a2ba0c77cbc42b3d7b350ae7c466` |
| `aa3c40ed-277a-4b37-8bf0-101842d3de28` | 0 / 0 | `c08015a43f6832eaa297dbe8a3e9e5481955801b96d3bba2a72ce2f0f5de9769` |
| `b50ee3d5-ca2d-4d5f-b5af-c9a6ab9955ae` | 0 / 0 | `02a2ffe5bcc541ab5b87e7e10838edafd24c367e79b62949f890983d8bbe9fda` |
| `eb9b8562-4e5d-47c6-adae-e9194fcab668` | 5 / 3 | `79cc13a356b7a532d26b01eb0d6027a97882a5b68886294402e984020432a873` |

The 40,960-byte owned original remains byte-identical in live and initial-candidate stores, SHA `679914498bf02ced1570f8b58ba441eb1f958e8dec54d2931a4eab9f3657ff6b`. One registered take is shared across all these projects/copies; no alternate mutable recording was substituted.

The retained initial candidate database remains 499,712 bytes, SHA `d7ac97ef63f9f4a85678289f698d9009b83613db1cbc0fb669166b60861d20ae`. The damaged original at `.runtime/native-studio.disconnected-85df7f15-09a0-4598-931a-0a1d7a3319d6/studio.sqlite3` remains 167,936 bytes, SHA `3cec762a9455ed3b53525529e0ddf8e8d631e4a92931f4c445bb9cdc00b234a4`. Neither was overwritten by later UI work.

## New UI-created state and history

| New session | Project identity | Revision / cursor | Tempo | Exact payload SHA-256 |
| --- | --- | --- | ---: | --- |
| Recovery archive UI verified | `4ae2dba8-10de-46bb-92af-03895d24d83b` | 3 / 1 | 121.75 | `934187aec07b3341f3ddbf5c4a37be9fba2ccb44af290051ce132481661e5ea1` |
| Recovery archive second generation | `79407467-b1f2-41e8-a7d2-42326dd43204` | 4 / 2 | 122.5 | `75bf9ecb75fb30a631416e5b4b6d197563365df13eab41044084474c91ce101a` |

For both current payloads, exact UTF-8 comparison against the original recovered revision-0 payload permits only the expected root `id`, `name`, `revision` and `tempo` changes. Every other byte remains unchanged, including the opaque recovery notice, original import receipt, embedded source JSON/archive hex, recording metadata and source reference. Comparison of copied proof/request values also retained numeric literal distinctions instead of rounding unknown numbers through floating point.

The first copy has one retained history entry and three typed operation receipts: `UpdateSession` 0→1, Undo 1→2, Redo 2→3. The save/redo results contain both the new name and 121.75; Undo contains both original recovered values. Its history before/after is exactly the original recovered document and one paired edit, with the new root identity. All request/result SHA fields verify.

The second copy retains that history entry and three remapped receipts, then adds one paired name/122.5 edit at 3→4 and history sequence 2. The copied entry differs only in the root project identity. Its new entry's before document is the first copy's revision-3 current state with that root identity changed; its after document equals the stored revision-4 current state.

## Complete lineage and opaque recovery evidence

The first copy owns one proof; the second owns that exact same proof plus one new proof. The first active edge points from the recovered snapshot to the first copy; the second points from the first copy to the second. The second proof's sole ancestor is the first proof:

- First proof: `1a7522e9a97de8298be80b12cb527d141941c2c55fc15fb70069b2cf96ddf29d`.
- Second proof: `c39ea34ec268c6ccb58ac5abf47016a25237d101e154b35be94a7ae439f4fa61`.

Both canonical proof hashes and initial copied-result hashes verify. The first mapping is empty because its source had no operations; the second contains exactly three source→fresh-destination operation mappings. An independent recomputation of the domain-separated sorted mapping digest matches each proof. Every source/destination request/result digest matches its live operation row. The three copied request objects change only typed project/operation IDs; their result bytes change only the root project ID. Live copy admission IDs are separate from edit operation IDs, and inherited proof bytes match those in the first copy's exported archive.

The original incident notice remains exact in both copies. Its source JSON still hashes to `04aceba28b170eda514355b77e92d12a9cf19072e6a90e00377af50a4717fcc3`. Decoding its completed archive hex still yields exactly 163,840 bytes at SHA `00c6ab268772bddd03c198694799c02d8754c4909762f79c41c31b51903e627e`. Those historical IDs and evidence were preserved as opaque content, not recursively rewritten or admitted as live operations.

The old missing IDs `33436211-fdbc-4076-9c31-c841d7739c89`, `f6d04291-049e-4192-99d6-97e8edb18db8` and `2d94be93-13f8-4962-b581-8c06dfd2871f` remain absent from both live operation and copy-admission namespaces. The original loss of one history row and three receipts has not been hidden by these new, independently admitted operations.

## Catalog and archive snapshot

SQLite integrity and foreign-key checks pass for the live catalog, preserved candidate and both UI-created archives. At the stable read-only audit snapshot, `.runtime/native-studio/studio.sqlite3` is 6,430,720 bytes, SHA `d2266024588bc4f4949043d5d0a0fd17eef0beee627fff2f73ddb9649b7d9fbb`.

| Live table | Rows |
| --- | ---: |
| projects | 6 |
| edits | 7 |
| operations | 8 |
| imports | 1 |
| assets | 0 |
| native_takes | 1 |
| archive_copy_proofs | 2 |
| archive_copy_proof_operations | 3 |
| archive_project_lineage | 3 |
| archive_copies | 2 |

| UI-created OSAR-v2 archive | Bytes | SHA-256 | Retained history / operations / proofs |
| --- | ---: | --- | --- |
| `.runtime/rust-migration/disconnect-ui-recovered.omniusong-native` | 495,616 | `88900359e53ab28c0dc9485733b53ee688911bb340ab82bcbfce4100f8396424` | 0 / 0 / 0 |
| `.runtime/rust-migration/disconnect-ui-copy.omniusong-native` | 2,580,480 | `e5ec2e689ffdc11e8905d63024c5c578f59d48f56d25defb12f7595ebbcdb218` | 1 / 3 / 1 |

Each archive's current/history/operation/import rows match its live source, including stored checksums. Each includes the complete exact original take file with verified chunk SHA. Their file SHA/length pairs match the corresponding copy proofs' source archive bindings. The second archive's inherited proof is byte-identical to the live first proof. Archive/catalog sizes include retained full documents and proofs; this snapshot is not a large-library latency or storage-efficiency benchmark.

## Actual UI evidence and issues

The before-restart and reopened accessibility records identify the selected second-generation session at revision 4 and 122.5 BPM, with the explicit incomplete-history warning. Both retain source stream `synthetic-stereo`, audio frames [100,104), placement frame −48,000 at 48 kHz, one-frame fades and sealed manifest `03eff69da5c7bbcaa4027974703acb4917e9253d13f086c515d25c79624ec766`. Screenshots agree. The coordinator separately recorded UI close, process absence and relaunch of the same bundle; the reopened state is consistent with the independently audited persistent document.

The 36-byte malformed fixture is SHA `c15ed0bbd0c444513f7905c37e90d4dab3b9f296da9f2bc394575d5f754ea38d`. Its saved accessibility/screenshot evidence shows `file is not a database`, the same selected revision-4 session and enabled controls. The later before-restart evidence shows a successful valid inspection/fingerprint. No source document was replaced by inspection.

Two nonblocking UI findings remain, with concrete saved evidence:

1. **Recovery warning contrast:** the gold incomplete-history text is faint against the observed light background in `rust-native-disconnect-reopened.png`. Preserve the warning and use a light-theme-appropriate semantic warning style with sufficient contrast; verify both themes. Accessibility text exists, but visual readability is not fully accepted.
2. **Completed error retains busy status:** `rust-native-disconnect-malformed-ax.txt` and its screenshot show the inspection error together with the old “Work is running in the background” status, although Inspect/Refresh/Retry controls are enabled. Set a completed-failure/idle status for ordinary nonterminal errors. A regression should check both preserved session/draft and status text after malformed inspection, then successful recovery through valid inspection.

These findings do not indicate database mutation or continued busy admission. No fix was made in this read-only lane.

## Source and binary identity

All 19 prototype project files and 11 assets match their initial `rust-migration-baseline.json` SHA/length records (30/30). All 45 entries in the 191-test `rust-native-disconnect-tests.json` source snapshot match current SHA/length records. The observed bundle binary remains 76,838,288 bytes, SHA `90e5250d942abb5d3d5d071bf46b32652aafe475e2508dc5fa6052359a19c7da`. No source change or new test run is inferred from the UI work.

Key saved evidence hashes:

| Evidence | SHA-256 |
| --- | --- |
| `rust-native-disconnect-before-restart-ax.txt` | `46ddd0148fc1acfa389a4c3d638a2460fc1e6dbf2097c0bd70e70055bae48a3f` |
| `rust-native-disconnect-before-restart.png` | `10f931cc26d312d068d5fc5392482cb8816d32bf9714446c9875962c766e665f` |
| `rust-native-disconnect-reopened-ax.txt` | `8d7336f1cdd1d9200e9f39119ab694759f78d9623e2eff8d7745fee8deaad626` |
| `rust-native-disconnect-reopened.png` | `c2bf35a695b248b6fc40650fc0db37fa9cd9c961a30d7cfb7b03d6217cef360d` |
| `rust-native-disconnect-malformed-ax.txt` | `b981f3dd5bd1a56d02990a375e2b9f9b9ed7653bc344316c2f515c80ea1e4da5` |
| `rust-native-disconnect-malformed.png` | `449d1099b15190d6edaacb871dd9962970f5ed1bd2b05f332ccfbf3340d9ee53` |

## Limits

The original missing history and three receipts remain lost. The case-specific conservative snapshot and later typed copies do not establish automatic/general recovery, safe hot-unplug or power-loss behavior, persistent volume identity, stalled-I/O deadlines, or universal physical durability. The inspected recording contains four synthetic frames; no microphone, playback/listening, expressive MIDI transport/MPE/ClipFiles, plugin, model inference, choir, installer or Windows acceptance follows from this walkthrough. The broader native workstation goal remains incomplete.

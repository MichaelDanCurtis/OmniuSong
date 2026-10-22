# Durable evidence jobs and session integration

October 22, 2026. This is the bounded Unix Rust source/backend slice. The [verification packet](../verification/rust-native-evidence-jobs-tests.json) records root-run commands, exact source inputs and independent preservation. The installed 6ab792 app is unchanged; native evidence controls are still closed.

## Persisted requests and restart

The journal lives in a deterministic private sibling of the configured canonical studio root, with its own process and OS ownership. Its strict namespace header binds that exact origin. Request, started, terminal and observation records are versioned canonical JSON with separate request/body/history hashes. The raw history includes malformed bounded known slots. Eight observation slots and 128 occupied job directories are hard limits; holes consume capacity and nothing is evicted or repaired.

Immutable requests reserve exact destinations. Raw `./`, duplicate separators, parent traversal and trailing separators refuse. Existing IDs resolve before requiring the source store: exact repeats return historical metadata with `attempted=false`, including request-only or started jobs. Changed requests refuse. New execution requires the actual canonical NativeStore root. List/Read require only the retained namespace and its ordinary canonical parent; neither creates nor opens the origin. Reconciliation requires the exact request and full prior-history fingerprints, inspects only the recorded existing artifact and appends a bounded observation. It never resumes export/restore or redirects a request.

OSRE deliberately excludes this execution journal. Restored numbered evidence cannot become job authority. Inert extraction, song OSAR and manual operator rollback retain separate scopes.

## Write proofs and bounded finalization

Independent review found that accepting an in-memory record after a pathname substitution could acknowledge bytes not retained on disk. Created files now stay held through write and both sync boundaries; post-write stamps precede fault hooks and bind held and pathname identity. Append proof checks all prior slot stamps and exact old-plus-new membership before refreshing directory state or acknowledging the new history. Namespace creation similarly retains its initial directory identity. Fault tests replace same-byte inodes, rewrite in place and add surplus slots at sync boundaries.

Before artifact access, the same sticky governor reserves 608,128 work bytes for one owner-bound, non-Clone finalizer. The earlier 53,248 estimate undercounted byte and metadata passes. The corrected allowance covers sixteen maximum record-byte passes, sixty-four bounded path/stamp observations and one bounded member scan; it raises no governor ceiling. Finalization uses preloaded incremental hash state and cached view size, without reloading/serializing the whole history or performing artifact work after cancellation. A real successful export returned before late cancellation keeps its successful terminal.

A real operation result can exceed a 16-KiB durable envelope. Its separate 32-KiB correlated reply preserves the receipt with a metadata-persistence problem rather than holding service credit indefinitely. Unknown/partial completion remains explicit and artifacts are retained.

## Session behavior and evidence layers

The sole capacity-one SessionService has an independent evidence cancellation token. Exact cancellation keeps credit until a matching completion or actual disconnect. Request/payload mismatches cannot release credit. Run/Reconcile are excluded from generic retry; losing their replies retains uncertainty. Drop can cancel an inspection but preserves accepted execution/reconciliation. Raw native routes refuse all new backend commands until a guarded UI exists.

The typed primary operation outcome, any later budget stop and optional current live-catalog retirement remain separate. Only the actual current typed export can request retirement from original LiveCatalog/BackupTransfer SQLite codes. Historical results and archive/private SQLite failures cannot retire a healthy new worker. Completion is queued before a current fatal backend retires; terminal metadata failure cannot hide that fact.

Verification distinguishes three layers. Real isolated NativeBackend/SessionService tests execute OSRE export, inspection, reviewed inert restore, explicit reconciliation and source/archive-absent restart; a coherently rehashed corrupt archived catalog produces a local error followed by healthy session access. An origin/code unit matrix checks the actual retirement classifier. A held injected backend composes fatal retirement, cancellation and terminal-metadata failure to test protocol delivery; it is not a physical live-catalog failure during the same real export. Journal tests inject orderly write/sync failures; they are not power-loss evidence.

A separate Astra architecture reviewer and Sol integration reviewer found no remaining source blocker. Reviewers ran no Cargo, UI, devices or live-database connections. Root owns the actual test gate. The late-cancellation test directly checks actual export success, authenticated receipt, durable terminal/history and exact replay bytes after the cancellation token flips.

The final root gate passes **1,096 workspace tests / zero failures / eight intentional ignored helpers / 66 groups**, **98 optimized checks**, formatting, warnings-denied Clippy and the default build against **204 stable inputs**. A final independent audit verifies **635 hash references across 218 unique files**, all input fingerprints and all 32 new regressions in focused/workspace/optimized logs. Preservation is exact against both snapshots and the prior accepted schema-6 baseline: **22 projects / 403 rows / 80 protected files**, catalog bytes and installed 6ab792 binary. The audit opened no live database and found no remaining bounded-source blocker.

## Remaining delivery

Native Export/Inspect/confirmed inert Restore, retained job cards, explicit cancellation and restart/reconciliation controls require a separate integrated package and actual walkthrough. Metadata history must remain available when normal initialization fails or the source is absent, using the same service. Restore confirmation must bind path, reviewed SHA, UI context and worker generation. Preserve drafts during reads; require resolved drafts and released audio/capture before mutations.

Hard power loss, actual combined live-catalog/terminal failure, large full-studio capacity, scalable thirty-minute recording, source recovery/attachment/adoption controls and editable performance projection remain unfinished. MIDI 2/UMP/native transport/standard Clip Files, MPE, instruments/plugins, eight Rust inference roles, choir, listening/latency, installers and actual Windows acceptance remain mandatory. This packet does not complete milestone 2 or the full goal.

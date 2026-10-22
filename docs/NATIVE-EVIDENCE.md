# Save and review studio evidence

The native **Studio evidence** button opens full-studio evidence tools. They preserve the entire catalog and retained captures, source actions and takes, including unassigned or malformed retained material. This OSRE archive is separate from a per-song OSAR backup and the operator schema rollback.

## Export

Finish audio work and resolve unsaved musical edits. Enter a new absolute archive destination, choose **Review export**, check the origin and destination, then choose **Confirm export**. No existing destination is overwritten. The result card contains the exact job ID and archive SHA. Keep the archive and the separate job history if you need both retained bytes and local execution history.

The current bounded implementation permits a 512-MiB archive and 1-GiB cumulative inspection work. A larger studio can refuse within those limits. The completed native test uses a small synthetic studio; it does not establish full-studio scale.

## Inspect and restore

Choose **Inspect & restore**, enter the archive and a new absolute review folder, then **Inspect archive**. Review the summary before choosing **Review inert restore** and **Confirm inert restore**. Confirmation binds the reviewed archive SHA. A changed file refuses even when it is another valid archive.

Restoration produces `manifest.json`, numbered objects and, only after successful verification, `complete.json`. These are inert retained evidence. Nothing is added to a session or made into a live catalog, take owner or source action. Adoption requires separate future controls.

A failed or cancelled operation can leave partial or complete output. Keep it and its exact job identity. Cancellation is not rollback; starting a new job is not a retry.

## Job history and uncertain results

Choose **Job history**, **List jobs**, select the exact job, then **Read selected job**. These metadata reads preserve unsaved musical drafts. The original request and terminal record stay historical.

**Reconcile selected job** checks only that recorded existing output and appends one observation. It does not execute, retry, repair or adopt anything. After reconciliation, choose **Read selected job** again for the new history. Eight observations per job and 128 occupied jobs are retained without automatic eviction. A successful history receipt is not a new execution or a fresh artifact check.

For a missing or renamed original studio, the current development recovery entrypoint is an explicit launch with `--evidence-history --data-dir <original-exact-canonical-studio-path>`. It creates an unopened metadata connection without opening or recreating the catalog. Use the original path, not an alias or the parked folder's new location. Normal startup retains its previous catalog-creation policy and is not the missing-root recovery entrypoint. A graphical cold-recovery/enrollment workflow remains required for release.

The sibling `.omniusong-evidence-jobs-v1-<origin-hash>` namespace is outside the archive's declared catalog/captures/source-actions/takes scope. Keep it separately; restored historical job identities must never become execution authority.

The signed `7752c193…` Mac development bundle completed [the bounded native walkthrough](verification/rust-native-evidence-ui.json). Its [source gate](verification/rust-native-evidence-ui-tests.json) and [independent review](reviews/RUST-EVIDENCE-UI-REVIEW.md) retain separate source, injected-fault, package and actual UI evidence. Native input, full-studio capacity, physical power loss, release installers and Windows remain separate gates.

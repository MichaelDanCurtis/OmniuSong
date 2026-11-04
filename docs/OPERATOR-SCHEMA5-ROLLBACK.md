# Operator rollback before schema 6

Before replacing the schema-5 development app or opening the live catalog with schema-6 code, the coordinator closed both native processes and retained a complete local operator copy at:

`backups/native-schema5-before-migration-20261022T144111Z`

Its `READY.json` names a complete `manifest.json` with SHA-256 `1c071a6e0dccaa18e477f3fd1f0c0e8e0686be90874787d090ce135444b1e7e2`. The manifest inventories 86 files, 24 directories and 386,045,156 payload bytes: the whole native store, the read-only prototype `data` tree and the exact signed schema-5 app bundle. This includes original stages, sidecars and unassigned evidence. Payloads and file/directory modes were checked and synced before this copy was accepted. The backup folder is private, outside runtime cleanup, and ignored by Git; it must remain available through integrated migration/restart acceptance and until the operator retires it explicitly.

This is a manual same-Mac rollback copy. It is not the portable OSRE archive format, a model backup, or evidence that restoration or physical power-loss recovery has been exercised.

## Manual restoration boundary

1. Quit OmniuSong and verify both its main process and audio worker have exited. Preserve the complete current native root and app separately before moving anything; a rollback must not destroy newer work.
2. Verify `READY.json`, its manifest hash, every listed payload hash/length/mode, directory membership and the copied bundle's strict code signature. Inspect the manifest's source/destination mappings rather than inferring destinations from filenames.
3. Restore the whole native-store root and matching app bundle together from the stopped copy. Keep prototype sources exact as inventoried; do not merge SQLite files, individual rows or schema versions. Preserve current roots as a separate recovery set.
4. Recheck all restored hashes/modes and launch only the matching schema-5 app. Verify the catalog, all 22 projects, original files, source requests and saved session revisions. Retain restoration evidence before retiring either recovery set.

The old `7473e066…` app cannot open a schema-6 catalog. Merely launching a previous app bundle after migration is not rollback. New recordings and edits made after this copy require their own preservation; this pre-migration copy cannot contain them. No restoration has been performed as part of creating this copy.

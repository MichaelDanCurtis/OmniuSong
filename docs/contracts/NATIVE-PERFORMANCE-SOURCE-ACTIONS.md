# Durable performance source choices

October 22, 2026. Initial ledger contract. Extends the [recovery plan](../reviews/RUST-PERFORMANCE-RECOVERY-PLAN.md) and [bounded review](BOUNDED-PERFORMANCE-INSPECTION.md). A stored choice is never evidence that copying, recovery, native Finish, owner release or project publication happened.

## First delivery and authority

Deliver immutable exact review storage and Prepared copy-action metadata first. The executor, artifact writing, recovered finalization, adoption, attachment, disposition/supersession and evidence export remain closed until their separately reviewed paths exist. No UI recovery control or filesystem side effect follows from saving/listing these rows. Never remove the existing bounded-reader recovery refusal or reconstruct normal-seal authority.

The generic store owns byte/type/size/hash/history/CAS integrity. The session owns strict typed review/action validation and source/intent relationships. Neither layer may present a shaped request as freshly authenticated source evidence. A later executor must load the exact current durable receipt and review, then re-open/review-match the source before its first filesystem write.

## Schema 6 and bounded opaque storage

Coordinator owns migration. The schema-5 recording-intent rows, all project/history/asset/take/lineage rows and existing schema objects are unchanged. Create two new tables in the existing IMMEDIATE migration transaction, with immutable UPDATE/DELETE triggers; set user_version=6 only on success. New-schema name collisions roll back every migration statement. Future versions refuse unchanged. The installed schema-5 app/store stay unchanged during source-only development; tests use isolated synthetic stores. No live migration or new package is implied.

`performance_source_reviews`: one exact UTF-8 JSON-object BLOB per nonzero 32-lowercase-hex action ID, 1..=65,536 octets, ordinary raw SHA-256. Primary key `id`; unique `(id,payload_sha256)`.

`performance_source_actions`: `(id,revision)` primary key, revision integer 1..=16, exact UTF-8 JSON-object BLOB 1..=16,384 octets, ordinary payload SHA-256 and `review_blob_sha256`. Composite `(id,review_blob_sha256)` references the immutable review. Both tables preserve original serialized bytes including whitespace/escape/order/usage differences. Do not canonicalize review bytes into a new observation.

At most 32 identities, 16 revisions each and one review per identity: 8,388,608 action payload bytes + 2,097,152 unique review payload bytes = 10,485,760 bytes, excluding indexes/hashes/keys/SQLite/temporary allocation. No eviction/truncation. Reject malformed historical rows, gaps, orphans, mismatched references/types/hashes and over-capacity rows before listing or unrelated save. Bound SQL values with typeof/octet_length before allocating them. Return each review once, never once per revision.

Public store API:

```text
StoredPerformanceSourceReview { id, payload: Vec<u8>, payload_sha256 }
StoredPerformanceSourceAction { id, revision: u64, review_blob_sha256,
                               payload: Vec<u8>, payload_sha256 }
PerformanceSourceActionHistory { reviews: Vec<StoredPerformanceSourceReview>,
                                revisions: Vec<StoredPerformanceSourceAction> }
NativeStore::list_performance_source_actions(&self) -> Result<PerformanceSourceActionHistory>
NativeStore::save_performance_source_action(&mut self, id: &str,
    expected_revision: Option<u64>, review: &[u8], payload: &[u8])
    -> Result<StoredPerformanceSourceAction>
NativeStore::save_performance_source_action_if_history_matches(&mut self, id: &str,
    expected_revision: Option<u64>, review: &[u8], payload: &[u8],
    expected_history: &PerformanceSourceActionHistory) -> Result<StoredPerformanceSourceAction>
```

The first review and action append commit together. Validate all retained rows, then require exact immutable review bytes. Exact CURRENT action-byte replay precedes CAS and capacity, including stale/None expectations at the limit. An older action payload is not current replay. Append requires exact latest CAS; changing review bytes/refuses, rather than creating another review or silently sharing it with another identity. Re-read/authenticate the entire bounded postimage and compare expected rows before commit, including protection against trigger-created surplus rows. Failed CAS/capacity/inserts leave no orphan review. Stored hashes establish consistency, not immunity to a malicious coherent rewrite of the entire catalog.

The typed session exclusively uses the history-matching variant. After acquiring IMMEDIATE and reading/authenticating the current ledger, it requires exact equality with the whole history previously validated by the session, before replay or writes. This binds cross-action uniqueness and all typed checks to the transaction snapshot. An unrelated concurrent change returns a known conflict without inserts; the next explicit retry must load/validate fresh history. The unfenced API remains opaque storage only. Exact payload replay does not bypass this additional typed validation fence. No automatic unbounded retry or silent replacement choice is permitted.

## Typed Prepared choices and stable source admission

The exact review blob hash differs from the DTO's existing domain-separated `review_sha256`. Recompute the DTO hash using its original algorithm (typed serialization with that one field blank). Preserve the full original blob including diagnostic usage. A version-1 source-admission projection separately binds every source/review semantic field except `usage` and `review_sha256`; include stage/path/take, profile/config, state, physical length/hash, descriptor/all-original identity, manifest facts, generic problems, full semantic checkpoint/selection/fault/cut. It has its own domain `OmniuSong.performance-source-admission.v1\0`. Merely supplying a matching digest grants no authority; the backend recomputes it, and the future executor recomputes actual source facts.

Typed Prepared payloads reference the review blob and admission digest rather than embedding a 64-KiB review or duplicate checkpoint/config. Bind version, fresh action ID, fixed copy-recovery kind, exact stage/take, optional exact historical recording-intent ID/revision/payload hash, recovery operation ID and resource/artifact-policy version. Artifact paths are derived from root + action ID under `source-actions/`, outside capture discovery; no arbitrary destination supplied by a UI. Initial typed admission permits Prepared creation/exact-current replay only, no successful result fields or future phase invention. Generic store revision-capacity tests are not proof of legal Prepared state transitions.

Frozen typed session surface:

```text
PerformanceSourceActionKind::RecoverCopy
PerformanceSourceIntentReference { id: LiveId, revision: u64, payload_sha256: String }
PreparedPerformanceSourceAction { version: u32, id: String, kind,
    stage_id: String, take_id: String, review_blob_sha256: String,
    admission_sha256: String, recovery_operation_id: OperationId,
    policy_version: u32, origin: Option<PerformanceSourceIntentReference> }
PreparedPerformanceSourceAction::prepare(id, review_bytes, recovery_operation_id, origin)
PreparedPerformanceSourceAction::validate(&self)
PreparedPerformanceSourceActionReceipt { revision, action, payload_sha256 }
Command::SavePreparedPerformanceSourceAction { expected_revision, action: Box<...>, review: Vec<u8> }
Command::ListPreparedPerformanceSourceActions
Reply::PreparedPerformanceSourceAction(Box<...Receipt>)
Reply::PreparedPerformanceSourceActions(Vec<...Receipt>)
```

All payloads deny unknown fields and unsupported versions/kinds/policies. Prepare recomputes both review identities and requires an unsealed review with a positive selected checkpoint; this is request-shape validation, never current source authentication. Saving binds the review's absolute original path to the opened root, and an optional historical intent reference to its exact retained bytes/stage/take/GUI clock/descriptor where present. `origin=None` means unlinked, not a claim that no matching request exists. Validate all retained typed histories before save/replay/list, including compact payload references to exact stored reviews and unique recovery-operation IDs. Typed actions use versioned canonical serialization so an unsupported rehashed/noncanonical action cannot turn an immutable Prepared replay into an extra revision; reviews retain their exact raw bytes. Initial legal typed history has revision 1 only; changed Prepared fields and invented later phases reject, even if generic opaque storage can retain up to16 revisions for future contracts. Both commands use the sole session service; no source opens or engine commands. Replies omit full review blobs; list at most32 compact receipts, bounded to576KiB including envelope overhead. Review blobs remain separately retained in storage. No application UI action/continuation is wired by this delivery.

The supported review shape keeps `unknown_tail=true`, positive physical length within the fixed512MiB ceiling and every diagnostic usage counter within the existing policy ceilings. Reuse the original checkpoint/Finish codec validators and bind checkpoint/optional Finish worker/session/Panic scope to the initial config; do not hand-reimplement a weaker record codec. Request-shape/root/hash/origin refusals use known pre-write metadata rejection (existing `IntentRejected`), including malformed review-stage IDs; the read-only source reader retains its different `SourceUnavailable` contract. Genuine catalog errors and possible writes keep conservative uncertainty/fatal-storage handling. No error-message parsing decides persistence.

Future copy/recovery must keep source/evidence immutable, a distinct working artifact, exact source/operation/checkpoint binding, one cumulative governor and continuous owners. Every filesystem read/write/hash pass consumes that governor; file-size high-water is not aggregate artifact capacity. Sidecars, identity/path replacement, unknown destructive trigger behavior and limits refuse without cleanup. Any possible mutation must reconcile the same working artifact after uncertainty, never create a fresh manifest by starting another copy. These are required later gates, not delivered by this ledger.

## Review and verification

Independent storage tests use real SQLite BLOB/type/hash oracles, byte-distinct semantically equal JSON, exact 64-KiB sentinels, capacity/replay, two-writer CAS races, corrupt/oversized history, immutable triggers, failed insertion/trigger postimages, populated5→6 migration, rollback on name collision and future-version refusal. Preserve original schema-5 rows and exact review bytes through reopen. Keep maximum raw-store fixtures distinct from typed Prepared semantics. The coordinator runs Cargo and audits originals; no devices, native input, downloads, Fuego edits or main-store migration are allowed for this source packet.

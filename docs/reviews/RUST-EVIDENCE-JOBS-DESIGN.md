# Durable evidence jobs: implementation design

October 22, 2026. Astra architecture recommendation accepted for the next bounded integration. This document specifies work to implement; it does not claim a journal, session command or native control is delivered.

## Module boundaries and fixed limits

Use session-owned `evidence_jobs.rs` for strict public DTOs, validation, typed OSRE execution and reconciliation; private `evidence_jobs/journal.rs` owns bounded records and metadata locks. Session `lib.rs` owns commands, exact cancellation and nested worker retirement. Later native `evidence_ui.rs` owns presentation only. Add no catalog migration or general job framework.

| Bound | Compiled maximum |
| --- | ---: |
| Jobs, including damaged/incomplete slots | 128 |
| Namespace header | 4 KiB |
| Request envelope | 16 KiB |
| Started record | 1 KiB |
| Terminal record | 16 KiB |
| Observations per job | 8 |
| Each observation | 16 KiB |
| Each exact path | 4096 UTF-8 bytes |
| Explanatory message | 2048 UTF-8 bytes |
| Compact list reply | 64 KiB |
| Selected-job reply | 256 KiB |

The header plus maximum record payload is 21,106,688 bytes, excluding filesystem overhead. Occupied partial slots count. Refuse excess; never evict unresolved jobs, silently truncate identity or collect files automatically. Only explanatory messages may be shortened, with an explicit truncation flag. Bounded serializers precede writes.

## Exact identity and separate metadata

A request contains version 1, nonzero32hex job ID, exact canonical origin string and one strict operation: full-root Export with scope/export ID/destination, or Restore with archive path/directory/reviewed whole-container SHA. UI epochs, transport request IDs and timestamps are outside durable identity. New execution requires origin equality with the actual NativeStore root.

Fingerprint the canonical strict request with SHA256 over `OmniuSong.EvidenceJobRequest.v1\0` plus its exact JSON bytes. Preserve exact lexical path bytes. Every subsequent record binds that request SHA, record kind, sequence and canonical body hash. Observations also bind the previous history SHA. Fingerprint ordered filenames, lengths and exact bytes, including bounded malformed occupied slots; an invalid file must not look absent.

Derive a deterministic sibling namespace from the exact previously established canonical origin:

```
<origin-parent>/.omniusong-evidence-jobs-v1-<SHA256(origin UTF-8)>/
  namespace.json
  .owner.lock
  <job-id>/
    request.json
    started.json
    terminal.json
    observation-01.json ... observation-08.json
```

The header pins version and exact origin. This remains available when the live root is missing/renamed. Replay must not reopen or infer a new origin identity. Use a nonblocking process-local guard keyed by namespace plus a single-link ordinary OS owner lock through each complete command. Do not hold a retained-file store writer lease across OSRE export. Validate canonical ordinary/private ancestry, exact membership and anchors around writes and before artifact work.

Only an explicitly admitted first mutation may initialize the namespace; startup/listing creates nothing. Retain incomplete initialization and refuse malformed namespaces. Reject archive inputs and mutating destinations that overlap the journal namespace. This separate application history is outside OSRE's catalog/captures/source-actions/takes closure and cannot be restored as execution authority.

## Write ordering and uncertainty

Under the journal owner, validate bounds, namespace, exact request, destination exclusions and reservations. An existing job ID never executes a mutating API again, even with an identical request and no started marker. Different request bytes refuse; identical bytes return the existing state. Refuse new admission if unreadable prior requests prevent reliable destination reservations.

For a new job: exclusively create its private directory and sync the namespace; write request with create_new, sync file and directory; reserve finalization credit; exclusively write/sync started; recheck anchors, request fingerprint and cancellation; invoke exactly one typed export or reviewed restore; exclusively write/sync terminal; return correlated completion. Direct write-once partial files remain evidence. Do not overwrite or rename-repair them.

Before starting artifact work, reserve 53,248 work bytes (3 × 16 KiB + 4 KiB) for bounded finalization. Sticky cancellation otherwise prevents persisting its own outcome. After cancellation, only the private prepaid terminal serializer/write/sync may consume that allowance; it grants no artifact work or higher ceiling. Reconciliation similarly prepays one observation. Keep optional live-catalog retirement independent of terminal/observation persistence failure.

Startup loads metadata only. A valid request without a conclusive result remains unresolved; started without terminal is interrupted/uncertain. A successful terminal is a historical receipt, without a new artifact-existence claim. A failed/cancelled terminal may coexist with complete artifacts. Invalid later records retain uncertainty but do not prevent explicit read-only reconciliation from a valid exact request. Invalid/missing request bytes authorize no artifact operation. Partial observations consume slots; later observations bind the full prior history. At capacity, refuse further journalled observation without deleting history.

## Commands and native integration

Expose RunEvidenceJob, InspectRetainedEvidence, bounded ListEvidenceJobs, ReadEvidenceJob and ReconcileEvidenceJob with expected request/history fingerprints. Reconciliation accepts no replacement paths or SHA and calls only the existing-artifact verification APIs. Retain original terminal records and append a bounded current observation.

Keep Run outside generic retry safety. A lost read-only reconciliation reply cannot erase the earlier mutation's uncertainty. Add a separate exact-request-ID evidence cancellation token, holding service credit until correlated completion or actual disconnect. Drop must not silently cancel accepted mutations.

Reply with exact job/request/history identity, typed primary outcome and uncertainty, optional journal persistence problem and optional catalog_retirement. Extend existing nested terminal-failure detection; queue the correlated reply before retiring a fatal live-related generation. Source-independent list/reconciliation must not accidentally call lazy store creation or require the missing original archive/root. New execution still requires the real origin store.

Connect native controls after these bounded commands pass: reads preserve drafts; mutations require resolved drafts and released audio/capture. Restore confirmation binds reviewed path/SHA/context/worker epoch. A new destination is a separately confirmed new operation, never retry of an uncertain job. No paint, selection or reconnect performs artifact work.

## Required verification

Test every request/started/artifact/terminal/observation write/sync boundary; persisted request before artifact mutation; existing identities never reexecuting; request/history/destination fences; partial metadata and no startup execution; cancellation-safe bounded finalization; catalog retirement surviving both cancellation and journal I/O failure; two-process nonblocking ownership without self-conflicting broad leases; aliases/substitution/unknown membership/self-destination refusal; capacity without eviction; lost replies and original-absent exact reconciliation. Then qualify explicit Export→Inspect→confirmed inert Restore/cancel/restart reconciliation in the installed native app and independently compare catalog/originals. Native input is neither needed nor permitted for that walkthrough.

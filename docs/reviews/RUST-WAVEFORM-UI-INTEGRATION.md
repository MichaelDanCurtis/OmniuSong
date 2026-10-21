# Native waveform UI integration review

Source-only review, 2026-10-20. This review covers the coordinator's integration
in [main.rs](../../crates/omniusong-app/src/main.rs) and the
[source waveform contract](../contracts/NATIVE-SOURCE-WAVEFORM.md). The reviewer
authored `waveform_ui.rs`; this is **not an independent review of that module**,
its projection mathematics, painting, tests or source authentication. The engine
client boundary was inspected only to check the integration's ownership,
nonblocking entry points and error wording. Backend validation needs its own
review and gates.

No Cargo, formatter, native UI, device, catalog or source-data operations were
performed for this review. Existing test names/source were not treated as proof
that tests ran. Actual accessibility, layout, cancellation timing and packaged
application acceptance remain separate.

## Resolved finding reported to the coordinator

**Contract wording: distinguish a failed preparation from a retired client.**
The contract's initial statement, “A failure retires this read-only client,” is
broader than `WaveformClient::try_poll`. A matched ordinary source, format,
resource-limit or cancellation error is returned in `WaveformCompletion.result`;
the admission credit is released and the persistent client permits an explicit
Retry. Disconnect, correlation failure and invalid successful completion are
retirement cases. The contract should name those terminal client failures and
state that ordinary preparation failures remain explicitly retryable. Neither
case retires the independent durable session service or audio process.

This documentation mismatch was sent before creating this review. The
coordinator corrected the paragraph during review; its revised text was read
back and now explicitly distinguishes retired clients from ordinary retryable
preparation errors. No new blocking main-loop integration defect was found in
the inspected source. Central source/runtime gates are still required; this is
not a release approval.

## Main-loop and ownership checks

| Concern | Source observation and implication |
| --- | --- |
| Loading and busy polling | `StudioApp::show` polls storage, audio and then waveform before its `!ready` early return. Pending waveform work requests a repaint after 50 ms independently of session busy state. The loading branch cannot strand a completion or admission credit solely by returning early. |
| Edit controls versus Hide | The central editor uses `ui.add_enabled_ui(available, ...)`. The waveform call is a sibling after that child scope, within the common outer scroll area. Session fields/history/clip controls remain disabled while storage, audio or recovery guards block them; the waveform panel is not disabled by that child scope. Its `WaveformAccess.allow_start` receives `available`, while Hide/Cancel remain module-local actions. |
| No implicit source reads | Construction adds only `WaveformUi::default()`. Main calls only `poll`, `pending` and `ui`; it never calls preparation or storage APIs for the waveform. `WaveformClient::spawn` captures the root in a worker closure and starts a thread; the source reader executes on that thread. Main does not create a worker merely because a document or clip is selected. |
| Start admission | `available` excludes session busy/disconnected, active audio and retained recovery publication guards. Explicit Show/Retry therefore cannot begin through this call while those guards apply. Dirty or retained musical fields do not themselves cause an implicit Show. Explicit read-only viewing can coexist with such local fields; the clip unresolved flag instructs the panel to avoid presenting uncertain draft settings as valid. Actual audio admission still uses the stricter, separate `can_start_audio` predicate. |
| Saved identity changes | Storage completion runs before waveform polling. A newly installed revision/project and the current exact `ClipSelection` are consequently passed to waveform synchronization in the same frame. Track changes use existing routing that clears exact musical selections. Main never binds a waveform to a track index, copied name or selected row position. Cancellation/rejection of stale publication remains the waveform module/client's responsibility, outside this independent review. |
| Studio root lifetime | Main passes the owned `self.paths.native` by reference. Paths are chosen at startup/CLI parsing; there is no runtime root assignment or root-change UI. The waveform client is independent from session reconnect and is not replaced by reconnect/navigation code. This supports one pinned root per app instance; arbitrary runtime root switching is not qualified. |
| Mutation isolation | There is no waveform variant in `Action` or the session `Command` enum, and no waveform result branch in `StudioApp::complete`. The waveform call returns no session action. It cannot enter Save/Discard/Cancel continuations, clear generic retry evidence, install `DocumentView`, advance project revisions or rewrite native/legacy source metadata through main. |
| Borrowing and paint work | The added calls pass `document.as_ref()`, `clips.selection.as_ref()` and `clips.draft.as_ref()`. Main does not clone, parse or serialize a whole project/payload to render or request the overview. Mutable waveform state is a distinct owned field; the supplied document, selection and draft references are shared borrows. No unsafe aliasing or additional project ownership transfer is introduced by the integration. |
| Close | Waveform pending is absent from session/audio/recovery close guards and `busy()`. Existing unsent-draft and retained-intent Close decisions remain authoritative. Window destruction drops the independent client; the inspected client Drop revokes publication and disconnects without joining a held reader. Process exit can end unfinished work; no eventual-completion guarantee is made. |

The waveform panel is below the music inspector and model disclosure in the
outer vertical scroll area. Source placement does not prove that Show/Hide,
legends and every channel remain easy to find at a small native window size;
that needs the actual packaged walkthrough. Headless AX presence alone would
not establish pointer focus, screen-reader usability or colour contrast.

## Contract boundary

The contract correctly separates authenticated raw-source bins from saved,
permanent-original and local projection overlays. It explicitly preserves exact
native origins, legacy Number text, missing samples and unknown tails; describes
the declared/decoded-duration mismatch without hiding source bytes; and keeps
Save authoritative. It also states source/channel/sample/object/summary limits,
trusted-parent filesystem scope, noninterruptible sealed scans and the absence
of a wall-clock I/O deadline. These are appropriately narrow requirements, not
proof that the backend or this reviewer's UI module satisfies them.

The existing audio eight-channel limit, physical recording accuracy, listening,
realtime coexistence, waveform editing handles and full DAW/release acceptance
are not inferred from this integration. Waveform errors remain display
diagnostics and must not be reclassified as session SQLite quarantine errors.

## Inspected source identity

The following SHA-256 values identify the inspected pre-central-format source
snapshot; a later build or format result requires its own receipt.

| Source | SHA-256 |
| --- | --- |
| `crates/omniusong-app/src/main.rs` | `cd262bfd0679b52d8374dfec4d7bff1b26e50507733b757118aeff46204fa402` |
| `docs/contracts/NATIVE-SOURCE-WAVEFORM.md` | `77d1c933ebe150df0e5433b48e8ff0a5e8940ab1829e3cfb9e90817e33c57d66` |
| `crates/omniusong-engine/src/waveform.rs` client boundary | `98b82dfd735a592789b7681ada5a9a19e601eca7b4532f09407c5123fadfb011` |

The corrected contract's follow-up SHA-256 is
`50e0491c6849396932eb4ecbebef79faf2b17a6caf86068d7b24b1c7bfa0122a`.
Main and the inspected client boundary remained byte-identical at that readback.

# Native original-source waveform contract

This is a read-only overview for the native clip inspector, implemented in Rust.
Qualification and remaining release work are recorded separately in STATUS and
the release acceptance matrix. A waveform does not qualify recording, audible
quality, realtime coexistence or any model inference.

## Identity and ownership

An explicit Show request binds the saved project ID and revision, exact legacy
audio or native audio-frame clip address, playable asset/take/stream identity,
sample rate for native audio, and bin resolution. Legacy `sourceAssetId` is
ancestry, not permission to decode a different asset. Raw event clips have no
audio waveform. Loading a waveform never changes documents, revisions, history,
retry identities, original snapshots, drafts or continuation choices.

The independent `WaveformClient` has one admitted request until its real terminal
completion is consumed. Hide, Cancel, revision/selection changes and Drop revoke
publication immediately. They do not imply that an OS read or the sealed journal
initial scan stopped. No replacement worker starts after a timeout. Drop never
joins a blocked worker. A disconnect, correlation fault or malformed completion
retires this read-only client without retiring the durable session service or
native audio process. Ordinary source, limit and cancellation errors return
admission credit on their real terminal completion and permit explicit Retry.

The UI has no source file I/O. It continues polling while session work is busy;
Hide remains accessible outside the disabled session edit controls. New Show and
Retry requests require an available studio and a free waveform admission credit.
Any delayed result must match ticket, request and current selection before it
can display. Navigation never automatically starts a replacement read.

## Read-only resolution

`NativeReader` opens an existing current-schema SQLite catalog with READ_ONLY,
query_only and a bounded cache. It refuses WAL header mode before SQLite open to
avoid creating SHM files. It never creates a catalog or migrates one. Root and
catalog must be ordinary nonsymlink entries; hardlinked catalogs are refused on
Unix. This is integrity checking inside the trusted local studio namespace, not
a security boundary against a hostile user concurrently replacing ancestors.

One short deferred snapshot resolves document/revision, selected source metadata
and, for a legacy asset, bounded checksummed bytes. It rolls back before native
take authentication/decoding. Other source files are not opened merely to load a
document. Native authentication checks exact registered reference, ordinary owned
file, registered and physical byte ceilings, sealed manifest, physical SHA-256,
and file identity. The existing sealed journal scan is noninterruptible; physical
hashing checks cancellation every 64 KiB. Error strings from this independent
reader are display diagnostics, never session-worker retirement classifications.

## Source, bins and overlays

Peaks describe the complete authenticated original, without applying track gain,
trims, fades, mixing, synthesis, normalization, resampling or channel folding.
Each channel has its own true finite min/max; extrema start at the first real
sample rather than zero. The graph states its amplitude scale. WAV uses source
frames `[0, actual decoded frame count)`. Native audio retains the exact absolute
descriptor origin and proven sealed-prefix end. Arrangement ticks/signed frames
are reported separately. Integer source-origin subtraction precedes drawing
conversion, so a large native origin does not erase a short visible interval.

Integer u128 floor division defines bin boundaries. Each bin accounts for every
content and declared-gap frame. Missing samples have no extrema; they are not
zero-valued silence. Mixed bins retain content peaks and visibly mark missing
coverage. Recovered unknown tails are identified beyond the proven prefix without
assigning them an invented duration.

Saved, permanent-original and valid local-draft windows/envelopes have separate
labels and line styles. Drafts are projected from small cached settings rather
than cloning/updating a complete document in paint. Invalid/unresolved drafts
fall back with an explicit explanation. A legacy declared/decoded-duration
mismatch still permits viewing the actual source; an unavailable overlay gets
its own explanation. Stored exact JSON Number text is never rewritten by a
floating-point projection. Removing legacy trim-end defaults to the declared
duration, not measured WAV length.

Native fades require a checked sum no greater than the source-window count.
Legacy windows/fades follow the core seconds constraints and existing 1e-9
tolerance, then renderer `.round()` projection; independent rounding can produce
a one-frame overlap. The production envelope multiplies applicable linear
factors and then applies `sin(product * PI/2)` for equal-power curves. Preview
gain outlines do not audition or save audio. Save retains authoritative core,
timeline, revision and source verification.

## Initial limits

| Resource | Bound |
| --- | --- |
| Legacy WAV bytes | Existing 30 MiB checked asset ceiling |
| Native complete object | 64 MiB registered and physical file ceiling before sealed scan |
| Complete native manifest | 65,536 chunks, including other streams |
| Selected content decoding | 32 × 1024 × 1024 channel samples |
| Channels | WAV 1–32; native PCM 1–16, every channel inspectable |
| Bins | 1–1024 per channel; no more bins than source frames |
| Typed summary | 2 MiB including allocated bin/Vec capacity and identity overhead |
| Display/cache | One displayed summary; bounded virtualized channel rows |
| Cancellation | Between source operations/chunks, each 64 KiB physical hash read and at most 4096 decoded channel samples |

There is no arbitrary duration cap. Known gaps use interval arithmetic and do not
spend decoded-sample work. These limits exclude many long real recordings and do
not establish a wall-clock I/O deadline. Oversized or unsupported sources remain
saved and editable; this overview reports its limit instead of truncating a
source or labeling a partial result complete. Playback's existing public
eight-channel ceiling remains independently enforced. No native input stream is
opened for this work; the user's software-loopback-only restriction remains.

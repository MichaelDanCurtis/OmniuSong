# Recording and original audio

The preview keeps the human source separate from estimated notes. Recording and import send audio only to the studio's loopback service. No cloud audio service is enabled.

The metronome/count-in applies to session Play, armed note recording, and the optional **Sing with song** mode. Recording placement uses an explicitly uncalibrated browser estimate. See [transport boundaries](TRANSPORT.md).

## Record a take

For a solo take, choose **Solo voice / imports at bar** (1–128). For a take with backing, enable **Sing with song** instead; that mode starts from bar one. These new-take controls are tab settings, not saved session preferences. Existing clip placement is saved with each take.

Choose **Record voice** to request the microphone. Permission is requested only on that action. While the request is pending, **Cancel mic request** cancels the application's request; if the browser supplies a stream afterward, its tracks are stopped. This does not change the browser's saved permission setting. A late result from a canceled request cannot reset a newer take.

Song mode resumes the audio context from the recording action, requests permission, then prepares a snapshot of the backing arrangement. **Cancel song preparation** cancels a pending load and releases the microphone. Capture starts after loading and before the scheduled count-in/song boundary. A failed preparation or encoder start stops its backing playback and releases its stream.

**Finish take** stops capture, waits for final encoded audio, releases the microphone, stores the original asset, attaches it to the current session, and saves that session. Session switching and restoration stay blocked through the permission, recording, finalization, and save stages. The preview stops recording after two minutes or when its encoded-byte limit is reached. Timer timing and media encoding are browser controlled; this is not sample-accurate recording alignment.

If storage fails, **Retry saving take** retries the same original and track identity. **Download unsaved original** gives you the original bytes independently. A completed original now also enters a local recovery journal before session attachment. The status distinguishes an original retained only in this tab from one saved in local recovery. Closing while a take or session edit is pending still triggers the browser's leave-page prompt. Original recovery does not silently discard a microphone take.

A device interruption saves the bytes the recorder produces and records an interruption message. An interrupted container may still need repair or may be unplayable; preserved bytes are not a guarantee of a complete performance.

## Recover a completed original after reload

After a take finishes, or an import passes duration/decoding checks, the browser hashes the complete original bytes. It writes a bounded proposed audio track to the local service, uploads the original unchanged under that content checksum, then attaches the track and saves the session with its expected revision. The service only completes recovery after it verifies the saved audio track, source identity/core metadata, and original checksum/length. A completed journal receipt is retained locally; audio assets are not deleted.

**Original saved in local recovery** means both the recovery metadata and completed audio reached the service. After a browser reload or service restart, **Unfinished audio saves** shows these originals with **Open session and recover** and **Download retained original**. Recovery opens the original session, reads its latest revision, and uses the same track identity. It preserves placement, timing estimates, interruption notices, original filename, and origin. It requests no microphone and does not encode again.

If the session saved before its recovery acknowledgment, the panel offers **Finish recovery**. It verifies the existing original and completes the journal without adding a track or replacing later clip/name/mute edits. Conflicting track identities, missing sessions, damaged audio, and unreadable metadata remain visible for review; they are not overwritten or silently removed. Missing-session or identity-conflict entries still offer the verified original download. If an upload did not finish, the metadata-only entry says so and offers no false recovery/download action.

The session's complete backup is blocked while it has active unattached originals. An active corrupt recovery metadata file also blocks backup until reviewed or explicitly set aside. Already-attached originals can be backed up before acknowledgment because their full source is in the project. Operational pending/completed journals are outside the project archive; once recovered, the source track, timing, and referenced audio are included normally.

The local journal lives in `data/pending-takes/`; verified completion moves its receipt to `data/completed-takes/`. Originals live in `data/assets/`. Active pending work is limited to 16 entries, 128 MiB of declared audio sizes, and 64 KiB per metadata request. Repeated references count toward this conservative pending budget. **Set aside · keep locally** moves an entry into `data/parked-takes/` without changing its bytes or audio assets. It stops blocking project backups and frees active recovery capacity. Set-aside entries stay visible with **Return to recovery**; returning respects the active count/size limits. Up to 64 set-aside entries and 512 MiB of declared source sizes are allowed. Unreadable metadata reserves 30 MiB conservatively. No original or journal file is deleted by these actions.

Setting aside moves only a recovery entry. It does not remove or rewrite an accepted session track; complete backups always include the audio referenced by that project. A source already attached to its session should use **Finish recovery**, and a stale set-aside request for it is rejected. Retrying a set-aside take cannot silently reactivate it; return it first. Set-aside originals and their operational journals are not added to project archives merely because they are kept on this Mac.

Missing-audio and unreadable-metadata entries can be set aside and returned without inventing a playable recording or a download link. Their original files/metadata remain local for review. No automatic deletion, quota purge, or interrupted-container repair is implemented. Decisions and local files survive service restart and browser reload.

This protects completed originals after both writes commit. **An active microphone capture, encoding not finalized by the browser, or an original that never reached the service can still be lost if the tab/process closes.** Before the durable boundary, use the retained-tab retry/download controls. Incremental recording-fragment journaling, power-loss/storage-device recovery, and repair of interrupted media containers are not qualified. Local disk failure is not covered by this recovery copy; use complete backups separately.

## Review timing

Song-mode takes save `audio.recordingTiming`: version 1, method `browser-recorder-start-call`, calibration `unmeasured`, the Web Audio time sampled immediately before the recorder start command, the scheduled song start, and the playback tempo, PPQ, and original loop length. This is command timing, not a timestamp for the first captured sample. Browser scheduling, input/output device latency, clock drift, codec padding, and encoder delay are not measured or compensated.

If capture begins before the song, the arrangement places the clip at tick zero and sets Source in to the estimated preroll duration. If capture starts late, its start tick uses elapsed song time without wrapping at the loop boundary. The original file always retains all encoded preroll. A take ending before the song, or whose estimated placement exceeds 128 bars, keeps the full source at bar one with a review notice. The fixed 128-bar preview can cut long takes at its boundary; the full-file player and download remain complete.

Select the audio track, listen, and adjust **Start** and **Source in/out**. These changes keep the raw clock receipt intact. **Restore original clip** returns to the initial estimated placement after save/reload; it does not calibrate it. Solo voice and imported audio use the chosen bar. New audio extends the arrangement as needed, capped at 128 bars.

Finishing a song-mode take stops its backing playback. **Stop**, **All notes off**, and operations that stop the transport also finish a song-mode take; pending session changes remain blocked until its original saves. Starting an unrelated playback/audition is blocked while a take is pending. Stop, Finish take, and cancellation remain available even with an invalid note/clip draft. Tempo and count-in length cannot change during capture. The backing arrangement loops; the take remains one continuous clip, with no automatic loop comping. New notes save independently but enter the backing snapshot on the next playback start.

Physical microphone permission, actual recorded playback, audible alignment, latency calibration, and listening quality remain unverified on this locked Mac. Software lifecycle tests and the simulated timing session do not close those gates.

## Import audio

Choose **Import audio**, then select a WAV, MP3, M4A/MP4, FLAC, Ogg, or WebM file. Imports are limited to 30 MiB and two minutes. The browser must report a readable duration and decode the audio. Support varies with browser, container, and codec. An unfinalized WebM file may lack a finite duration and be rejected; recording within the studio does not require that import metadata path.

The original is stored unchanged under its content checksum. Its filename, MIME type, duration, byte length, and import origin remain in the project and its backup. Decoding for validation does not replace the stored original. Audio tracks have a full-take player and **Download original**. Arrangement playback honors optional `audio.startTick` placement and can cut audio at the loop boundary. Older microphone/import takes without placement metadata default to the loop start. New solo takes and imports use the chosen starting bar. Magenta candidates can choose a placement bar. Select any audio track to edit its exact start and source in/out points, audition that window, or restore original clip settings after reload. Full-file players and downloads keep the complete source. Transcription analyzes the selected window at the clip's saved position and records its decoded frame bounds. Accepted note tracks remain independent of later audio edits. See [clip editing](CLIP-EDITING.md).

If an import fails, retry it, download its retained bytes, or use **Cancel import**. Canceling an import does not delete the source file. If an asset was already written, it remains in the local content store; automatic orphan cleanup is not implemented.

**Import example phrase** loads a procedural four-tone WAV through the same import and saving pipeline. It is synthesized test material, not a human recording or an AI-generated song. Select that audio track and choose **Turn selected audio into notes** to run actual local Basic Pitch inference. Audition and acceptance keep its original audio intact.

## Observed verification

- The browser imported the 3.2-second example, ran Basic Pitch, auditioned and accepted four pitches, and reloaded the saved session. Its complete archive restored into an empty store with both original and analysis audio intact. See `verification/audio-import.json`.
- A separate temporary local service deliberately failed one audio write. The UI retained the original, blocked session changes, downloaded byte-identical audio, and retried without adding a duplicate track. The temporary service was stopped. See `verification/audio-recovery.json`.
- Audio HTTP responses provide byte lengths and byte-range seeking. The browser's original player reported 3.2 seconds after the fix. Automated checks cover full, prefix, suffix, and open-ended ranges and invalid requests.
- The **Recording placement qualification** browser session imports the unchanged procedural WAV at bar three (tick 7,680). The **Voice timing · simulated capture** session uses programmatically constructed recording metadata and procedural tones, with no microphone used. Its initial 1.75-second source in was restored after reload, then adjusted to 1.8 seconds and tick 97. The full source player still reports 3.2 seconds. The actual 221,364-byte browser-downloaded archive restored exact tracks, history, timing and original bytes into an empty store. The download event timed out despite the file completing; the actual file was verified independently. See `verification/voice-timing.json`.
- Six additional checks exercise permission-before-preparation ordering, timing sampled after loading, preparation/encoder failure cleanup, canceled preparation versus a newer take, unwrapped placement, fractional tempo, short takes, out-of-timeline review, original-settings restoration, and complete archives. That slice passed 98 tests; focused recorder/transport checks were rerun after the final capture-hook change.
- A later isolated service rejected session writes after the original upload and journal succeeded. Restarting that service and reloading the browser exposed the original for recovery at bar three. The locked expressive source stayed exact. A simulated missing acknowledgment then offered Finish recovery from another session, opened the correct original session, and kept its revision unchanged with one take. The actual 220,903-byte downloaded backup restored all tracks/history and its 282,284-byte source exactly into an empty store, then into the main studio as **Recovery restart qualification (restored)**. The isolated service and its test files were removed. See `verification/take-recovery.json`.
- Seven recovery checks cover original content IDs, restart/readiness, premature completion, concurrent retries, identity conflicts, corrupt/missing files, count/size limits, HTTP origin/revision guards, archive safeguards, and microphone-origin recovery without a new recorder. That slice expanded the suite to 105 tests; focused recovery/recorder checks were rerun after tightening source matching. Visible-panel layout checks at 1,440 and 680 pixels showed no overflow or console errors.
- Recovery-choice verification used a complete procedural original, a metadata-only unfinished upload, and deliberately malformed metadata. The UI set all three aside, kept the available source download, and produced an unchanged 533-byte note-only backup. After service restart/reload, returning the original restored the backup guard; recovery added it once at bar three. Its 220,914-byte complete backup restored exact tracks and original bytes into an empty store and the main studio. Raw journal hashes matched through set-aside, return and completion. Three additional checks cover reversible moves, malformed journals, and active/set-aside capacity limits; HTTP checks cover archive exclusion and rejection of already-attached set-aside requests. The full suite passes 108 tests. See `verification/recovery-choices.json`.
- Recorder tests use fake streams to exercise pending permission, cancellation, denial, interruption, final events, save failure, retry, and stale timers. They do not establish microphone hardware operation.

WAV decoding is qualified in this browser flow. Other listed containers have storage paths, with MP3/FLAC byte-preservation tests; their actual codec playback and human-recording quality are pending. External file selection in automated Chrome control remains blocked by the extension's file URL permission. No browser permissions were changed to work around it. The generated example avoids that selection step while exercising the real application import path.

![Controlled failure showing retained-original recovery](verification/capture-recovery.png)

![Reviewable simulated timing and complete source](verification/voice-timing.jpg)

![Completed original offered for recovery after a controlled failure and restart](verification/take-recovery-pending.jpg)

![Recovered project restored in the main studio](verification/take-recovery-finished.jpg)

![Reviewed set-aside entries and recovered original in a controlled fixture](verification/recovery-choices-review.jpg)

# Recording keys and pads in the native preview

This workflow records the built-in procedural instrument's output and preserves the original keyboard/pad observations. It does not open a microphone or generate music with an AI model.

1. Open or create a session. Save or discard existing drafts, then choose an output device if needed.
2. Press **Record keys/pads**. Wait for **Recording** and the actual output format before playing. The app saves the recording choice before opening output.
3. Use the onscreen keyboard/pads or enable computer-key input in the instrument controls. Keys, pads, velocity and sustain use the existing live instrument controls. Keep the instrument focused while using computer keys.
4. Press **Finish recording**. New input closes; the callback cuts held voices and saves its final original. Wait until **Original saved locally · not attached** appears and output has stopped.
5. Press **Attach recording to session** to add its audio to the song. Placement starts at frame zero and is uncalibrated. All original audio, event and control streams remain retained. Undo/Redo reverses the song attachment without deleting the original.

**Stop audio** remains available while preparation, saving or a dialog is active. Stop interrupts an unfinished recording; it does not promise a complete final tail or automatically attach audio. Keep the original locally if you want to return to editing without attaching it.

**Start live audition** plays without recording. **Advanced · solo input capture** is a separate input-recording workflow; it is not needed for keys/pads output recording.

## Retained choices and restart

The app retains exact recording choices and source locations in the local studio. Reopening never resumes recording, replays Start/Finish or automatically attaches a take. Open **Retained recording choices** with a clean session to review them.

A pending stored attachment can be retried explicitly in its original target session. The retry keeps its original operation, source and expected revision. A conflict or uncertain result keeps that request unchanged. Correcting a conflicted attachment choice is not available in this preview.

A recording choice alone does not prove its source exists or is sealed. Reopened sources without a stored attachment still need the forthcoming performance-aware inspection/recovery workflow. Generic retained-capture recovery does not handle these performance stages. Keep the source files; do not rename or replace an existing stage to record again.

## Preservation and current limits

Song backups include the complete attached original and song history. Unassigned recording stages and local pending choices are outside song backups and still need a separate export workflow.

Recorded audio can be arranged and edited with the existing clip controls. Original key/pad events remain read-only; editable-note derivation is a separate planned step. Writer-durable frames describe retained output, not every GUI action: known locally discarded staged events are counted, while the parent transfer-loss count remains unavailable.

The development ledger retains at most 32 recording identities and 16 revisions per identity. A full ledger rejects further choices without deleting prior evidence. This is a preview limit that must be addressed before release.

This build uses native catalog schema 5. Use the current native build for an upgraded store; older previews expect earlier schemas. Existing project/history/original content is preserved during the upgrade.

The [status](STATUS.md) and [release matrix](RELEASE-ACCEPTANCE.md) distinguish source tests, actual Mac UI checks, and remaining microphone, listening, latency, long-session, MIDI, AI, plugin and Windows acceptance.

# Metronome, count-in, and recording boundary

October 15, 2026. Implemented in the portable development studio.

The transport row provides **Metronome**, **Count-in** (Off, 1 bar, or 2 bars), **Click level**, and a visible loading/countdown/bar-and-beat status. Settings belong to the session and survive reload and complete project backups. Older projects default to no metronome, no count-in, and 35% click level.

## Play and record notes

Arm a note track with **R**, choose a count-in, then use **Play**. The count-in uses four quarter-note beats per bar at the session tempo. Count-in clicks sound even when the ongoing metronome is off; click level controls both. A level of zero leaves the visual countdown. The sound is a procedural sine click, with an accented first beat, not an AI model.

Loading, the count-in, music scheduling, and note-recording readiness use one Web Audio clock. Keys and pads can sound while preparing or counting in, but notes and raw MIDI capture begin at the song boundary. A key held through that boundary is not silently turned into a new recorded note; release and play it again. Saved song ticks exclude count-in time.

Metronome and click level can change during playback without restarting the song clock. Stop before changing the count-in length. **Stop** and **All notes off** cancel scheduled clicks and music, including a pending load or countdown. A canceled load cannot start music later, and an active load failure returns to a stopped transport. Missed clicks and old loops are skipped after a stalled tab rather than replayed as a burst.

## Audition and voice capture

Candidate and clip auditions start immediately, bypass project count-in/metronome settings, and show **Audition**. They monitor keys without recording into an armed source track. Accept a candidate, then use session **Play** to record along with it. Starting another playback finalizes held input and clears sustain before changing clocks. Unrelated playback/auditions stay blocked while a voice take is pending. Stop and recording finish/cancel bypass pending edit validation so an invalid draft cannot trap the transport.

**Record voice** keeps its own permission/capture lifecycle. With **Sing with song**, permission comes first, the backing snapshot loads, then capture begins before its scheduled count-in/song boundary. The saved clip uses an explicitly uncalibrated recorder-start command estimate, with preroll kept in the original file. Finishing, Stop, or All notes off ends a song-mode take and stops its backing. Solo voice and imports use a chosen starting bar. Review placement with the clip controls; see [recording details and limits](RECORDING.md). Native host ticks remain uncalibrated to audio time. Microphone latency, hardware keyboard/pad timing, audible click quality, dropouts, and native launch are separate acceptance work.

Playback uses a snapshot of the arrangement. Newly played notes save during recording; restart playback to hear them as part of the loop. The current transport supports the project's fixed 4/4 meter and single tempo, not tempo maps or a production realtime engine.

## Project data

Optional `project.transport` has `version: 1`, boolean `metronome`, integer `countInBars` in 0/1/2, and finite `clickLevel` in 0–1. Unknown fields are retained. Validation rejects invalid settings before a project save or archive restore. Clicks and the count-in are playback controls: they do not become notes, audio assets, or an exported MIDI prelude.

## Verification

The **Transport qualification** session uses a muted, locked synthetic source and two armed tracks. Browser automation canceled a two-bar count-in without adding notes, then used a one-bar count-in at 120 BPM. An on-screen key held under sustain during the countdown added no note, including when released after the boundary. Subsequent keyboard and snare-pad presses recorded at song ticks 688 and 1,748, with durations 19 and 18 ticks. These are automation-driven UI gestures, not physical controller measurements.

Live metronome changes kept playback running. The count-in selector was disabled while active. A procedural arrangement audition skipped the saved count-in; playing during it added no notes. Discard and reload left the exact saved project unchanged, with a one-bar count-in, metronome on, and 34% click level. The locked source remained exact.

The actual 869-byte downloaded archive restored all three tracks, their metadata, history, and transport preferences exactly into an empty temporary store. It has no audio assets because this fixture contains notes only. Independent Mido parsing found three note-ons, including the recorded ticks 688/1,748 with no exported count-in. The separate UMP trace retains full 16-bit velocities; it remains OmniuSong JSON interchange, not a standard MIDI 2 Clip File.

Eight new engine/clock checks cover count-in timing and accents, readiness at the boundary, live changes, stop and pending-load cancellation, load failures, audition isolation, skipped scheduling backlog, validation, and archive persistence. This slice originally passed 92 tests with no skips; the later voice-timing slice expands the suite to 98. Browser checks at widths 1,440 and 680 found no horizontal overflow or console errors. Earlier clip, note, and Magenta qualification sessions were unchanged.

Evidence: [receipt](verification/transport.json), [original source](verification/transport-source.json), [saved performance](verification/transport-project.json), [MIDI 1](verification/transport.mid), [UMP trace](verification/transport.ump.json), [count-in screenshot](verification/count-in.png), and [finished studio](verification/transport-preview.png).

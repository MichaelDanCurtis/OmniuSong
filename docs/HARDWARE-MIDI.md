# OmniuSong hardware and MIDI design

## Start with a playable desk

The starting rig is a Mac, headphones or monitors, a keyboard, pads, and a microphone. Use existing compatible equipment before buying anything. An audio interface with a suitable microphone input and headphone output makes recording and monitoring easier; the exact choice follows the user's current gear and recording needs.

The Akai reference is interpreted as an MPC-style interaction: pressure-sensitive pads, direct transport control, quick sampling, and an immediately playable instrument. The MPX line is sample-player hardware; confirm the exact unit before designing around its capabilities. The [MPK Mini Plus](https://www.akaipro.com/mpk-mini-plus/) combines 37 keys, 8 MPC pads, and a sequencer and is a useful compact reference. A 16-pad controller plus a larger piano keyboard may suit the desk better. No purchase is selected yet.

| Surface | Default musical mapping | Alternate AI mapping |
| --- | --- | --- |
| Keys | Play selected patch; preserve velocity and expression | Seed a continuation or constrain a melody |
| Pads | Drums, slices, clips, scenes | Audition candidates or choose a take |
| Knobs | Filter, envelope, level, send | Density, brightness, variation amount, prompt weights |
| Transport | Play, stop, record, undo | Capture a phrase for generation |
| Sustain pedal | Sustain performance | Hold a musical constraint during improvisation |
| Footswitch | Punch capture or end a take | Request or accept the next prepared variation |
| Microphone | Original voice or acoustic instrument | Melody transcription, chorus planning, singing input |

Controller maps are versioned portable assets. Their mappings cannot assume a particular model or force AI use during ordinary playing.

## MIDI 2 is a system path

MIDI 2.0 uses Universal MIDI Packets and includes high-resolution channel voice data. Capability inquiry and profiles have distinct specifications. A device, host API, musical-event model, instrument engine, and export format must all support the intended data for a complete workflow. The [MIDI Association specification collection](https://midi.org/midi-2-0-core-specification-collection) lists the current protocol and MIDI Clip File documents.

Most transcription models produce notes in a MIDI 1 representation. OmniuSong can retain their timing and pitch estimates in its richer project and encode supported MIDI 2 messages. That does not recover gesture precision that the model or input recording never observed.

| Layer | First preview | Completion requirement |
| --- | --- | --- |
| Project notes | 16-bit attack/release velocity including MIDI 2 zero, integer ticks, source/channel/group, internal voice identity, key-release timing, 32-bit expression lane values | Sample clock links and expressive playback |
| Protocol output | MIDI 2 note-on/off and per-note pitch UMP encoding | Remaining expression types and supported message families |
| Native input | CoreMIDI port requested with MIDI 2 protocol; source IDs, raw words and host-tick strings wired through the bridge | Virtual and hardware round trips; sample-clock calibration |
| Browser input | Web MIDI where available, ordinary MIDI 1 events | Clearly distinguish from native UMP input |
| Live output | Planned | Endpoint enumeration, scheduling, recovery, hardware acceptance |
| MIDI 1 files | SMF type 1 with tempo and note tracks | Broader independent reader tests and overlap handling |
| UMP interchange | Timestamped OmniuSong JSON trace | Document versioning and independent packet decoding |
| Standard MIDI 2 files | Planned | MIDI Clip File encoder/decoder and independent conformance |
| MIDI-CI and profiles | Planned | Inquiry, supported profiles, property exchange as needed |

The Mac preview uses [MIDIInputPortCreateWithProtocol](https://developer.apple.com/documentation/coremidi/midiinputportcreatewithprotocol(_:_:_:_:_:)) and reads event-list UMP words. CoreMIDI may translate a MIDI 1 source for a MIDI 2 protocol port. The port protocol label therefore does not prove that the attached controller is a native MIDI 2 device.

Windows should use [Windows MIDI Services](https://github.com/microsoft/MIDI) after verifying the installed service and SDK version on the actual host. Keep an explicit MIDI 1 fallback. MPE is a valuable expressive MIDI 1 mapping path and should remain available alongside MIDI 2.

## Input capture in this preview

Each active note has an internal UUID. Devices, UMP groups, and MIDI channels stay separate. Overlapping identical pitches on one MIDI address are paired FIFO because these messages do not carry a general voice identity. Pointer gestures use their internal note identities. Sustain is scoped to its device/group/channel. The on-screen Sustain button operates the computer and on-screen keys. Pedal release, device removal, focus loss, and All notes off finalize saved note lifetimes and cancel pending audio starts. Reconnecting CoreMIDI currently releases all native input voices.

New recorded note timing uses integer ticks without grid quantization. Sustained duration and physical key duration are separate fields. These are browser receipt and elapsed-time measurements, not sample-accurate audio alignment. Native host timestamps remain decimal strings in host ticks; no unsupported clock conversion is implied. The Web Audio preview does not yet perform recorded pressure or pitch gestures, note attributes, or half-pedaling.

While the selected note track is armed and playing, raw Web MIDI bytes or complete UMP messages are retained in `project.inputCapture`. The limit is 4,096 messages per project; the UI warns once and counts later omissions while note capture continues. This is a bounded preview capture, not an unlimited recorder. Unknown UMP messages are retained without claiming to interpret them. Notes from the computer and on-screen surfaces use local input metadata and do not invent wire packets. Backups preserve the raw capture. UMP JSON export includes it separately from the edited-note packet list, which uses track-assigned addresses and sustained durations.

MIDI 1 velocity uses min-center-max scaling into the project while keeping the original bytes. MIDI 1 zero-velocity Note On means release; MIDI 2 zero-velocity Note On remains a real note. MIDI 1 export clamps such an attack to 1 and reports the conversion. See the [official Windows MIDI UMP primer](https://microsoft.github.io/MIDI/kb/ump-data-format-primer/), [MIDI 2 bit scaling specification](https://amei.or.jp/midistandardcommittee/MIDI2.0/MIDI2.0-DOCS/M2-115-U_v1-0-1_MIDI_2-0_Bit_Scaling_and_Resolution.pdf), and [MIDI Association controller table](https://midi.org/midi-1-0-control-change-messages).

## Recording acceptance

Perform a real key press, sustain gesture, pad hit, and sung take. Confirm incoming events, resulting sound, a saved recording, and a reopened session separately. Measure round-trip audio latency with the actual interface; a UI timestamp or a successful build cannot establish it.

For MIDI 2 acceptance, use virtual endpoints first, then a real controller and receiving instrument. Compare recorded packets and audible expression. Test hot plug, device removal, repeated notes, overlapping same-pitch notes, panic, transport stop, and application focus changes.

## Audio plugin strategy

Audio Units are the first native plugin target on Mac. VST3 and CLAP remain portable targets. The [CLAP SDK](https://github.com/free-audio/clap), [VST3 SDK](https://github.com/steinbergmedia/vst3sdk), and [Carla host](https://github.com/falkTX/Carla) provide concrete interfaces and references to evaluate. The production host framework decision requires a small spike covering license, audio graph control, plugin crash behavior, latency compensation, state restoration, and expressive event support.

A plugin scan, a successfully opened plugin UI, and a usable recording session are separate acceptance gates. Keep model workers outside the realtime callback even when an instrument depends on a model.

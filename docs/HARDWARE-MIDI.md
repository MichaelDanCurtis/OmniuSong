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
| Project notes | 16-bit velocity, integer ticks, 32-bit expression lane values | Release velocity, source and channel/group, same-pitch note identity, sample clock links |
| Protocol output | MIDI 2 note-on/off and per-note pitch UMP encoding | Remaining expression types and supported message families |
| Native input | CoreMIDI input port requested with MIDI 2 protocol | Raw packet retention, source identity, timestamps, virtual and hardware round trips |
| Browser input | Web MIDI where available, ordinary MIDI 1 events | Clearly distinguish from native UMP input |
| Live output | Planned | Endpoint enumeration, scheduling, recovery, hardware acceptance |
| MIDI 1 files | SMF type 1 with tempo and note tracks | Broader independent reader tests and overlap handling |
| UMP interchange | Timestamped OmniuSong JSON trace | Document versioning and independent packet decoding |
| Standard MIDI 2 files | Planned | MIDI Clip File encoder/decoder and independent conformance |
| MIDI-CI and profiles | Planned | Inquiry, supported profiles, property exchange as needed |

The Mac preview uses [MIDIInputPortCreateWithProtocol](https://developer.apple.com/documentation/coremidi/midiinputportcreatewithprotocol(_:_:_:_:_:)) and reads event-list UMP words. CoreMIDI may translate a MIDI 1 source for a MIDI 2 protocol port. The port protocol label therefore does not prove that the attached controller is a native MIDI 2 device.

Windows should use [Windows MIDI Services](https://github.com/microsoft/MIDI) after verifying the installed service and SDK version on the actual host. Keep an explicit MIDI 1 fallback. MPE is a valuable expressive MIDI 1 mapping path and should remain available alongside MIDI 2.

## Recording acceptance

Perform a real key press, sustain gesture, pad hit, and sung take. Confirm incoming events, resulting sound, a saved recording, and a reopened session separately. Measure round-trip audio latency with the actual interface; a UI timestamp or a successful build cannot establish it.

For MIDI 2 acceptance, use virtual endpoints first, then a real controller and receiving instrument. Compare recorded packets and audible expression. Test hot plug, device removal, repeated notes, overlapping same-pitch notes, panic, transport stop, and application focus changes.

## Audio plugin strategy

Audio Units are the first native plugin target on Mac. VST3 and CLAP remain portable targets. The [CLAP SDK](https://github.com/free-audio/clap), [VST3 SDK](https://github.com/steinbergmedia/vst3sdk), and [Carla host](https://github.com/falkTX/Carla) provide concrete interfaces and references to evaluate. The production host framework decision requires a small spike covering license, audio graph control, plugin crash behavior, latency compensation, state restoration, and expressive event support.

A plugin scan, a successfully opened plugin UI, and a usable recording session are separate acceptance gates. Keep model workers outside the realtime callback even when an instrument depends on a model.

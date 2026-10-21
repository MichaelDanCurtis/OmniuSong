# Recorded-output seal cost and retained native failure

October 20, 2026 local; raw measurements recorded October 21 UTC. The current development bundle is `ffc90e34130702ba9bf5615ec9a9a8cc834266d2653d1334ad0dcc4301ac7188`. It is not a recording-qualified release.

## Actual native boundary

The earlier `4981d54a…` package completed a 9.002667-second output-only recording, normal seal, explicit attachment, Undo/Redo, complete archive restoration and fresh-generation playback. Its immediate post-recording playback failed a missing-ended-scope guard. See the [actual UI receipt](../verification/rust-native-recorded-output-ui.json).

The subsequent `a948b887…` attempt failed a live-progress guard. Its original remains unsealed with a verified 565248-frame checkpoint and unknown tail. The independently tested Panic watermark correction does not establish the cause of that particular native failure.

The `ffc90e34…` attempt recorded keys/pads, held a sustained voice and requested Finish. The UI showed 856576 rendered and durable frames, 1673 complete callbacks and Sealing, then reported only “Audio worker generation is retired; no new control admission.” The retained 14,716,928-byte source has no manifest and was never attached. Its SHA-256 is `d94a8986ebfa149546d0b5c244117fe969d6a31fae1e5f33913684452729bcaf`. Intent revision 2 retains the exact requested Finish; the initial revision 1 naturally contains no Finish. The later explicit Keep locally choice releases the review without deleting that evidence. The app was closed through its native window control and its process exit verified.

The exact failing phase cannot be recovered from that generic diagnostic. No source repair, recovery, resumed recording, fabricated callback receipt or normal seal has been applied to this original.

## Isolated storage measurements

The ignored manual helper `performance_writer::seal_cost_tests::profile_trusted_storage_seal_cost` requires explicit absolute source and fresh receipt paths. It reads the source without modification, copies every original chunk into a new isolated journal, compares original-row identities and times the equivalent authentication/storage operations in the debug build. It never opens a device or catalog, constructs a private native seal permit, or asserts callback retirement. Copy preparation and final all-chunk comparisons are excluded from the total below.

| Phase, seconds | 432128 frames / 7,495,680 source bytes | 856576 frames / 14,716,928 source bytes |
| --- | ---: | ---: |
| Owned view inspection | 0.944111 | 1.778630 |
| Semantic scan under held view | 0.938445 | 1.840604 |
| Complete-prefix authentication | 0.689718 | 1.407988 |
| Checked complete-prefix sealing | 4.000902 | 7.768577 |
| Four-phase total | **6.573177** | **12.795799** |

Raw receipts are `.runtime/rust-bounded-inspection/seal-cost-small.json` and `seal-cost-large.json`, with their adjacent logs and retained isolated clones. Both helpers passed, compared every original chunk and proved source bytes and directory names unchanged. These are two measurements of trusted storage work, not a benchmark distribution, a wall-time guarantee, physical durability proof or native lifecycle acceptance. The helper was executed before final formatting and the later wire-diagnostic correction; the final integrated source gate separately fingerprints the formatted helper and all production source.

The native SEAL_TIMEOUT remains **10 seconds**. The larger measured cost exceeding that deadline is a plausible explanation for the actual longer-take failure. It does not prove that the actual watchdog expired, nor that the native run followed identical timings. The native cause must be observed using the corrected diagnostic build.

## Source correction and next qualification

The new retirement diagnostic latches a bounded immutable trigger before publishing retirement. A final response fence returns that cause without a raced Ready/update; its exit decision belongs to the response actually prepared. Retirement arriving after a normal response waits for the next bounded request to carry the rejection. The parent validates request/result identity first and treats an update-free error as terminal before publishing any state. Nine deterministic regressions and the [independent review](RUST-PERFORMANCE-RETIREMENT-REVIEW.md) cover this source boundary. Existing deadlines, callback ownership, complete-prefix checks and normal-seal authority remain unchanged.

After the goal is resumed, qualify the diagnostic package against a fresh software-output recording of comparable duration. If the deadline is confirmed, profile repeated SQL preparation/full scans and design a measured long-recording strategy. Preserve authentication of every original row, transactional sealing, private native authority and bounded abandonment. A blind deadline increase or skipped validation is not an accepted fix. Then repeat Finish, explicit attachment and same-generation Play/Status/Stop, followed by history, complete archive and restart checks. Performance-aware recovery controls, editable-note projection, thirty-minute reliability, listening, MIDI 2/MPE, models/plugins, installers and Windows remain open.

The full goal is unfinished. At the end of this packet Codex reports it paused; no newer native package has been built or launched after the diagnostic source correction.

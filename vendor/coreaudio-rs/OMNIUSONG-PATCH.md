# OmniuSong native-input patch

Based on the exact crates.io coreaudio-rs 0.14.2 archive identified in UPSTREAM.json. Original MIT and Apache-2.0 license files are retained. This local patch is selected by the root Cargo.toml patch table; the user's registry cache is unchanged.

The input callback previously queried format properties and resized a Vec when callback frame counts changed. Cleanup reconstructed the allocation using the last view length as capacity. The local input path instead preallocates 16,384 frames before activation, rejects setup above that frame bound or 16 MiB, and keeps the original aligned Vec<u64> as the unique InputCallback allocation owner. Per-callback checked arithmetic changes the byte view only. The callback never reconstructs, resizes or frees the allocation. The backing allocation explicitly guarantees up-to-eight-byte sample alignment; no custom allocator over-alignment assumption is used. Normal cleanup drops its original owner on the AudioUnit control side.

An oversized/empty callback or AudioUnitRender failure delivers both zero bytes and zero frames to the client callback before returning a native error. OmniuSong must latch this as unknown loss and retain an unsealed journal. This sentinel is an explicit local integration behavior. It is not successful silence or known lost-frame coverage. No callback attempts a format/property refresh.

The separate test lock is retained for deterministic native-wrapper unit tests:

```sh
cargo test --manifest-path vendor/coreaudio-rs/Cargo.toml --lib --locked omniusong_input_buffer
```

These tests check bounded arithmetic and allocation ownership. They do not qualify the native driver, stop/quiescence, format changes, all CPAL error paths, actual microphone delivery, physical latency or power-loss behavior. The normal application PCM path also needs its independent allocation and lifecycle tests. Keep the upstream defect and this patch visible in release dependency notices and re-audit before changing either pinned dependency.

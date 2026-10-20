# OmniuSong Rust UI feasibility proof

Independent Cargo workspace, pinned egui 0.37.0. This is a headless research
fixture, not a production studio or renderer. It touches no saved projects.

```sh
cargo test --locked -j 2
cargo run --locked -j 2
cargo clippy --locked -j 2 -- -D warnings
```

Seven tests cover real egui geometry/AccessKit output, pointer and assistive
selection, exact human note values, bounded submission of a large offscreen
score, key repeat, text focus and focus-loss release. The waveform is synthetic.
No native window, audio, MIDI, models or personal recordings are used.

See `../../docs/research/RUST-UI.md` for the choice, current APIs, alternatives,
license sources and remaining acceptance gates.

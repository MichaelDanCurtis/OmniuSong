# Recorded-output presentation review

October 22, 2026. Bounded source review against baseline `3c7b450`; [display contract](../contracts/NATIVE-PERFORMANCE-PRESENTATION.md) and [verification receipt](../verification/rust-native-performance-presentation-tests.json).

The coordinator implemented the borrowed, read-only presenter and renderer changes. A separate test agent supplied real egui/AccessKit regressions. The architecture reviewer independently checked precedence, request/observation authority, exact storage-operation shapes, historical attachment semantics and unchanged lifecycle/control behavior. The coordinator inspected the code, tests, actual command outputs and preservation snapshots.

## Finding and resolution

The initial detail still said “not attached” when a sealed original existed but an attachment reply was unknown. Publication could already have committed, so that wording asserted more than the observation. It now says **Original saved locally · attachment not confirmed here**. The independent publication test covers both known conflict and unknown acknowledgement loss, retaining the exact publication, retry, error and original. Confirmed attachment wording identifies the completed operation, which may be a replay, without asserting current clip placement.

No remaining actionable production finding was identified in this bounded review. The presenter separates accepted recording/sealing observations, pending owner release, queued/active/retry storage work, first audio failure, latest storage error and historical uncertainty. Reopened stored Finish/attachment requests do not become active recording or sealed-source facts. Old and invalid statuses retain the existing admission checks.

The coordinator's exact comparisons confirm that mutable methods from `set_root` through `attach`, the card action/eligibility block, and retry/reopened-review controls are byte-for-byte unchanged from the baseline. No engine, storage, input, Stop, normal-seal or recovery authority was added. Repeated headless paint compares complete intents, receipts, worker revisions, task/engine queues, errors, flags and public guards before and after every frame.

## Evidence limits

Twenty focused regressions cover Start, actual Recording, local/saved/sent Finish, worker Finishing/Stopping/Sealing, automatic and explicit Stop, typed retirement, failures, sticky uncertainty, historical review through actual AccessKit clicks, attachment and release, malformed task shapes and priority. Real AX text and strict 240/480/1080 logical-width layouts are checked in both themes at native scale 1.5. This is headless UI evidence, not an actual native-window walkthrough, physical latency or listening judgment. The initial test harness dropped an unapplied intermediate texture delta; clearing it fixed that harness failure without a production change.

Final source/test/profile fingerprints and command results are in the receipt. The installed signed `7473e066…` source-review package remains unchanged. After the user's Allow confirmation it exited normally, reopened with the same signature and remained at SQLite → macOS `__open`; that stack alone does not establish the cause. No new native source Scan/Inspect, recording or input stream was submitted. Its source-review walkthrough and a later integrated presentation build remain pending separately. Full milestone 2 and the complete Rust Mac/Windows, MIDI 2/MPE, inference, plugin and release goal remain incomplete.

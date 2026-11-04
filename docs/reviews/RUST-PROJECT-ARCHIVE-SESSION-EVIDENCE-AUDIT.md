# Project archive session: artifact evidence audit

The frozen artifacts support the bounded in-process session-worker archive slice and the four inert schema-3 OSRE cases. All eleven final commands passed against unchanged inputs. They do not qualify native archive admission, source-aware consumers or an installed application workflow.

I compared saved artifacts and source; I did not execute the gate, Cargo, formatters, SQL, applications or devices. I previously implemented the DTO layer, so this artifact audit is separate from the [independent source review](RUST-PROJECT-ARCHIVE-SESSION-REVIEW.md). The coordinator alone collected protected-store preservation records.

## Artifact identity and source stability

The [verification packet](../verification/rust-native-project-archive-session-tests.json) is **75,902 bytes**, SHA-256 `7ac79842dcb9277298ba27442016c5d6a6c81b75c5990b7ba71c66a3777d1e48`. Its authoritative `.runtime/rust-project-archive-session/gate-final/gate-run.json` is **117,556 bytes**, SHA-256 `98117dbb289743ac244c196234018f495f9c63de4e1fe0c0b43f3c30f980a463`.

I verified every final raw-log fingerprint and reconstructed its test totals. The receipt's before/after inventories and packet inventory are identical. After rejecting absolute/parent-traversing allowlist entries, I independently recomputed **all 288 current source-input hashes and lengths**; each matches. The explicit finite audit allowlist itself hashes to `3997237c6e03df96bd06ab89b3c9d9b60aad9c4c23b9ce8779c37cbc21fd4181`. The separately bound source review also matches. Relevant exact fingerprints are:

| Artifact | SHA-256 |
| --- | --- |
| Contract | `45b1048c091c1dda73be629e20de4a1b2b201081816487213662a665bd144a25` |
| Design | `a2755f02422266d12817dffc32c00398d21c03aa988c45b446f7fb8638950993` |
| Independent source review | `a88066cddf9476c6b80670196df07e934077e28f63c9f6bb60f5fff5ffb58d8e` |
| Gate runner, read without execution | `7b9e0c8756250661e5a6afcdf92de36e7ceb8da5eac6995f524df8015cf66fef` |
| Packet builder, read without execution | `aceca61b7577984d551a38f6b72e9c7ec114788a5229da5276caba59b250ee45` |

The earlier `gate-qualified` candidate is correctly excluded: all eleven commands exited zero, but `passed=false` and `sourceStable=false`. Comparing its inventories identifies exactly one changed input, the contract's corrected copy-intent wording. The final complete rerun is stable. Historical baseline commit/addition provenance and preliminary transcript observations are packet declarations; they are not additional executed evidence in this audit.

## Reconstructed execution evidence

Final formatting, warnings-denied offline workspace Clippy and workspace build exited zero. Raw test-result lines give:

| Final test command | Passed | Failed | Ignored | Groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,479 | 0 | 9 | 73 |
| Store library, release | 275 | 0 | 1 | 1 |
| Session library, release | 151 | 0 | 2 | 1 |
| Engine projection review, release | 65 | 0 | 0 | 1 |
| Archive review, release | 8 | 0 | 0 | 1 |
| Archive copy review, release | 10 | 0 | 0 | 1 |
| Public archive service, release | 3 | 0 | 0 | 1 |
| Native raw-route guard, release | 1 | 0 | 0 | 1 |

The seven selected release commands total **513 passes**; these repeat selected workspace coverage. I matched all **36 distinct names** in the packet to their source test definitions, then found each exactly once as `ok` in the workspace log and exactly once across the selected release logs:

| Source group | Cases | Evidence class |
| --- | ---: | --- |
| `project_archives/tests.rs` | 19 | DTO construction, strict decoding/bounds, correlation, context and error/retirement representation |
| `project_archive_controls_review.rs` | 9 | Injected channel/backend/token composition and real pre-effect runner refusal |
| `tests/project_archive_service.rs` | 3 | Actual public SessionService workers and isolated storage operations |
| `governed_archive_review/evidence_archive_review.rs` | 4 | Actual isolated OSRE export/restore/refusal operations |
| App raw-route guard | 1 | Headless coordinator preservation |

The public service fixture uses actual software rendering, PerformanceWriter output, interrupted recovery, store-owned originals and authenticated projection publication. Its all-routes case executes both performance profiles; separate cases execute exact copy replay after input removal and a later human edit, and wrong-origin refusal followed by a valid request on the same worker. Independent assertions compare complete source catalog cells, raw originals, opaque document content and mapped publication identities. Those later test reads are not backend postcommit operations. Initialize succeeds with intentionally guarded projection summaries.

The four OSRE cases exercise published/copy-ancestor catalogs, ordinary unrelated content, independent framing/content/whole-file digests, complete catalog-cell/raw-tree preservation and numbered inert restoration. They inspect restored evidence after parking its input archive, refuse a replaced reviewed container without a completion marker, preserve coherently rehashed false operation music inertly while active closure/OSAR validation rejects it, and refuse export with a registered original missing. Inert preservation supplies no adoption authority.

Token isolation, late completion after cancellation, mixed source/main retirement, fatal-code fallback, recorded refusal and acquired second-check replay representation remain constructed controls. They do not execute a physical disk fault or concurrent acquired-replay race. Authentication-constructor counts were not instrumented. The native case checks all three raw operations plus an injected continuation, unchanged document/name draft, no queued work and a paint; it opens no native window and does not enumerate every musical draft.

## Preservation comparison and limits

The two authorized snapshot JSON files are each **51,310 bytes**. Their SHA-256 values are `1d331ace526d37c5554f79ffd65e57f488dcaa1f99b873a00090c7fb9687d02e` before and `43869a710444cbc3d726880685af2b191b4879e3c8827c80f8712a53e912fb75` after. I verified their packet fingerprints and exact equality of **tables, projectIds, files, catalogFile and binary**: **13 tables, 22 projects, 403 rows and 80 protected-file records**. Both coordinator integrity flags are true. The unchanged catalog record is 8,613,888 bytes / `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`; the installed-binary record is 23,062,544 bytes / `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`. I compared records only and never opened, hashed or followed their referenced targets.

No unresolved artifact discrepancy remains. The contract retains the initial unpaid-reservation outer-error exception, exact in-memory reference/retirement handling, 65,536-byte request and 262,144-byte reply boundaries, and no post-success load/hash/checkpoint in the runner. Identity restore still lacks a reviewed input-SHA parameter; export lacks an expected-revision parameter; no bounded Inspect command or serialized worker transport was added. Universal lower-layer physical-error retention, every-allocation or hard wall-clock bounds, durable restart admission/reconciliation, native warning presentation, source-aware reader/list/attachment qualification, actual UI/device/listening, power loss, Windows and release/installation acceptance remain unqualified. This evidence does not complete the full product goal.

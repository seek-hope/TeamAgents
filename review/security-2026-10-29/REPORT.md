# Source, dependency and robustness review (D-492)

Date: 2026-10-29. Starting source: `3d8e9e70`. Scope: user-authorized cleanup of stale documents and dead code,
plus concrete security and robustness fixes. This is a targeted source review with reproducible local probes,
not a new independent security audit or a proof that all remaining code is safe.

## Dependency findings and resolution

`cargo-deny 0.20.2` inspected each independent crate graph and fetched the current RustSec database. The
following advisories were present in the starting lockfiles.

| Area | Starting version(s) | Resolution | Advisory IDs |
|---|---|---|---|
| Core compression | zstd 0.13.3, zstd-safe 7.2.4, zstd-sys 2.0.16 | zstd 0.14.3, zstd-safe 8.2.0, zstd-sys 2.1.2 | RUSTSEC-2026-0391, -0398, -0403, -0410 |
| TLS and trust | rustls 0.23.41, rustls-webpki 0.103.13, webpki-roots 1.0.8 | rustls 0.23.46, rustls-webpki 0.103.16, webpki-roots 1.0.11 | RUSTSEC-2026-0285, -0346, -0348, -0349, -0381, -0382 |
| Async runtime | tokio 1.53.1 | tokio 1.53.3 | RUSTSEC-2026-0386 |
| Markdown | pulldown-cmark 0.13.4 | pulldown-cmark 0.13.5 | RUSTSEC-2026-0414 |
| TUI cache | lru 0.12.5 through ratatui 0.29 | ratatui 0.30.2, lru 0.18.5 | RUSTSEC-2026-0002, -0253 |

The YAML frontmatter parser now uses `serde-saphyr` instead of deprecated `serde_yaml` and its unmaintained
`unsafe-libyaml` dependency (RUSTSEC-2024-0449). Existing Skill tests cover the replacement. `resvg` 0.49 and
`ratatui` 0.30 also remove the rustybuzz and paste maintenance chains respectively. All three lockfiles were
updated; no unversioned dependency was introduced.

`deny.toml` and `make deny` now check all three graphs for vulnerabilities, unsound releases, yanked releases,
unmaintained direct dependencies and unknown sources. CI installs the same cargo-deny version and runs the
target. Maintenance-only notices on transitive dependencies are not represented as known vulnerabilities.

### Remaining maintenance notices

The TUI graph still contains bincode 1.3.3 and yaml-rust 0.4.5 through syntect, and ttf-parser 0.25.1 through
the pinned Mermaid renderer's font handling (RUSTSEC-2025-0141, RUSTSEC-2024-0320, RUSTSEC-2026-0192). They
have no safe drop-in upgrade in the current dependency chain. This review did not establish an exploit path
through TeamAgents' bounded inputs. The renderer's existing isolation and source fallback remain in place.
These are tracked maintenance liabilities, not silently reclassified as harmless or as vulnerabilities.

## Code findings and fixes

| Finding | Fix and regression |
|---|---|
| OAuth staging used a predictable PID path with create/truncate, allowing a pre-positioned symlink to overwrite its target; concurrent read-modify-write could lose services | Unique `create_new` staging, 0600, file and directory sync, a cross-process `flock`, and tests for a planted link plus 80 concurrent service updates |
| Unknown credential versions and arbitrarily large credential files were accepted or read without a cap | Version 1 only, a 4 MiB credential limit and explicit errors |
| OAuth token forms followed redirects, allowing credential material to be forwarded, and JSON/error bodies were unbounded | Redirects disabled; metadata/token/error bodies limited to 1 MiB; a two-listener test proves the redirect target receives no form |
| The loopback callback read an unbounded request line and could panic while cloning the stream | An 8 KiB line limit and fallible cloning; an over-limit probe is rejected |
| Stdio MCP lines, HTTP JSON/SSE bodies and accumulated SSE events were unbounded | A shared bounded-line reader and 16 MiB limits; invalid/oversized input fails instead of growing memory |
| Update metadata and archive downloads could grow memory/disk without a content cap | Metadata and sums limited to 1 MiB, archives to 256 MiB, declared/actual sizes checked, partial files removed on failure |

No dynamic OAuth provider or real release server was contacted for these checks. Tests use loopback fake
servers, temporary credential stores and the real parsers.

## Dead code and stale artifacts

- Removed unused `Envelope`, `ModelRequestRecord`, `Attempt`, `Artifact` and `Goal` mirror structs. SQLite rows
  and the schema remain authoritative; removing Rust types does not change the stored format.
- Removed the unreachable `TurnControl::wait_idle` and `DriverHandle::cancel_turn` paths. Existing interruption
  through the supervisor and control plane is unchanged.
- Extended `review/dead_code.py` to public named fields, shortened its stale narrative and reduced exemptions
  from 12 to the five still-used test/evaluation interfaces. Current result: 680 public items/fields scanned,
  zero unexplained findings.
- Removed an accidentally tracked zero-byte root `session.sqlite` and added a root-specific ignore rule.
- Removed the retired `docs/RUST-SANDBOX.md`; its historical content remains in Git. Current host-execution
  guidance no longer maintains a parallel sandbox document. Corrected the stale claim that the synchronous HTTP
  path had disappeared.

Historical decisions, acceptance evidence and frozen evaluation material were not rewritten as cleanup. The
large decision log remains authoritative history, not a low-information document to delete mechanically.

## Evidence and limits

Current checks are reproducible with:

```bash
python3 review/dead_code.py
make deny
cargo test --offline --locked --manifest-path core/Cargo.toml --lib
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib mcp_auth
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib mcp::
cargo test --offline --locked --manifest-path tui/Cargo.toml
```

An old-code control used a detached worktree at `3d8e9e70`, the old OAuth implementation and the new tests.
After correcting a test-only trait import, three original tests passed and all four new security tests failed
as expected: planted-link handling, credential format/size, redirect/response bounds and callback bounds.
The same four pass on the fixed source. Logs are `/tmp/ta-d492-before-probe2.log` and
`/tmp/ta-d492-mcp-auth2.log`; the first failed probe assembly is not counted as evidence.

Repository-wide final gates are recorded in ACCEPTANCE only after they run. Local tests and the advisory scan
do not prove semantic correctness of every unsafe block, protection from same-user host processes, complete
coverage of all external inputs, or future absence of newly published advisories.

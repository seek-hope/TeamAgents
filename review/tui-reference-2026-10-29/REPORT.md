# TUI reference study and selection (D-489)

Date: 2026-10-29. Scope: the user asked for a materially better TeamAgents TUI, explicitly naming Codex,
Grok Build and herdr and authorizing selection of the closest implementation followed by adaptation.

## Method and evidence boundary

We inspected pinned upstream source and official screenshots, then compared interaction and rendering fit with
TeamAgents. The upstream checkouts and preview material live under `/tmp/ta-tui-reference`; exact revisions and
selected source hashes are in [sources.json](sources.json). We did not adopt an upstream agent runtime, execute a
paid model workflow or claim a matched usability benchmark. Official screenshots show upstream products, not
TeamAgents. TeamAgents previews are scripted `TestBackend` frames; PTY checks use a fake daemon.

## Selection

| Reference | Useful implementation ideas | Fit decision |
|---|---|---|
| Grok Build `2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8` | Rust/ratatui single-agent conversation; elevated user-prompt band; quiet workspace/model chrome; live status between scrollback and prompt; task-oriented activity; `@` file references | **Selected as the base interaction and visual model.** Closest to TeamAgents' Leader conversation and existing renderer, so adaptation does not require a new protocol or runtime |
| Codex `79be2e201fb61e105178a62c8d8d77fa5ed6ad93` | Compact composer footer, semantic text roles, selection/approval surfaces and readable Markdown | Useful secondary reference. Its chat/app-server component graph is substantially more coupled to its own runtime, so it was not selected for wholesale adaptation |
| herdr `bc8c28988f75c538d4ca058b702f5adbea30566e` | Clear working/blocked/idle cues, mouse/keyboard parity and detachable terminal ownership | Not selected as the conversation base. Herdr is a terminal multiplexer with workspaces, panes and real child terminals; importing that model would replace rather than refine TeamAgents' single-Leader product surface |

All three repositories carry Apache-2.0 licenses at the inspected revisions. TeamAgents code remains its own
implementation: the adaptation uses the selected interaction patterns, not Grok's ACP state, agent loop,
configuration, marketplace, task model or plugin system.

## Adapted surface

- A quiet header carries TeamAgents, current state, optional session/workspace and authoritative goal usage.
- User prompts use a full-width elevated band with vertical padding. Assistant Markdown and media continue
  through the existing shared renderer.
- Consecutive tools receive operation-derived headings: Explored, Edited, Ran, Searched, Coordinated or Worked.
  A received receipt still does not imply semantic success.
- Non-idle work gets a one-row live activity surface between transcript and composer. Spinner repainting is
  presentation only and does not create runtime facts. Ready sessions do not reserve that row.
- The main composer has a prompt glyph, quiet rounded border, and model/effort in its lower border. Attachments
  share that border instead of adding a permanent panel. Approval cards retain explicit focus and one-shot
  `a`/`d` decisions.
- `@PATH` followed by Tab uses the existing daemon `input_paths` query. Accepting a candidate replaces only
  that token, stages the selected path and preserves the rest of a multiline draft. A later Enter submits the
  prompt and attachments together. Stale replies, duplicate paths and the eight-file limit do not widen this
  boundary. `/attach PATH` remains available.

## Preserved contracts

No daemon command, SQLite schema, grant, model request or runner behavior changed. The TUI still submits only
to the Leader. Slash completion remains local until explicit Enter. Ctrl+C interrupts the model request rather
than proving tool/process stop; Ctrl+Q only detaches. Display helpers, graphics fallback, per-session drafts,
queue revision checks, approval ownership and immutable attachment snapshots retain their prior contracts.
There is no resident herdr-style sidebar, split terminal or second agent runtime.

## Verification

Focused rendering/input tests, the full TUI suite and `pty_v2_smoke.py` cover wide/narrow layouts, model and
approval states, cursor placement, multiline `@` references, stale replies, attachment limits and the final
submitted frame. Repository-wide gates and the complete terminal matrix are recorded in ACCEPTANCE after their
actual runs; this report does not turn scripted frames into evidence of user preference or real-model quality.

The release TUI built from `d0828d7f` was installed locally after two checks found no running users of the
previous installed binary. The previous binary was preserved in a private backup and the replacement was
atomic; the engine and user configuration were not changed. The installed binary passed the same isolated
PTY smoke, including `@` attachment submission, approvals, editing, resizing and clean detach. The install
receipt is `/tmp/ta-d489-install.json`; this local installation is not a public release.

## Color follow-up (D-490)

The user subsequently requested Codex CLI's palette while retaining this layout. Additional inspected files
at the same Codex pin are recorded in the manifest: shared styles, contrast/selection handling and syntax
theme defaults. The adaptation replaces global Catppuccin chrome with terminal-default prose, dim secondary
text, ChatGPT Blue 200 accents and ANSI semantic colors. Unknown-background fallbacks supply inverse
selection and a dark-grey prompt band; explicit white prompt text prevents dark default text on that fill.
Fenced syntax uses Codex's default dark Catppuccin Mocha theme. No upstream terminal probe, adaptive contrast
solver, theme loader or automatic light/dark syntax switching is imported. D-489's runtime/input contracts
are unchanged; this follow-up does not rewrite the earlier selection or installation evidence.

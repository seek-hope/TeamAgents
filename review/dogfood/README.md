# dogfood: run the build on the repository's own fixtures

`run.py` copies one task fixture from `review/eval/r2-p6/tasks/` into a scratch directory, configures that
fixture's own `checks.txt` as a user-defined completion check (`[[checks]]`, D-50), runs `teamagents exec`
with the fixture's prompt, and then verifies the artifact by running the acceptance command **itself**, outside
the agent. It reports the model's report, the turn count, the session's workspace and whether the success
matches the artifact.

```bash
make build
python3 review/dogfood/run.py --task edit-integrity
python3 review/dogfood/run.py --task rust-fix --state-dir /tmp/ta-dogfood
```

It is a real-model check: it needs the credential named by the profile's `api_key_env` (`DEEPSEEK_API_KEY` by
default) and always runs the model at its native context window (D-36). It is not part of `make check`.
Everything it writes stays under `--state-dir`; the repository's fixtures are only read, and the frozen
evaluation material is untouched (its Chinese prompts are *input data*, which is why the exception in
AGENTS.md exists).

This is the check that found D-57: run against `edit-integrity` it reported success while the session had
worked in the *repository* rather than the given `--cwd` (60 turns, 291 s, and the edited fixture left in the
repository root). After that fix the same task runs in 6 turns and ~9 s, and `rust-fix` in 8 turns and ~8 s
with its `cargo test` check passing both in the runtime's check round and independently.

## `authority.py`: the user's authority surface end to end

`authority.py` runs one session in two turns and checks the capability boundary itself:

```bash
python3 review/dogfood/authority.py                                  # fresh /tmp state root
python3 review/dogfood/authority.py --state-dir /tmp/ta-authority --timeout 180 --budget 12
```

1. the Leader spawns one worker and asks it whether it can run shell commands in the shared workspace — a
   spawned worker holds no `shell@workspace` (§5.1), so it cannot, and `proof.txt` must **not** exist;
2. `teamagents authority` reads the session (instances and grants with their ids) and `authority grant` gives
   that worker `shell@workspace`;
3. a second instruction asks the same worker to run `printf granted > proof.txt` — now `proof.txt` must exist
   with the expected content;
4. `authority revoke` takes the capability back and the probe asserts that no live shell grant is left.

Measured (DeepSeek Flash, native window, isolated state root `/tmp/ta-authority-run`, 2026-09-25): turn 1
8.9 s and the worker answering *"No — I cannot run shell commands in the shared workspace. My runtime exposes
no bash/exec/terminal tool"*; the grant in revision 8; turn 2 14.0 s with the worker reporting the command's
exit code 0; the revocation in revision 11; 13 model requests, both tasks `SUCCEEDED`, no failed request.

Two things it guards against, because both were observed while developing it:

- **A model that answers with prose and never calls `finish` while its task stays open is re-asked by the
  runtime** (unbounded without a goal budget) — the first version of this probe asked a worker *without* the
  shell grant to run a shell command, which is unachievable, so the model answered `BLOCKED.` as prose and the
  runtime asked again: **169 model requests / 1,226,717 prompt tokens in ~15 minutes** with no progress. The
  prompt now asks what the worker can do (it can answer that, and does), and the probe measures the request
  count against `--budget`, parks the worker through the daemon protocol and reports the finding if it sees
  the loop. The gap itself is recorded in `docs/ACCEPTANCE.md`; it needs the user's decision, not a probe's.
- **The session shape of this probe is the one that produced D-62**: turn 1 ends on an accepted `finish`, and
  turn 2 is a new task in the same epoch — the second request used to be rejected by the provider
  (`HTTP 400 ... must be followed by tool messages responding to each 'tool_call_id'`). The first run failed
  exactly there; the wire projection now answers every call it carries, and the run above (and a 169-request
  run after the fix) has no failed request at all.

It is a real-model check (same credential and native-window rules as `run.py`), and everything it writes stays
under `--state-dir`.

## `providers.py`: a team that spans two providers

`providers.py` runs one session with the Leader on DeepSeek Flash (native 1M window, D-36) and a worker
spawned with `model = "worker_kimi"` — the Kimi entry of the user's own catalog shape (262,144 tokens) — and
delegates a file write to it:

```bash
python3 review/dogfood/providers.py                  # fresh /tmp state root
python3 review/dogfood/providers.py --state-dir /tmp/ta-providers
```

It needs `DEEPSEEK_API_KEY` and `KIMI_API_KEY`, then asserts the three facts the acceptance row claims: the
session really spanned two models (each member's resolved model is recorded, D-69), the delegation exchanged a
task assignment and a task result, and `answer.txt` holds exactly the line the task asked for. Measured
(2026-09-25): 7 model requests, 14.0 s, goal `SUCCEEDED`, `i-leader` on `deepseek-flash`, `worker1` on
`k3-256k`, `task t1 SUCCEEDED`; re-measured after D-71 on the same shape: 8 requests, 23.1 s, all four
assertions green.

**This harness is intermittent, and the reason is worth knowing** (measured 2026-09-25): a run whose Kimi
worker answered with *prose* ("Confirmed: answer.txt written…") instead of calling `finish` left
`task_completed` missing — the task stayed `RUNNING` (D-65's ceiling: the runtime does not re-ask a model that
stopped settling), the Leader read the artifact itself, and the goal still settled `SUCCEEDED` with that task
open, because `complete_goal` checks open **operations**, not open **tasks**. The run is honest about it (the
harness fails on the missing event, and `docs/ACCEPTANCE.md` records both the ceiling and the question), and
the goal's own claim rests on the artifact the Leader verified.

## `runtime_note.py`: the runtime's own closing note rides the next turn

`runtime_note.py` runs two turns against one state root: the first ends with
`finish(status = success)` (the goal settles `SUCCEEDED` and the runtime's closing note joins the
conversation), the second is an ordinary prompt whose request therefore carries that note.

```bash
python3 review/dogfood/runtime_note.py                      # DeepSeek
python3 review/dogfood/runtime_note.py --provider kimi      # over `responses`
python3 review/dogfood/runtime_note.py --providers deepseek,kimi
```

It needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi) and asserts what D-71 claims: after a settlement
the context carries an entry of kind `runtime` in the *user's* voice, and the next turn is accepted by the
provider (`end=reply`, exit 0) instead of being rejected for its shape. Measured (2026-09-25): deepseek
turn 1 1.6 s / turn 2 0.9 s, kimi 8.9 s / 11.8 s, both sessions' note at `i-leader:0:4` as
`runtime`/`role: user` — the shape is safe on the thinking-mode chat wire and on Kimi's `responses` wire.

## `queued_input.py`: a queued input is answered by its own turn

`queued_input.py` runs two sessions turns against one state root: the first settles its goal in its first
response (no tool round in between), and the second is submitted *while* that request is in flight, so its
input is queued at the boundary (D-63). The probe waits for the first turn to be in flight instead of sleeping
through it, and reports whether the second run really was queued:

```bash
python3 review/dogfood/queued_input.py                    # DeepSeek
python3 review/dogfood/queued_input.py --provider kimi    # over `responses`
```

It needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi) and asserts what D-72 claims: the first run reports
its own settlement, and the queued run reports `end=reply` with *its own* word and `goal_status: null` — not
the settlement the first run left behind. Measured (2026-09-25): deepseek 2.8 s
(run 1 `completed`/`SUCCEEDED`; run 2 `reply`/`BANANA`/`goal: null`), kimi 40.4 s with the same shape.
Against the pre-fix build the same harness reports `end=completed / goal=SUCCEEDED / reply=null` for the queued
run: the prompt was never answered, and the run claimed the earlier turn's goal as its own.

## `mcp.py`: a configured MCP service is really bound

`mcp.py` writes a user config whose `[tools.probe]` entry is a tiny stdio MCP server (one tool, answering a
token the server generates when it starts), then asks the model in one headless run to call that tool and
report its output:

```bash
python3 review/dogfood/mcp.py                    # DeepSeek
python3 review/dogfood/mcp.py --provider kimi    # over `responses`
```

It needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi) and asserts the chain the design promises: the
server starts with the session, it is asked for its tools, the model calls the tool and the run reports the
tool's own output (the unguessable token makes a good guess fail). Measured (2026-09-25): deepseek 2.5 s,
kimi 9.6 s, both `end=reply` with the token, the server's log showing `initialize`/`tools/list`/`tools/call`.
Before D-74 no surface could bind a configured service: the same config started the session, never spawned
the server and never offered the tool.

## `workspace.py`: the git-worktree lifecycle end to end

`workspace.py` makes a real git repository and has the model spawn a `git_worktree` worker, delegate a file
write to it, and wait for the result. It then walks the documented lifecycle:

```bash
python3 review/dogfood/workspace.py                    # DeepSeek
python3 review/dogfood/workspace.py --provider kimi    # over `responses`
```

It needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi) and asserts: the member's file is in its own
worktree and **not** in the shared project (the wrong-tree class D-57 found); terminating it with uncommitted
work keeps the checkout and reports why in `daemon.log`; and after the probe commits and merges the branch, the
running session retires the checkout by itself, record included. Measured (2026-09-25): deepseek 16.4 s, kimi
41.8 s, both goals `SUCCEEDED`, one refusal line per reason after the D-76 fix.

## `skills.py`: the configured Skills registry reaches the model

`skills.py` writes a skills root with two skills and drives both halves of the tool. The first skill's *body*
carries a token generated for the run (deliberately absent from the YAML description, so a model that only
searched cannot know it); the second is findable only through `search`, because its keyword (`frobnication`)
lives in its description and nowhere else:

```bash
python3 review/dogfood/skills.py                    # DeepSeek
python3 review/dogfood/skills.py --provider kimi    # over `responses`
```

It needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi) and asserts: `doctor` reports the same registry the
session uses (`2 skill(s) under 1 configured root(s)`), the model calls `skill {action: read}` and the receipt
carries the **body** (with the run's token), the instructions are followed (the file exists with the token),
and then — in a second turn — the keyword search finds the second skill by its description and the model
follows that one too. Measured 2026-09-26: deepseek 6.3 s (turn 1) with the action sequence
`read canary → search frobnication → read inbox-triage`, kimi 63.9 s with the same sequence; both goals
`SUCCEEDED`, and in the deepseek run the model verified its own work with a shell `cat` (the skill's third
step).

## `web.py`: the bound web tools work and their guard holds

`web.py` runs two turns in one session with a `[tools.fetch]` binding (the credential-free half of the web
tools): the model fetches `https://example.com` and reports the page's title, then it is asked to fetch a
private address and to report the tool's answer verbatim.

```bash
python3 review/dogfood/web.py
```

It needs `DEEPSEEK_API_KEY` (or `--provider kimi`), network access for `example.com`, and asserts: the fetched
page's **body** reaches the conversation (a `web_fetch` receipt carrying `Example Domain`), and the private
address is refused by the runtime (`{"error":"refusing private address for 127.0.0.1"}` — the SSRF guard,
observed live rather than only in `guard_url_blocks_private_targets`). Measured (2026-09-26): deepseek 3.6 s
(turn 1); kimi 7.9 s with the same refusal. `web_search` still needs a provider credential this machine does
not have, so its evidence stays the unit tests.

## `crash.py`: a daemon crash replays nothing

`crash.py` kills the daemon while a shell command is in flight and then starts the session again:

```bash
python3 review/dogfood/crash.py                    # DeepSeek
python3 review/dogfood/crash.py --provider kimi    # over `responses`
```

The prompt asks for a command whose only trace is a counter (`echo run >> runs.log; sleep 20`), then for a
file, then for a finish. The probe waits for the instance to reach `TOOLS_PENDING` (the command really in
flight), kills the daemon (the client exits 2, which is expected), lets the *runner* — a separate process,
A12 — finish the job, and resumes the session. It asserts the core claim with a model in the loop: the
command ran **exactly once** across the crash (`runs.log` holds one line, so the receipt was consumed, not
replayed), the file exists, and the resumed session settles coherently instead of timing out. Measured
(2026-09-26): deepseek and kimi both `end=completed`, goal `SUCCEEDED`, `input_queued=true` (the resume waited
behind the recovered turn), one line in `runs.log`.

## `checks.py`: the completion gate with a real model

`checks.py` configures one `[[checks]]` entry that can never pass (`test -f never-written`), asks a real
model for a small file, and then watches the gate do its job. The gate is protocol-agnostic in the design, so
the same scenario runs on either catalog entry — the wire for the check round's synthetic entry differs per
protocol, and that difference is where D-70 lived:

```bash
python3 review/dogfood/checks.py                  # fresh /tmp state root, DeepSeek
python3 review/dogfood/checks.py --provider kimi  # the same scenario over `responses`
python3 review/dogfood/checks.py --state-dir /tmp/ta-checks --timeout 420
```

It needs `DEEPSEEK_API_KEY` (or `KIMI_API_KEY` with `--provider kimi`). The assertions are the acceptance
row's claims: the work really happened (the file exists with the asked content), no success was reported
(`exec` exits 1, `end=failed`, the goal ends `BLOCKED`), the settlement is a `goal_completed` event, and the
repair ledger names the failing check (`check_id: impossible`, `class: exit`) while the model sees its output
in the conversation.

Measured (2026-09-25): on **deepseek** 11 model requests / 9.7 s, on **kimi** 8 requests / 37.8 s — both exit
1, goal `BLOCKED`, the artifact exact, and the model explicitly reporting that creating `never-written` to
satisfy the gate would be bypassing it. Two defects came out of this harness: the first deepseek run failed
with `chat API 400: The reasoning_content in the thinking mode must be passed back to the API` (D-70: the
repair turn after a failed check died on the wire), and the first kimi run reported
`exit 0 / end=reply / goal=None` although the goal was `BLOCKED` (D-71: the runtime's own block note was
stored in the member's voice and read back as the reply).

## `tui.py`: the surface a user opens first, with a model in it

Every other harness here drives `exec`, so the client half of a turn had never met a real daemon and a real
model in one run. `tui.py` forks the real binary in a real PTY (the interface `make pty` covers against a
*scripted* daemon), waits for the session and the leader on screen, types a prompt, presses Enter, and
requires the answer on screen before checking the session's own database:

```bash
python3 review/dogfood/tui.py                    # DeepSeek
python3 review/dogfood/tui.py --provider kimi    # over `responses`
```

It needs `DEEPSEEK_API_KEY` (or `KIMI_API_KEY` with `--provider kimi`) and writes only under `--state-dir`.
Two design points are the whole value of this probe (D-85): the screen needles are the *rendered* form
(`i-leader TUIDONE`, `you Reply with the single word`), because the bare answer word is part of the
instruction and is already sitting in the composer before the turn — the first version "passed" the answer in
1.1 s that way; and the run carries its positive control (bare word present, labelled one absent) so an
insensitive needle is reported instead of passing quietly.

Measured (2026-09-26, native windows): **deepseek** attached in 2.4 s, answer on screen 2.7 s after Enter,
8.0 s total; **kimi** 2.7 s / 2.5 s / 7.5 s. Both runs end with exactly the two expected `context_entries`
rows for `i-leader` (the user prompt and `"content":"TUIDONE"`), which is the assertion: the client and the
session agree about the same turn.

## `input_latency.py`: how fast the interface keeps up with typing

`input_latency.py` measures the client's own responsiveness against the **scripted** daemon from
`pty_v2_smoke.py`, so the number is the client's timer (no credentials, no model, no session state):

```bash
python3 review/dogfood/input_latency.py
```

Measured (2026-09-26): a single keystroke reaches the composer with min 0.01 s / median 0.03 s / max 0.11 s
latency, and ten characters written as one burst render in 0.05 s. This is why `tui.py` types its whole
prompt at once: what appears late in that case is a burst being rendered, not a user's keystroke lagging.
The guards are deliberately loose (1 s per keystroke, 0.3 s median, 2 s per burst) so a loaded machine
reports numbers instead of a red gate.

## `boundary.py`: the state roots the CLI refuses

`boundary.py` is the model-free half of this directory: it drives the three entry points against state
roots they must **not** open, and needs neither a credential nor a network.

```bash
python3 review/dogfood/boundary.py
```

1. **Someone else's database** (A34): a root holding a `session.sqlite` with a `users` table. `doctor` exits
   1 with the FAIL line, `exec` refuses in 0.2 s with exit 2 naming the file and the foreign table, and the
   probe re-hashes the file: the bytes are identical afterwards. Before D-87 the daemon's `create = true`
   path adopted the file, wrote the whole v2 schema into it and ran a real model turn.
2. **A second daemon** (A33): the first owns the root, the second exits 1 naming the coordinator, and after
   the first is killed with `SIGKILL` a new daemon binds the same root again instead of inheriting a lock.

Measured (2026-09-26): all four assertions hold; `sha256` of the foreign file unchanged
(`4242ca5de3fc…`), refusal latency 0.2 s, second daemon exit 1, restart after SIGKILL in under a second.


## `cancel.py`: does a running command really stop?

`cancel.py` answers A13's product question with a real model and a real command, and it needs no Leader in
the loop beyond hiring the worker:

```bash
python3 review/dogfood/cancel.py
python3 review/dogfood/cancel.py --state-dir /tmp/ta-cancel --stop-window 30
```

The Leader spawns one worker (a spawned worker holds no `shell@workspace`, §5.1), the probe grants it through
`teamagents authority grant`, sends the instruction to that member directly (the same `submit_input` the TUI
sends), and then pulls `teamagents instances terminate --id … --yes`. Two things are asserted, and both can
fail: the **artifact** — `heartbeat.txt`, written five times a second by the command the runner spawned —
must stop growing (1.5–6 s after the lever across the runs of 2026-09-26), and the operation's **receipt
class** must be `cancelled`, which is how the probe tells the user's lever apart from the command simply
running into its own 120 s tool timeout.

The probe also documents the level the lever lives at: `tasks cancel` is delegation-level (it lands the task
`CANCELLED` and releases the delegator, while the assignee's operation keeps running), which is why the D-68
CLI test and this probe cover different halves of A13 (D-88).


## `approval.py`: the user's decision, taken in the TUI

The gated call is the one moment the product asks the user, and this probe is the only one that decides it in
the real TUI with a real model:

```bash
python3 review/dogfood/approval.py                  # Ctrl+A then 'a': approve
python3 review/dogfood/approval.py --decision deny  # Ctrl+A then 'd': deny
```

The daemon runs **without** `--full-auto`, so every `shell` call parks (`require_shell_approval = !full_auto`)
and the approved command executes inside bubblewrap — this is the only probe that exercises that path. The
pending id has to be on screen before the key is pressed (the panel is a surface, not decoration), and the
artifact decides afterwards: on approve `proof.txt` holds the content, the approval is `APPROVED`, the box
drops the id and nothing stays pending; on deny the file never exists, the approval is `DENIED`, and the
operation lands `CANCELLED` with receipt class `denied` (the model is told). Measured 2026-09-26, five runs.

Two probe lessons are recorded with it (D-89): an absence check must be the absence of a *proven present*
thing (the first version waited for UI text that never exists, and the fix asserts the decoded id's
disappearance instead of an empty list), and a model may ask for a *second* decision in the same turn, so the
assertion names the decided id rather than the whole list.


## `stale_check.py`: a verified input that changed blocks the goal

A16 (`checks.py`) is a check that can never pass; A17 is the subtler half — the check *passes*, and then the
thing it verified is not what would be delivered. `stale_check.py` makes that deterministic: the check's own
command rewrites the file it declares as its input.

```bash
python3 review/dogfood/stale_check.py                  # DeepSeek
python3 review/dogfood/stale_check.py --provider kimi  # over `responses`
```

The `[[checks]]` entry is `id = "bound"`, `command = "printf changed > out.txt"`, `inputs = ["out.txt"]`, and
the model is asked to write `out.txt` with `original` and finish. The artifact decides both halves: the file
ends up holding *the check's* content (so the check really ran and the gate really saw a different value),
the model's write is in the conversation, and no goal is ever reported `SUCCEEDED` — the run ends `failed`
with the goal `BLOCKED` and the reason `required checks failed (bound:stale_inputs) after 3 round(s)`.
Measured 2026-09-26: deepseek exit 1 / 19.7 s / 3 rounds / 12 requests; kimi exit 1 / 56.6 s / 3 rounds /
9 requests (D-90).


## `job_identity.py`: the running job's identity, and its one-start rule

Two claims live in the runner's journal and neither had a live witness:

```bash
python3 review/dogfood/job_identity.py
```

While the member's command runs, the probe reads
`<state root>/instances/<id>/jobs/<operation>/journal.json` and **re-derives** the recorded identity itself —
`start_ticks` is field 22 of `/proc/<pid>/stat`, `boot_id` is `/proc/sys/kernel/random/boot_id` — so the check
compares the runner's record with the machine (A15). It then asks the runner over the socket name the *token*
derives (§6.2) and requires agreement, sends a **duplicate GO** and requires `starts` to stay at 1 with the
same pid (A10), and connects with a *guessed* token, which must be refused. Measured 2026-09-26, two runs: all
four hold, and a guessed token gets `ConnectionRefusedError`.

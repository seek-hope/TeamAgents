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

`skills.py` writes a skills root with one skill whose *body* carries a token generated for the run (the token
is deliberately absent from the YAML description, so a model that only searched cannot know it), points
`skills_paths` at that root, and asks the model to read the skill and follow it:

```bash
python3 review/dogfood/skills.py                    # DeepSeek
python3 review/dogfood/skills.py --provider kimi    # over `responses`
```

It needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi) and asserts: `doctor` reports the same registry the
session uses, the model calls `skill` with action `read` and the receipt carries the **body** (with the run's
token), and the instructions are followed (the file the skill asks for exists with the token). Measured
(2026-09-25/26): deepseek 5.9 s, kimi 17.4 s, both goals `SUCCEEDED` — and in the deepseek run the model
verified its own work with a shell `cat`, which is the skill's third step.

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

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
default) and always runs the model at its native context window (D-36).

**Every probe stops the daemon it started.** `exec` autostarts one and the daemon is detached on purpose
(background work survives a client exit, §9), so a probe that just ran would otherwise leave a live session
behind on the user's machine; `atexit` calls `leak_guard.stop_daemons(<scratch root>)` for every probe here —
the pid-based stop of `review/leak_guard.py`, which never guesses by pattern (`pkill -f "daemon --state-root
<root>"`, the shape these probes used until D-148, also matches any shell whose command line merely mentions
that string; D-144 measured it killing two of a session's own shells). **Every probe also removes its own scratch** (D-138): the default `<TMPDIR>/ta-<name>` goes away at
exit, because a directory left per run accumulates until the machine's `TMPDIR` fills — the defect D-131 fixed
for the test suite, which the probes shared for 31 files. Pass `--state-dir` to keep a run's state for
inspection: that is the escape hatch when a probe fails, and it is left alone on purpose. It is not part of
`make check`.
Everything it writes stays under `--state-dir`; the repository's fixtures are only read, and the frozen
evaluation material is untouched (its Chinese prompts are *input data*, which is why the exception in
AGENTS.md exists).

## The credential-free subset, in one command

These probes need no model and no credential — `budget.py`, `truncation.py`, `input_latency.py`,
`providers.py --self-check`, and since D-138 `boundary.py`, `tui_panels.py`, `tui_reconnect.py`, `shutdown.py` and
`geometry.py` (they drive the real CLI, real daemons and the real TUI, but a member begins no turn: the session's
`model_requests` table stays empty). `probes.py --self-check` requires this list to name every probe in the set,
because the sentence had gone stale — it said "seven", named seven, and the set had eight (D-233). `review/dogfood/probes.py` runs a whole set and reports one line per probe:

```bash
make probe-offline                     # needs `make build`; the 9 credential-free ones, about two minutes
make probe-models                      # the probes that take a model, one after another (~7 min)
python3 review/dogfood/probes.py --list
python3 review/dogfood/probes.py --only checks.py --set models
env -u DEEPSEEK_API_KEY -u KIMI_API_KEY python3 review/dogfood/probes.py   # the offline set needs no credential
```

**The model-choice sweep 2026-09-27** (after D-187/D-196): a probe that needs the model to *do* one particular
thing is a latent flake unless either the prompt makes that thing necessary or the probe classifies what
happened instead. Read probe by probe, the model set handles it in exactly those two ways, and the one place
that cannot is recorded:

| Probe | What it needs from the model | How that is secured |
|---|---|---|
| `team_ring.py` | hire exactly two teammates and `send` one token to a named id | the prompt states both actions and the ids verbatim (`HIRE`/`RELAY`), so the alternative is not a path the model can take |
| `providers.py` | spawn a worker on the Kimi catalog entry and delegate one file | numbered instructions naming the key and the exact content; failures name the shape ("the worker did not run the Kimi entry") |
| `workspace.py` | spawn a worker with `workspace = git_worktree` and delegate one file | numbered instructions naming the policy; failures name the missing artifact |
| `skills.py`, `hooks.py`, `web.py`, `mcp.py` | call one bound tool (`skill`, a shell call the hook vetoes, `web_fetch`, the MCP tool) | the answer is only obtainable through the tool, so not calling it cannot produce a passing run; the failures say so ("no `skill` call for canary is in the conversation", "an answer without one is a guess") |
| `authority.py` | report that the worker could not run a command, then run it after the grant | the premise is retried once and, when it still fails, the probe reports which shape it saw (D-143) — the only model-set probe whose premise is not made necessary by its prompt |

The remaining probes drive the product through its own entry points (a cancel, a deadline, a crash, a queued
input, the TUI's keys) and assert product state; their per-item evidence is in `docs/ACCEPTANCE.md`. This sweep
looked for choice-dependent assertions, not for weak ones — a probe can be robust to the model and still assert
too little, and D-187's `stale_check.py` was exactly that until it asked what the model had been told.

**That other half was done 2026-09-27** (D-197): each probe's docstring claim was read against its
`failures.append` messages. The result is a negative one and it is the point — the assertions cover
their claims. `crash.py` checks not only that the work completed but that the command ran **exactly
once** across the crash; `unknown_outcome.py` checks the operation's class, the park, exactly one
notification and the absence of a success claim; `boundary.py` checks the refused file's **bytes are
unchanged**; `two_gates.py` and `checks.py` check that the model was told about the runtime check's
failure; `hooks.py` checks the veto, the reason reaching the model and the two allow-shapes for a
broken hook; `instructions.py` is built to *flip* when the feature lands. What no reading settles is
whether a covering assertion is itself strong enough (a tolerance too loose, a count too generous):
`probes.py --self-check` gates the mechanical part — every probe must be cited by an acceptance row
or a decision — and the rest stays a reading, stated as such.

**The model set re-run 2026-09-27, at `db90bf52`** (all 26 in one pass, 941.5 s): **24 green, 2 red**, and the
two reds have different dispositions. `stale_check.py` (165.0 s) failed on its assertion that `out.txt` holds
what the *check* wrote: the goal ended BLOCKED with the model's own report (D-187's path), two check rounds each
recorded `class: "stale_inputs"`, no success was claimed, and the file held the model's `original` — because the
model re-wrote its deliverable on the repair turn, so the last writer was the model and not the check. The
ending was honest and the verdict reached the model, so the *probe* was the defect: it required an order the
claim does not, and it now asserts that the file holds one of the two values the scenario writes (D-204 — the
lesson D-187 applied to the ending, one assertion over; re-run alone after the fix, green in 16.8 s).
`authority.py` (390.6 s) reproduced the recorded D-143 shape instead of passing: after the grant the worker was
offered `shell=yes` with the tool list and never ran the command — `task=none` after 16 requests, the turn's own
deadline ending the probe — which is the ACCEPTANCE-recorded gap, and the probe's own message says so rather
than reporting a surface defect. The pass left **0 daemons and 0 new scratch directories**, and `providers.py`
(23.1 s, one team spanning DeepSeek and Kimi), `protocols.py` (31.4 s, every wire protocol against a real
service) and the other 22 are green.

**The model set re-run 2026-09-27, at `f521fd4f`** (all 26 in one pass): **24 green, 2 red, and both reds were
findings rather than flakes.** `stale_check.py` failed on its two assertions about the stale-input verdict, and
the state it kept showed the runtime doing its job — two `completion_repair` events carrying `class:
"stale_inputs"`, no success claim, the goal BLOCKED — while the *model* was never told why its completion had
been refused: the §8 gap D-187 fixed. `authority.py` reproduced the recorded D-143 shape instead of passing:
turn 1 closed in neither attempt (`exit 124` after 600.4 s and 600.5 s), the probe's own premise — a worker
that answers without settling its task leaves the leader's wait pending, which is the ACCEPTANCE-recorded gap —
holding both times, so this is that gap and not a regression. The pass also reported one scratch directory it
had not created, `/tmp/ta-stale-d187`: a probe run *beside* the harness (this document's own), whose guard
correctly saw a new directory appear in `TMPDIR` during the pass, and correctly stayed quiet in the re-run
below, where nothing ran beside it.

**The check family re-run after D-187's fix, same day** (`probes.py --set models --only checks.py --only
stale_check.py --only two_gates.py --only exec_check.py --only authority.py`, 810 s): **all five green** —
checks 15.4 s, stale_check 12.3 s (reporting "the runtime recorded the stale input", "the conversation carries
the verdict", "the runtime parked the goal naming the stale input: required checks failed (bound:stale_inputs)
after 3 round(s)"), two_gates 13.2 s, exec_check 6.8 s and `authority.py` 762.4 s. Authority's own flakiness is
visible in the pair: the same probe failed after two 600 s turns in the pass above and closed in 762 s here —
D-143's shape, and the reason its budget is the largest in the set. Two live runs of `stale_check.py` also
settled the goal two *different* ways (the model parked the goal after three rounds in the post-fix run; the
pre-fix run's model settled it itself with an honest blocked report), which is why the probe now asserts what
must hold in both instead of the park reason alone (D-187).

**The model set re-run 2026-09-27, after the D-163…D-175 pass** (one probe at a time in five chunks, on the
revision those changes ended at): **25 of the 26 green** — every probe except `run.py`, the fixture task that
verifies itself outside the agent and takes a task argument rather than a session — including `authority.py`,
which passed on its first attempt, so D-143's shape did not reproduce and its witness had nothing to classify. Two observations worth
keeping: `authority.py` took **617.8 s** (against 21.0 s in the earlier run below), which is what a run looks
like when the model is slow to use an offered tool without the shape actually failing; and the whole set left
0 daemons and 0 scratch directories. The formal gates were re-run on the same revision: `make verify-model-all`
(11 configurations, no error), `make verify-model-counterexamples` (10 refutations) and `make verify-kani`
(3 harnesses). The paragraph below is kept as the record of the run before those changes.

**An earlier model set re-run (2026-09-27**, after D-153's runner-lifecycle change, one probe at a time in six
chunks): 25 of the 26 green — `authority.py` was the exception, reproducing the open D-143 shape (its dated note
carries the kept session), and `deadline.py` ran through the harness for the first time (D-155, green). That
run's own accounting: 0 daemons and 0 scratch directories left.

It counts the daemons and the scratch directories before and after the run and fails if the set added either,
because those two leaks are how D-111 and D-131 were found; the rules themselves live in
`review/leak_guard.py`, which `make test` uses too, so the harness's counting and the suite's counting are one
implementation (D-147). The model-requiring probes still run one at a time, each at its model's native window
(D-36).

Each probe runs with an explicit `--state-dir` under the harness's own root, so the harness can clean up after
a probe it had to kill (a signal skips the probe's own `atexit`) and can **keep** the state of a probe that
failed — the line it prints for that probe names the directory, which is where the evidence is. A run in which
nothing failed removes its root.

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
- **A member's offered surface is now a witness, not an inference** (D-143): the probe starts the daemon with
  `TEAMAGENTS_LOG_SURFACE=1`, and the driver then writes one line per request into `daemon.log`
  (`driver: surface <instance> shell=yes|no tools=…`). The probe prints the worker's lines before and after the
  grant and, when the command does not run, says which side failed — *never offered* (a product finding) versus
  *offered and unused* (the model's choice) — instead of reasoning from the model's own account of its tool
  list. Measured 2026-09-27: `shell=no` for the three requests before the grant, `shell=yes` for the two after
  it, the worker confirming "a `shell` tool is now present in my toolset", and `proof.txt` written.
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

It needs `DEEPSEEK_API_KEY` and `KIMI_API_KEY`, then asserts the facts the acceptance row claims: the session
really spanned two models (each member's resolved model is recorded, D-69), the delegation exchanged a task
assignment and a task start, `answer.txt` holds exactly the line the task asked for, and the worker settled its
task (`task_completed`). Measured (2026-09-25): 7 model requests, 14.0 s, goal `SUCCEEDED`, `i-leader` on
`deepseek-flash`, `worker1` on `k3-256k`, `task t1 SUCCEEDED`; re-measured 2026-09-26: 8 requests / 15.7 s and
8 requests / 22.6 s, both green with `task_completed` present.

**A missing `task_completed` is neither read as a regression nor accepted silently** (D-129): the worker is a
real model and sometimes answers with *prose* ("Confirmed: answer.txt written…") instead of calling `finish`.
Then the task stays `RUNNING` (D-65's ceiling: the runtime does not re-ask a model that stopped settling), the
Leader reads the artifact itself, and the goal still settles `SUCCEEDED` with that task open, because
`complete_goal` checks open **operations**, not open **tasks** — the recorded known gap in `docs/ACCEPTANCE.md`.

## `protocols.py`: every wire protocol, accepted against a real service

DESIGN §7 keeps four protocol families apart (Chat Completions, DeepSeek extensions, Anthropic, Responses) and
requires that each is "accepted with a real service separately". The tree had contract samples for all four
(`engine/tests/providers_fake.rs` drives a fake HTTP server through each adapter) and live runs on two wires —
nothing said which family had a live acceptance and which had only a fake. This probe is the live half for every
family, one small goal each (write a file, `--check` verifies it) in its own state root:

```bash
python3 review/dogfood/protocols.py                    # every family whose credential is set
python3 review/dogfood/protocols.py --family anthropic
python3 review/dogfood/protocols.py --strict           # a family not accepted fails the run
python3 review/dogfood/protocols.py --self-check       # the table against the code, no model and no key
```

Each run takes the native context window from the service's **own model list** (`/models`; D-36's "value and
source") and writes that value into the config, so a window a vendor changed shows up as a mismatch instead of
a quiet shrink; it then asserts the turn really completed on that wire (exit 0, a settlement, the artifact, a
recorded request) and that the adapter's native field survived into the stored message (`reasoning_content`,
`anthropic_blocks`, `responses_output`). `--self-check` re-derives the endpoint, the native field, the protocol
dispatch and the contract tests from the code, so the table cannot drift.

Measured (2026-09-26, all four, no `--strict` needed): `anthropic` `k3-256k` via Kimi's Anthropic-compatible
`/v1/messages` (exit 0, 2 requests, 7.7 s, `anthropic_blocks` kept), `chat/completions` `k3-256k` (2 / 7.7 s,
`reasoning_content`), `deepseek` `deepseek-flash` (3 / 3.1 s, `reasoning_content`), `responses` `k3-256k`
(2 / 9.2 s, `responses_output`) — and both declared windows match the tree's recorded values (`1048576`,
`262144`). A family whose credential is missing is printed as `NOT ACCEPTED LIVE` with its contract tests, so a
gap is stated rather than skipped.
The probe accepts that one shape only (artifact exact **and** goal `SUCCEEDED` **and** the task still open) and
prints it as the gap; every other shape where the event is missing still fails the run.
`python3 review/dogfood/providers.py --self-check` checks that rule with no model, no network and no
credentials.

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

## `shutdown.py`: a graceful stop with a command in flight

DESIGN §9 says a normal daemon shutdown "freezes new dispatch, persists pending work and then stops itself"; the
crash path had a probe (`crash.py`) and the graceful one had none. `shutdown.py` runs the same shape with **no
credentials and no network** — a local chat-completions server scripts the turn that dispatches
`sh echo run >> runs.log; sleep 20` — and then stops the daemon the supported way:

```bash
python3 review/dogfood/shutdown.py                    # fresh /tmp state root
python3 review/dogfood/shutdown.py --state-dir /tmp/ta-shutdown
```

It asserts what the design promises (measured 2026-09-26, D-152): the stop is bounded (0.3 s) and **settles
nothing** — the operation keeps `DISPATCH_COMMITTED`; the runner survives and finishes the command alone; the
next daemon settles that operation `SUCCEEDED` **from the runner's journal** with its receipt; the command ran
exactly once; and the session stays usable, with the input that arrives after the recovery already settled the
goal reporting its own outcome (`end=unsettled`) rather than the earlier settlement. It ends with 0 daemons and
0 runners.

## `checks.py`: the completion gate with a real model

`checks.py` configures one `[[checks]]` entry that cannot pass in any workspace state (`exit 1`: a builtin, so
no content the model writes changes its status — the file test this probe used to carry could be satisfied by
writing the file the check names, and a model did exactly that, D-146), asks a real model for a small file,
and then watches the gate do its job. The gate is protocol-agnostic in the design, so the same scenario runs
on either catalog entry — the wire for the check round's synthetic entry differs per protocol, and that
difference is where D-70 lived:

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

Measured (2026-09-26): on **deepseek** 10 model requests / 16.2 s, on **kimi** 9 requests / 36.9 s — both exit
1, goal `BLOCKED`, 3 check rounds and 2 repair rounds, the settlement `blocked_by: runtime`, the artifact
exact, and 0 job runners left (A12). The probe prints which of the two honest routes settled the block, because
they are not the same evidence: the runtime blocks a goal whose checks exhaust their repair rounds, while a
model that reads a gate no workspace state can satisfy may concede on its own first (with an editorializing
comment in the config, the deepseek run did; with the neutral wording the probe now writes, both providers kept
claiming success and the runtime decided — the config is model-readable, D-146). Two defects came out of this
harness: the first deepseek run failed with `chat API 400: The reasoning_content in the thinking mode must be
passed back to the API` (D-70: the repair turn after a failed check died on the wire), and the first kimi run
reported `exit 0 / end=reply / goal=None` although the goal was `BLOCKED` (D-71: the runtime's own block note
was stored in the member's voice and read back as the reply). Its first version (2026-09-25, 11/9.7 s and
8/37.8 s) measured the same gate on a check the model could have satisfied.

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


## `hooks.py`: the user's own policy hook, live

`[hooks]` is where the user's programs wrap the runtime: `notify` on events, `pre_tool` in front of every
native tool call (exit 0 allows, exit 2 denies with the first stderr line as the reason, anything else
allows). Its unit tests are all in-process; this probe drives it with a real daemon, a real model and real
tool calls:

```bash
python3 review/dogfood/hooks.py                 # the veto must deny the shell call
python3 review/dogfood/hooks.py --mode broken   # negative control: exit 1 must NOT deny
```

It generates `notify.sh` (records `argv[1]` plus the JSON payload from stdin) and `veto.sh` (records what it
is asked about, denies the `shell` tool by exit code) in the scratch root, then asks for one file write and
one shell command in one turn. Veto mode: the file write happens, the shell command never does, the model's
conversation carries the hook's reason verbatim, and the model finishes `blocked` of its own accord. Broken
mode: the same hook exiting 1 allows the call — without that run, "the veto worked" would also be consistent
with a hook that denies everything. The `notify` stream in those runs carried 4–6 `tool_call` events
(including `ok: false` for the denial) and 4–5 `run_completed`, all valid JSON with the session and instance
ids; that count also corrected the guide, which now says `run_completed` fires per model request (D-92).


## `exec_check.py`: the CI contract of `exec --check`

The headless entry point's acceptance commands are what a CI job hangs on, and this probe drives the three
outcomes it can see, in one real session with a real model:

```bash
python3 review/dogfood/exec_check.py
```

1. a **passing** check after a turn that really wrote `hello.txt`: exit 0, ledger `ok: true / exit_code: 0`;
2. a **failing** check followed by a second one: exit 1 and a ledger with one row — the list stops at the
   first failure — with the first turn's file still in place;
3. a command that prints `(exit 0)` and exits **7**: the verdict is 7 (`ok: false`), exit 1, and the forged
   text stays in `output`.

4. a second session **without** `--full-auto`: a turn that parks on an approval exits 3 with
   `verification: []` and `verification_path: null`, and the ledger an earlier passing run in that session
   wrote is untouched — an approval stop verifies nothing (§8).

Measured 2026-09-26, three runs (D-93). That fourth scenario also fixes what the ledger is: a run without
verdicts writes no file and reports a null path, so the reliable signal is the path in the run's own report,
not the presence of `<state root>/verification.json`.


## `tui_panels.py`: the instances panel's keys against a real daemon

The PTY smoke drives the panels against a scripted daemon (asserting the frames each key produces). This probe
attaches the real TUI to a real daemon to check the thing a user cares about — that the key acts on the row
they selected and that the daemon really changes state:

```bash
python3 review/dogfood/tui_panels.py
```

No model, no credential, ~10 s: the session starts, the probe creates one extra instance through the
documented protocol, and then `Ctrl+N` (instances view) → `Down` (the selection marker `▶  i-worker` moves) →
`p` (paused in `instances --json` **and** repainted as `i-worker · PAUSED`) → `r` (ACTIVE again) → `Ctrl+N`
(tasks view) with a task the probe delegated itself → `c` (cancelled in `tasks --json` **and** repainted
`t-panel · CANCELLED`) → `Esc`, `Ctrl+N`, `Down` (back on the worker's row) → `t` (only asks: the footer
wants `y`, the instance stays ACTIVE) → `n` (cancelled, still ACTIVE) → `t`,`y` (TERMINATED, and the panel
shows it). Measured 2026-09-26, five runs (D-95).


## `deadline.py`: the goal deadline, end to end

A35's formal coverage says the daemon refuses past the goal deadline; this probe shows what a user meets, and
it is the probe that caught a gap in D-97's own fix:

```bash
python3 review/dogfood/deadline.py     # ~70 s: one minute of waiting is the point
```

`[limits] deadline_minutes = 1`, one short turn creates the goal with its absolute deadline (printed from the
session), the probe waits until it has passed, and the next run exits **1 in 0.1 s** with
`failure: "goal goal-s-main deadline passed before request … could start"`. The session is the witness: exactly
one model request (the refused turn never reached a model), a `goal_deadline_refused` event, the leader
`PARKED` with the same reason — and the `--check` the probe attaches never runs (no marker file,
`verification: []`, `verification_path: null`), which the first run of this probe found still happening.


## `lifecycle_run.py`: the user's levers against a run in flight

Two levers, two different truths, and the harness that measured both:

```bash
python3 review/dogfood/lifecycle_run.py                     # terminate: the run ends at once with the reason
python3 review/dogfood/lifecycle_run.py --lever reset       # reset: the epoch move is named, also at once
python3 review/dogfood/lifecycle_run.py --lever pause       # pause + resume: the run still finishes
```

`terminate` closes the instance's open execution, so the waiting run ends 0.2 s later with exit `1`,
`end=failed` and `failure: "instance i-leader is terminated; this run cannot finish (termination is final —
start a fresh state root for new work)"` (before D-98 it waited out its deadline and said `timeout`). `pause`
is a boundary: the run keeps following its turn, and after `instances resume` the *same* run finishes —
measured `exit 0 / end=completed / goal SUCCEEDED` with the file on disk. `--lever reset` speaks the
protocol's `reset_instance` (no CLI verb) and the run ends 0.2 s later with `failure` naming the epoch move
(`epoch 0 → 1`) — the case D-100 fixed, where the client used to wait out its deadline. Measured 2026-09-26,
three runs per lever (D-98/D-100).


## `tui_reconnect.py`: the TUI against a daemon that dies and comes back

```bash
python3 review/dogfood/tui_reconnect.py
```

No model, ~25 s: the TUI attaches and its panel lists an instance created through the protocol; the daemon is
`SIGKILL`ed; the client says it is disconnected, and a key pressed meanwhile reports `command failed` in the
conversation (a panel command that fails is not swallowed). A new daemon on the same state root then brings it
back — measured 0.6 s until the status line clears, with an instance created *after* the restart appearing in
the panel, which is the only non-vacuous proof that the client is live again (the pre-kill row is still painted
either way).

The first run found the defect D-99 fixed: the client really reconnected while the status line kept saying
"disconnected" for 90 s, because clearing the flag did not rebuild the frame.


## `two_gates.py`: the runtime's gate and the client's gate in one run

The product has two acceptance mechanisms — `[[checks]]` gate the *goal* (§8/A16), `exec --check` decides the
*exit code* of a finished turn (D-49) — and nothing had driven them together:

```bash
python3 review/dogfood/two_gates.py
```

Two scenarios on their own fresh state roots: a runtime check that cannot pass in any workspace state (`exit
1` — the file test this scenario first used was satisfiable, and a model satisfied it by writing the file the
check named, D-146) plus a passing client check gives goal `BLOCKED` (the repair ledger names the check) with
exit `1` and the client's verdict `ok: true`; a passing runtime check plus a failing client check gives goal
`SUCCEEDED` with exit `1`. Which is the division of labour stated in one sentence in `docs/USER-GUIDE.md` §1:
neither gate replaces the other. Measured 2026-09-26, 16.0 s and 5.2 s (D-101, D-146).


## `instructions.py`: `instruction_files` is declared, validated and unread

```bash
python3 review/dogfood/instructions.py
```

The config key is accepted, its path is validated, and `doctor` used to print
`[ok  ] instruction files  1 file(s) reach every member's prompt` — while nothing in this build reads those
files: a member's system text is its profile's `instructions`. The probe configures a file holding a canary,
runs one real-model turn and reads the prompt the leader was given: the canary is **not** in it, and `doctor`
now says `declared, not applied` (D-102). It is written to flip — when the feature lands, the assertion becomes
"the canary is in the prompt" and this probe is its acceptance test.


## `mcp_http.py`: the HTTP transport, its bearer token, and a required service that cannot start

The `mcp_transport = "http"` half of the MCP surface had no test at all before this probe: the offline suite
and `mcp.py` both drive stdio.

```bash
python3 review/dogfood/mcp_http.py
```

It stands up a minimal streamable-HTTP MCP server on loopback and runs two scenarios with a real model:

1. `bearer_token_env_var` set → the model is offered only the declared tool, its call comes back as
   `probe-pong-ping-1` in the conversation, and the server's log shows every POST carrying
   `Authorization: Bearer <token>`;
2. the same binding with that variable **unset** → the runtime parks the instance with
   `required tool service "probe" is unavailable: binding "probe": bearer token env var PROBE_MCP_TOKEN is not
   set`, and `exec` exits `2` naming the parked leader and the resume lever (`instances resume --id i-leader`).

The second scenario is what found D-104: before the fix the coordinator task died on the boot failure and the
session stayed up and silent — no event, no log line, no model request — while a headless run waited out its
whole deadline.

## `team_ring.py`: a message travels around a team (A02's live half)

A02's offline evidence is the control plane (`control::messages_flow_across_an_authorized_ring`) plus the
authority surface D-61 gave the user. What a user does with a team is different: they ask a Leader to hire
people, and then the members *talk*. This probe builds the smallest ring that is still a ring — A → B → C → A —
and puts one token through it.

```bash
python3 review/dogfood/team_ring.py
python3 review/dogfood/team_ring.py --state-dir /tmp/ta-ring --timeout 300
```

1. The user asks the Leader to hire two workers (`exec`); the probe reads the member ids from the session.
2. A spawned worker holds no authority of its own (§5.1/D-61), so the user grants both of them
   `message@session` through `teamagents authority grant` — the permission step is part of the ring.
3. The user submits one small instruction to B ("send exactly this text to C"), then one to C ("…to i-leader"),
   each through the daemon's `submit_input` (the same call the TUI makes).
4. The artifact is the **recipient's own context**: the delivered envelope is rendered by the runtime as
   `[message from <sender>] <text>`, so delivery *and* attribution are read from the session, never from a
   model's prose. The probe also asserts the two `message_sent` events exist with the expected sender and
   recipient (the recipient is the event's scope).

A hop is re-asked up to three times when a turn did not send anything: the claim is about delivery, not about
one turn's obedience. Measured (2026-09-26, DeepSeek Flash, native window): the ring closed in **23.5 s** and
**14.2 s** in two runs; the first run also shows the members talking more than the minimum (B replied to A as
well), which the probe tolerates — it asserts the ring's hops, not silence. One finding from writing it: a
delivered message is a `user`-kind context entry (not a `message`-kind one), which is why the probe's wait
looks for the rendered `[message from …]` line instead of a kind.

## `unknown_outcome.py`: the crash that leaves nothing verifiable (A09)

`crash.py` covers the *recoverable* crash: the daemon dies, the runner survives and finishes the job, and the
receipt is consumed exactly once (A08/A11). A09 is the other half — the crash that leaves nothing verifiable —
and it is a promise about honesty rather than about recovery.

```bash
python3 review/dogfood/unknown_outcome.py
python3 review/dogfood/unknown_outcome.py --provider kimi --state-dir /tmp/ta-unknown
```

1. the Leader hires one worker (the product path), and the user grants it `shell@workspace` through
   `teamagents authority grant` (a spawned worker holds nothing of its own, §5.1/D-61);
2. the user asks for a delegated task whose text is exactly one shell call
   (`sh -c 'echo run >> runs.log; sleep 20'`), and waits until **the runner's own journal says the command
   started** — see below for why that witness and not the operation row;
3. the probe kills the **runner** (so no terminal receipt is ever written) and then the **daemon**, and starts a
   cold daemon: recovery respawns a runner over the same job directory, which marks the journal
   `OUTCOME_UNKNOWN` (a runner that finds itself restarted over a non-terminal state cannot know whether the old
   child produced effects);
4. asserts: `runs.log` holds **one** line (the command ran once and was never replayed), the worker's operation
   is `OUTCOME_UNKNOWN` with receipt class `outcome_unknown`, the delegated task is `BLOCKED` with exactly one
   `task_blocked` notification, and no goal claims success. The orphaned command (its process group outlives the
   runner, D-41) is then stopped by pid, which is the user's own lever (`docs/USER-GUIDE.md` §4).

Measured (2026-09-26, DeepSeek Flash, native window): two consecutive runs green, each ~10 s. Writing it took
three predicates, and the two wrong ones are the interesting part:

* "the worker is in `TOOLS_PENDING` and `runs.log` exists" — a model can *write* the side-effect file itself
  (one run did), so the probe killed nothing and there was no crash to recover from;
* "a shell operation is `DISPATCH_COMMITTED`/`RUNNING`" — that only means the driver committed the dispatch. A
  runner killed before it accepts the GO leaves a `READY` journal, and recovery then starts the command once,
  which is correct (§6.2) but is A08's scenario: the probe measured a *successful* replay instead of an unknown
  outcome;
* the shipped one — the runner's journal is `START_ACCEPTED`/`RUNNING` **and** the delegated task is `RUNNING`.

## `budget.py`: the ceiling is enforced before any money is spent (A18)

```bash
python3 review/dogfood/budget.py
python3 review/dogfood/budget.py --state-dir /tmp/ta-budget --max-tokens 500
```

A session whose `[limits] max_total_tokens` is below one request must refuse the request **before any model
call**, park the leader with the runtime's own arithmetic, and report that through the headless client instead of
waiting out its deadline (D-97). The probe runs one `exec` (with a `--check` attached, to catch the case where a
turn that never started still verifies something) and asserts on the session database: zero `model_requests`
rows, a `budget_refused` event carrying `known 0 + reserved 0 + est 2228 > max 1000`, the leader `PARKED` with
that sentence, the goal carrying the ceiling, exit **1** with the same words in the report's `failure`, and no
acceptance command run (D-96).

Unlike its siblings this probe needs **no credentials and no network**: the refusal precedes the provider, so a
dummy `api_key_env` value is enough and the run is over in about a second. Measured 2026-09-26: three runs green
(two at 1000 tokens, one at 500 — which correctly reports `> max 500`).

## `truncation.py`: A19's whole path over a real socket (no credentials, no network)

A19 had two halves: the provider edge classifies a truncated stream per protocol (in-process fake servers), and
since D-117 the driver's retry is tested with a scripted provider. Neither drove the engine's own HTTP/SSE stack
over a socket. This probe does, with a local chat-completions server, and it needs nothing else:

```bash
python3 review/dogfood/truncation.py
python3 review/dogfood/truncation.py --state-dir /tmp/ta-truncation
```

Two scenarios, both through the real binary:

1. **truncated before any visible text** — the connection closes after a delta that carries only a tool-call id.
   The attempt is transient, the driver retries inside the turn, the second response completes with a `finish`
   call, and the goal settles `SUCCEEDED`. The probe asserts: exit 0, goal `SUCCEEDED`, exactly **two** requests
   seen by the server, and the session's attempts table holding `FAILED` with **no usage** and `error_class`
   `Transient` next to the priced `COMPLETE` one; the truncated attempt's call id never reaches the conversation.
2. **truncated after visible text** — the connection closes after a delta with content. The attempt is
   *permanent* (retrying could duplicate text the user already saw): exit **1**, `failure` starting with
   `permanent model error:`, exactly **one** request, one `FAILED` attempt with class `Permanent`, and the
   partial text absent from the conversation.

Writing it found D-123: the driver sends the failure class in every `record_attempt` and the `attempts` table has
the column, but nothing wrote it — so this probe's classification assertions failed against rows that all said
`NULL`. Fixed in the control plane, with a core test; measured 2026-09-26, both scenarios green in ~6 s.

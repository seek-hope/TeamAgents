# TeamAgents user guide

This guide describes the current implementation. Notes from earlier releases and the migration material
were removed from the tree and stay reachable through Git history (`git log -- docs/archive`).

## 1. Quick start

```bash
teamagents init                      # write the config (kept if it exists) and prepare the state root
export DEEPSEEK_API_KEY=...          # the environment variable named by api_key_env
teamagents doctor                    # config, credentials, state root, skills, bubblewrap and host checks
teamagents                           # open the TUI (starts the per-user daemon when needed)
```

- **One daemon per user**: `teamagents` probes `$XDG_STATE_HOME/teamagents/v2/daemon.sock` and, when it is
  missing or refuses the connection, starts `teamagents daemon` detached and hands the socket to the TUI.
  Quitting the TUI does not stop the session.
- **Stopping that session**: the daemon the client started is *detached*, so there is no terminal to Ctrl-C;
  send it SIGTERM and it shuts down normally (freezes new dispatch, keeps committed state, removes its socket;
  DESIGN §9) — `ps -eo pid,args | grep "[t]eamagents daemon"` lists the daemons with their `--state-root`,
  then `kill <pid>` (D-150). A daemon you started by hand in the foreground also stops with Ctrl-C. The
  session database stays where it is and the next `teamagents`/`exec` on that state root starts a fresh daemon.
- **Headless use**: `teamagents exec [--json] [--timeout SEC] [--check CMD] "prompt"` goes through the same
  daemon and reports the goal's terminal state, the assistant reply or a timeout; the prompt may come from
  stdin (`-`). The full contract is in §1.1.
- **Authority** (D-61): `teamagents authority` lists the session's instances and grants, `teamagents authority
  grant --subject ID --action shell --scope workspace` hands one out and `teamagents authority revoke --grant
  ID` takes it back. This is how a worker the Leader spawned gets the shared-workspace shell (§3); the full
  contract is in §3.1.
- **A session owns its workspace and permission mode**: both are fixed when the daemon boots (`--cwd DIR`,
  `--full-auto`), so a client that joins a session already running keeps that session's settings and prints
  them (`note: a session is already running for this state root in … mode` / `that session works in …`).
  Starting a client with a different `--cwd` against a live session therefore does not move it — stop that
  daemon or use another `--state-root`. A `--cwd` that is not an *existing directory* is different: it can
  never be honoured (the session confines every file and shell command to it), so `exec`, the TUI and
  `daemon` refuse it outright with `--cwd … is not a directory` instead of booting a session whose workspace
  no tool can resolve.
- **State root**: `$XDG_STATE_HOME/teamagents/v2/` (default `~/.local/state/teamagents/v2`), where
  `session.sqlite` is the **single source of truth** (WAL with `synchronous=FULL`, carrying a format and
  version stamp). It is a *directory*: the session database and the daemon socket live inside it, so a
  `--state-root` naming a file (most often the `session.sqlite` itself) is refused with the flag named, by
  `doctor`, `init`, `daemon`, `exec` and the session verbs alike (D-166). A path that does not exist yet is
  not an error: it is created.
  If such a fresh path is *named* like the database (`…/session.sqlite`), `init` creates it as asked and prints
  a note, because the root is the directory that holds that file (D-169).

### 1.1 Headless runs (`teamagents exec`)

`exec` is a thin client of the same daemon the TUI uses; it submits one input to the leader and reports what
happened. Diagnostics go to stderr, the outcome to stdout (`--json` prints one JSON object with `end`,
`goal_status`/`reply`, `verification` and the event `watermark`).

| Exit code | Meaning |
|---|---|
| `0` | settled: the goal completed as `SUCCEEDED`, or the leader answered directly |
| `1` | not delivered: the goal settled otherwise, the turn failed permanently, the runtime **refused the request before it began** (a goal budget ceiling, a passed goal deadline — it says so in `failure`), the leader was **terminated while the run was in flight** (it says so in `failure` too), the instance was **reset** while the run was in flight (the epoch move is named), a `--check` command failed, or the run ended `unsettled` (the turn closed with nothing settled — the runtime's own closing word, never a reply) or `undelivered` (the input waited for a boundary and a context reset sealed it before it landed) |
| `3` | an approval is pending — a headless run does not wait for the deadline; decide it with `teamagents approvals` (§4) |
| `124` | the `--timeout` deadline passed with the instance still running (a refusal is not a timeout: it ends the run at once with `1`) |
| `2` | usage or infrastructure: no daemon, no model profile, a leader that is parked or paused |

- **Prompt**: the positional argument, or everything piped into stdin when it is `-`. An empty prompt is a
  usage error.
- **`--check COMMAND`** (repeatable): your own acceptance command, run after the turn ends in order in the
  isolated shell (bubblewrap) inside your workspace (`--cwd`, else the current directory). The first failure
  stops the list; the verdicts are printed, written to `<state root>/verification.json` and included in the
  `--json` report. A failed check makes the run fail (`1`) even when the goal itself settled. Checks are
  skipped when the turn is not finished — the run stopped for an approval, your own `--timeout` deadline
  passed, or the runtime refused the request before it began (a goal budget ceiling or a passed goal
  deadline) — because there is nothing to verify yet, and a check must not stretch the deadline you set: with
  no verdicts the report's `verification_path` is `null` and no ledger is written.
- **Both acceptance mechanisms can be configured at once**, and they answer different questions: the
  runtime's `[[checks]]` (§2.1) decide whether the goal may be reported done, your `--check` commands decide
  the exit code of a finished turn. So a run can exit `1` with the goal `SUCCEEDED` (your check failed) or
  with the goal `BLOCKED` while your check passed (measured: `python3 review/dogfood/two_gates.py`, D-101).
- **An input sent while a turn is running waits for that turn** (D-63): a model request is fixed once it is
  registered, so the input enters the conversation at the next boundary — after that turn's own answer — and
  gets a turn of its own. A headless run reports `input_queued` (and prints
  `queued: a turn was already running, so this input enters after it ends`) instead of pretending it landed;
  the message is never dropped. What such a run *reports* is its own outcome (D-72): the settlement or reply of
  the turn that follows its input, never the earlier turn's — and if a context reset seals the waiting input,
  the run ends `undelivered` (exit 1) instead of waiting out its deadline. The TUI says the same in its note
  line.
- **An approval in a headless run** exits `3` and names the exact call. Decide it with
  `teamagents approvals` / `approvals approve --id …` (D-67, §4.1) and the session goes on — the decision is
  bound to that operation and its arguments, so the run needs no second prompt; the TUI's approvals box shows
  the same call.
- **A parked or paused leader refuses new input** (`2`) instead of queueing work nobody drains: resume it with
  `teamagents instances resume --id i-leader` (§4.2) or in the TUI instances panel (`r`), or use a fresh state
  root.
- When `exec` starts the daemon itself, the daemon's output goes to `<state root>/daemon.log`; if the daemon
  exits while starting, the reason is reported immediately together with that path.

### 1.2 The JSON reports (`--json`)

Five verbs print one JSON object instead of human text, and that object is the scripting surface: `exec`,
`authority`, `approvals`, `instances` and `tasks`. The table is the contract — `review/exec_report.py` reads
the field names out of the source and out of this table and fails if the two drift apart in either direction.
The arrays inside a report (`instances`, `tasks`, `approvals`, `grants`) are rows of the daemon's own views,
catalogued with their fields in `docs/PROTOCOL.md`.

| Verb | Fields it adds |
|---|---|
| every report | `session_id`, `state_root` |
| `exec` | `permissions`, `session_workspace`, `instance_id`, `instance_lifecycle`, `end`, `goal_status`, `reply`, `failure`, `approval`, `input_queued`, `workspace`, `verification`, `verification_path`, `watermark` |
| `authority list` | `revision`, `instances`, `grants` |
| `authority grant` | `grant_id`, `revision`, `subject`, `action`, `resource_scope`, `parent_grant_id` |
| `authority revoke` | `grant_id`, `grant`, `revoked`, `revision` |
| `approvals list` | `approvals` |
| `approvals approve` / `deny` | `approval_id`, `decision`, `approval`, `result` |
| `instances list` | `instances` |
| `instances pause` / `resume` / `terminate` | `instance_id`, `lifecycle`, `instance`, `result` |
| `tasks list` | `tasks` |
| `tasks cancel` | `task_id`, `task`, `result` |

What the field names do not carry:

- `end` is the run's own word for how its turn ended — the `End` values behind §1.1's exit-code table;
  `goal_status` and `reply` are the goal's settlement and the member's answer, and a run reports only what its
  own input produced (D-72); `failure` is the runtime's reason when it refused or failed the turn.
- `verification` is the list of `--check` verdicts, each carrying the command, its ok flag, its exit code and
  its output; `verification_path` is the ledger written next to the session database (`null` when no check ran).
- `watermark` is the event cursor the report was built at, which is what a client resumes from after a
  reconnect (A28).
- `result` is the daemon's own reply to a command, and `instance`/`task`/`approval`/`grant` is the affected row
  as the daemon reported it.

A key this build does not serve is **refused at load with a pointer** (`unknown key … docs/CONFIG.md lists every key this build serves`) rather than ignored, so a typo fails loudly instead of silently leaving its setting at the default (D-161).

## 2. Configuration

Every key this build reads, with its type, what it holds when absent and the files that consume it, is in
[docs/CONFIG.md](CONFIG.md) — generated from the structs and kept in sync by `make hygiene`; `doctor`
reports what a session resolved (including the keys that are accepted but not applied).

The user config is `$XDG_CONFIG_HOME/teamagents/config.toml` (default
`~/.config/teamagents/config.toml`). Credentials are referenced by environment-variable name and never
written into the file:

A repository-local `<cwd>/.teamagents/config.toml` is **not read by the current entry points**: the merge
loader (`load_user_config_for`) with its trust rules is implemented and unit-tested, but nothing wires it into
the daemon yet, so cloning a repository cannot change a session today. Until that lands, everything below is
the *user* config.

```toml
[models.leader_main]
provider = "deepseek"
protocol = "deepseek"        # deepseek | chat/completions | responses | anthropic | openai (or absent:
                             # chat/completions, with the base URL following provider)
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000     # native window; leave empty when unknown (never guess a small value, D-36)

[tools.web]                  # optional tool binding (web / fetch / mcp)
kind = "web_search"
provider = "anysearch"
url = "https://api.anysearch.com/v1/search"
api_key_env = "ANYSEARCH_API_KEY"
```

- `context_window` drives the request budget and the compaction threshold; real models must use their native
  window, and the value and its source are recorded (D-36 in `docs/DECISIONS.md`).
- `teamagents doctor` checks the config item by item, that every model profile's credential resolves, the
  state root (stamp, WAL, read/write), the bubblewrap isolation probe and that the programs named in
  `[hooks]` are executable; it also lists the `[[checks]]` that will gate every goal. An `sessions/` layout
  from an earlier release is reported explicitly and is never migrated.
- A `[tools.*]` binding with `kind = "web_search"` needs a credential: `api_key_env` names the environment
  variable it authenticates with. Without it — not configured, or the named variable unset — a search call
  answers with a **capability state** naming what to configure, and no request leaves the host unauthenticated
  (DESIGN §7, D-167). `kind = "web_fetch"` needs no credential. A binding with `required = true` that cannot
  work (missing credential, unserved `provider`) refuses the member's start, so `doctor` reports its row as a
  **FAIL** and the parked instance carries the reason (D-164/D-165); an optional one keeps the capability and
  the call answers with that state.
  The tools themselves appear only for the kinds you declare (§12.1: binding is the authorization, D-168): a
  config with no web entry offers the model neither `web_search` nor `web_fetch` — `doctor` says so, and adding
  a `[tools.fetch]` entry makes exactly `web_fetch` appear.

### 2.1 Acceptance checks (`[[checks]]`)

These are the machine contracts a goal must satisfy before it can be reported as done (§8, Q11). They come
**only from your own config** — never from a cloned project, because a check is a command that later runs
without an approval prompt:

```toml
[[checks]]
id = "tests"                     # stable id: appears in failures and repair feedback
command = "cargo test --offline" # run through the same shell tool the model uses
inputs = ["src", "Cargo.toml"]   # optional: hashes bind the result to these inputs (A17)
timeout = 900                    # optional seconds (default: the shell tool's own)
network = false                  # optional: the sandbox is offline by default
```

- The runtime runs them in the isolated shell at the completion boundary, so a goal cannot settle while a
  check fails. A failure returns the work for repair (bounded rounds) and then blocks the goal with the
  failing ids; the check's output lands in the conversation as a tool result.
- `inputs` are workspace-relative paths without `..` escapes. Their hashes are taken when the check runs and
  re-verified before completion, so a check that passed against files that then changed does not count — and a
  check whose own command rewrites its declared input blocks the goal (`bound:stale_inputs`) instead of
  passing. Both halves are measured live (`python3 review/dogfood/stale_check.py`, D-90).
- Checks are not asked through `pre_tool` (you already pre-authorized exactly these commands) and they skip
  the approval gate for the same reason.
- The TUI reports each round by name (`completion check round 1 started: tests, docs`), the failure
  (`completion check round 1 failed: tests (exit), entering a repair turn`) and the final block reason, so a
  goal that is being repaired or parked explains itself instead of looking stuck.
- A broken entry (empty `command`, `timeout = 0`, an escaping `input`) is refused when the config loads, so
  `doctor` and every entry point report it instead of a goal silently never settling.

### 2.2 Goal limits (`[limits]`)

Every goal this session creates can be bounded in cost and in time (D-64):

```toml
[limits]
max_total_tokens = 2000000   # optional: usage ceiling (provider-reported + unknown usage)
deadline_minutes = 45        # optional: wall clock, counted from the moment the goal is created
```

- `max_total_tokens` is the goal's own ceiling: a new request is refused when the settled usage plus the live
  reservations plus the request's estimate would pass it (§8/A18), and the instance parks with the budget as
  the reason instead of overspending. Usage is settled honestly, so a provider that reports later than it
  bills can still overshoot the ceiling afterwards — that is the design's choice, not a bug.
- `deadline_minutes` is turned into the goal's absolute deadline when the session creates it; past that
  moment no new request starts and the instance parks with the deadline as the reason (A35).
- With neither configured a goal — and therefore the session — runs until you stop it, which is why `doctor`
  says so out loud (`goal limits: none: …`). A zero is a config error at load time.
- Both come from **your** config: like `[[checks]]`, a cloned project cannot set them. They are fixed when
  the goal is created; changing them means starting a new session (or a new state root).
- The TUI's status line shows what is in force (`goal ACTIVE · usage 12000/2000000 · ends in 30m`), so a
  configured ceiling is visible and not just enforced.

### 2.3 Hooks (`[hooks]`)

Hooks are **your own programs** (their paths come only from the user config; a model cannot choose them) and
run on the host with your permissions:

```toml
[hooks]
notify = ["/home/you/bin/teamagent-notify.sh"]    # notifications: argv[1] is the event name, JSON on stdin
pre_tool = ["/home/you/bin/policy.sh"]            # policy hook before tool calls
```

- `notify` runs asynchronously and never blocks a turn; it is killed after 10 seconds and failures only reach
  the engine's stderr. The events are `tool_call` (with `tool`/`arguments`/`ok`/`error` — a call your policy
  denied arrives with `ok: false`), `team_action`, `run_completed` and `run_failed`/`run_cancelled` (each
  names its `request_id`, so they fire per **model request**, not per turn), and `run_paused` (the instance
  entered PAUSED).
- `pre_tool` runs synchronously before every native tool call (files/Shell/web/Skills/MCP). Exit code 0
  allows it; **exit code 2 denies it** and the first stderr line becomes the reason handed to the model;
  any other exit code, a spawn failure or a timeout allows the call and logs to stderr — a broken hook never
  stalls the team. Replays after crash recovery are not asked again (the decision was made at first
  dispatch), and the required checks of §2.1 are your own acceptance commands, so they skip `pre_tool`.

## 3. Sessions and teams

- The daemon owns the session: one JSON-lines protocol (`/v1`) over a Unix socket, with the TUI and `exec`
  as thin clients. A reconnect resumes events from the last watermark and commands deduplicate by
  `command_id`. The event log is a documented surface: every kind
  this build emits, with its scope, payload and emitting site, is listed in [docs/EVENTS.md](EVENTS.md)
  (generated from the code and kept in sync by `make hygiene`). The requests and replies themselves
  (framing, the greeting, `command_id` dedup, every read method and command) are catalogued in
  [docs/PROTOCOL.md](PROTOCOL.md), also generated from the dispatchers.
- **The Leader builds the team**: it uses `spawn` to create working instances, `delegate` to hand out tasks,
  `send` for messages and `wait` for results. These appear in the model-visible tool surface according to
  its grants (`manage`/`delegate`/`message`), and the permission revision is re-checked at dispatch. The
  session's bootstrap grants the Leader those three capabilities over the session, so the team tools are there
  without further setup; revoking a grant removes the tool from the surface and makes every call fail closed.
  `spawn` also takes `model` (a catalog key or the model name it declares) so a team can mix entries: an unknown
  entry fails that call with the available keys, and an instance that cannot boot at all is parked with the
  reason (its task stays open for you to cancel). A tool the instance cannot dispatch is not offered at all: a
  worker the Leader spawned holds `shell` only after you grant it `shell@workspace` (§5.1's boundary, D-60), so
  it works with the file/web/skill tools (they need their binding, not a grant) until then.
- **Who runs on what**: every member's model is recorded when it is created and shown in the instances panel
  and in `teamagents instances` (`i-worker · ACTIVE · READY · k3-256k`), which is what makes a team spanning
  two providers (`spawn` with `model = "…"`) readable instead of guesswork (D-69).
- **Workspace policies**: the `workspace` argument of `spawn` decides where a new instance works — `shared`
  (default: the project directory), `isolated` (a private directory under the session state root) or
  `git_worktree` (its own branch and worktree). Asking for a worktree in a project that is not a Git
  repository or has uncommitted changes falls back to shared mode and says why in the tool receipt.
  Terminating an instance retires its workspace from the record, and **a directory with uncommitted or
  unmerged work is never deleted automatically** — only the reason is reported.
  A worktree member's branch is **not merged for you**: the branch name and base commit are recorded in
  `<state root>/instances/<id>/worktree.json` (and `git worktree list` shows the checkout), so the merge is
  yours — or the Leader's, if it holds `shell@workspace` and runs `git merge` in the project. Once the branch
  is merged, the running session retires the worktree by itself on its next pass (the record goes with it);
  until then the refusal is reported **once per reason** in `daemon.log`, and the checkout is kept.
- A member whose model ends its turn with plain text (no tool call) and does not settle its task goes
  **idle with the task still `RUNNING`** (D-65: the runtime never reads an outcome out of prose, §8, and it
  never asks the same question twice — that was a turn storm). The delegator's wait stays pending, so
  cancelling the task (`c` in the tasks panel) is what releases it: a cancelled task satisfies the wait and
  the delegator wakes to re-delegate or settle honestly. A `BLOCKED` task does *not* satisfy it.
- User-side intervention: switch instances, pause/resume/cancel and approve or deny tool requests in the
  TUI — or headlessly with `teamagents instances` / `teamagents tasks` (§4.2), which is what a script or a CI
  job can use. Budget, task and grant panels all read the same facts.
- Goal and task completion goes through the runtime's completion gate: `finish` only accepts honest
  outcomes, and the required checks you define must really pass.
- **Required checks** come from `[[checks]]` in your config (§2.1) and are carried on the goal itself
  (`create_goal limits.required_checks`, which only the user or the project bootstrap may predefine). The
  runtime runs them in the isolated shell at the completion boundary: a failure sends the work into a bounded
  repair loop and then blocks the goal with the failing ids. Their output appears in the conversation like any
  other tool result, so the reason is visible instead of reported as an unexplained "done".
- The headless client's `--check` (§1.1) is a *different* thing: it is your own acceptance command, executed
  by the client after the turn ends, and it only decides that `exec` exits non-zero. A runtime check (above)
  is the stronger contract, because the goal itself cannot settle until it passes.

### 3.1 Authority: who may do what (`teamagents authority`, D-61)

A team is a set of agents with different capabilities, and **you** decide which ones they have. The unit is a
grant: a subject (an instance), an action and the resource it applies to. The session's bootstrap gives the
Leader the three capabilities its own team tools need (`manage`, `delegate`, `message`, D-58) and the
workspace shell; a worker the Leader spawns holds **none** of them (§5.1), which is deliberate — an agent the
team created does not inherit the user's reach. A worker therefore cannot run shell commands until you grant
it `shell@workspace`; it uses the file, web and skill tools (they need their binding, not a grant) until then.

```bash
teamagents authority                                                  # instances + grants, with the ids
teamagents authority grant --subject i-worker-1 --action shell --scope workspace
teamagents authority revoke --grant g-1a2b3c4d        # the full id or an unambiguous prefix
```

The action vocabulary is exactly what some check consults — nothing else can authorize anything:

| Action | Asked over | Meaning |
|---|---|---|
| `shell` | `workspace` | run commands in the shared project directory |
| `manage` | `session` or `instance:<id>` | spawn, and reset/park an instance |
| `message` | `instance:<id>` | send a message to that instance |
| `delegate` | `instance:<id>` | hand a task to that instance |
| `task_result` | `task:<id>` | settle that delegated task back |

`session` is the widest scope: it covers every resource below it, so `--scope session` grants the capability
everywhere in this session, while `--scope instance:i-worker-1` grants it for that peer only. `--parent G`
makes the new grant *derived*: it is then covered by `G` and is revoked with it (a snapshot of the grant tree
rather than a second, independent authority).

What the surface refuses and why:

- **an action outside the table** (`--action shel`) — a typo would create a row nothing reads;
- **a pair no check asks about** (`shell@instance:i-worker-1`): it would be dispatched never, offered never and
  refused never, so the surface says what `shell` *is* asked over instead of writing a grant that looks like
  power and does nothing;
- **a subject that does not exist** is only a *warning*: instance ids are chosen by the spawner, so granting
  ahead of a spawn is legitimate — and a typo is caught the same way, by the note on stderr.

Revocation is final and takes the derived subtree with it. Because every dispatch re-checks the live grants at
its own linearization point (§6.1), a revocation also stops an operation that was authorized when it was
queued, and it is never rewritten into a silent success: the model sees the refusal, and the tool leaves its
surface at its next request.

Exit codes are `0` done, `1` the session refused the command (for example a `--parent` it does not know) and
`2` usage or no session to talk to; `--json` prints the raw report. `authority` is a client of the running
session — it never opens the database — so start the session first (`teamagents`, or any `exec` run) and use
`--state-root PATH` to point at another one. The TUI topology panel shows the same facts (subject, action,
scope and the short id); issuing and revoking grants is the CLI's job.

## 4. Permissions and isolation

| Mode | Behaviour |
|---|---|
| `approved_scope` (default) | Shell runs under bubblewrap; network access and out-of-scope writes need user approval, bound to the concrete operation and its argument hash |
| `full_auto` | Host shell (D-41): long commands and background services survive across calls, and `exec` exiting does not stop an already started service |

A service a command leaves behind (`dev-server &`) belongs to **you**, not to the session: the product does not
adopt it, does not restart it and does not clean it up when the client, the daemon or the member exits — that is
the deliberate half of D-41, and the runner that owned the command's process group is retired once the job
settles (D-112). Stop such a service the way you would from a shell: have the command print its pid (`sleep 300 &
echo $!`) and `kill` it, or keep the command short-lived. What the session *does* stop is the command itself —
`instances terminate` cancels an operation that is still running, and its receipt says `class: cancelled`.

The mode belongs to the **session**, not to the client, and you choose it in two places: the flag
(`teamagents --full-auto`, `teamagents --full-auto --state-root …`, `teamagents daemon --full-auto`) asks for host
execution for that boot, and `mode` in `[permissions]` of **your own** config makes it the default for the
sessions you start:

```toml
[permissions]
mode = "full_auto"            # or "approved_scope" (the default); the flag still wins
```

A project file can never set the mode (only your user config is read for it), and nothing a model says changes
it. A client that
finds a session already running keeps that session's mode and prints which one it is, so
`teamagents exec --full-auto` against a live `approved_scope` session reports
`a session is already running for this state root in approved_scope mode` instead of silently ignoring the
flag. To switch
modes, stop that daemon (§1: SIGTERM to its pid, or Ctrl-C where you started it by hand) or use another `--state-root`.

When bubblewrap is unavailable this is a **classified failure** (`started=false`); the command never falls
back silently to host execution.

### 4.1 Approving a call from the CLI (D-67)

In `approved_scope` (the default) an out-of-scope call parks on **your** decision, and the operation is bound
to it — the hash of the exact command or arguments is part of it, so a modified call needs a new decision
(§6.2).

```bash
teamagents approvals                                   # id, tool and the exact call, per pending approval
teamagents approvals approve --id ap-d-req-4caeb0c6    # once: the parked turn continues with the result
teamagents approvals deny --id ap-d-req-4caeb0c6       # fails closed; the model is told you denied it
```

In the TUI the same decision is one key: a pending approval appears in the approvals box (the status
line and the footer count it), **`Ctrl+A`** focuses that box, `↑`/`↓` pick an entry, **`a`** approves and
**`d`** denies, `Esc` returns to the composer. Measured with a real model (`python3 review/dogfood/approval.py`,
D-89): approving runs the call (its artifact appears), the box drops the id and nothing stays pending;
denying never runs it, the operation lands `CANCELLED` with a receipt whose class is `denied`, and the model
is told.

An id must name a *pending* approval (a typo is refused before anything is decided), the full id or an
unambiguous prefix is enough, and `--json` prints the raw report. Exit codes match `teamagents authority`:
`0` done, `1` the session refused it, `2` usage or no session. This is also what makes `teamagents exec` usable
in a script: a run that exits `3` is not a failure, it is a question — answer it and the session continues.

### 4.2 Pausing, resuming and cancelling from the CLI (D-68)

The TUI's instance and task actions have headless equivalents, so the recovery paths in the operating notes
work without a terminal UI:

```bash
teamagents instances                    # id, lifecycle, phase — the same rows the TUI panel shows
teamagents instances resume --id i-leader      # a parked instance runs again (budget, an unusable model, …)
teamagents instances pause  --id i-worker-1    # stop driving it at the next safe boundary
teamagents instances terminate --id i-worker-1 --yes   # deliberate: retires it and its workspace
teamagents tasks                        # id, status, assignee, goal
teamagents tasks cancel --id t-prose    # releases a delegator waiting on a task that can only wait
```

- **`tasks cancel` is the lever for a stuck delegation**: if a member's model ends its turn without settling
  its task (D-65), the task stays `RUNNING` and the delegator waits; cancelling it **satisfies** that wait
  (a `BLOCKED` task would not), so the delegator wakes and can re-delegate or settle honestly.
- `terminate` needs `--yes`: it retires the instance, its open work is dealt with explicitly, and a workspace
  holding uncommitted or unmerged work is never deleted — the reason is reported instead. Retiring an instance
  while a run is waiting ends that run at once (`1`, with the reason); **pausing** it does not: a pause stops
  the instance at a boundary, the run keeps following its turn, and resuming lets it finish (§4.2, D-98).
  If a run does hit its deadline on a stopped instance, the report names the lifecycle instead of claiming the
  instance is still running.
- **Stopping a command that is already running**: `tasks cancel` is *delegation-level* — the task lands
  `CANCELLED` and the delegator is released, but the assignee's command keeps running until its own tool
  timeout, because the process-group stop (§6.4) is a request against an *operation* and the assignee learns
  about the cancel at its next boundary. The lever that stops the process group is
  **`instances terminate --id … --yes`**: it closes the instance's open execution, the running operation is
  flagged, the runner kills the process group, and the operation lands `CANCELLED` with a receipt whose
  `class` is `cancelled`. Measured with a real model and a real command (`python3
  review/dogfood/cancel.py`, D-88): the effect stops 1.5–6 s after the lever. There is no lighter lever for
  one command today.
- A **reset** has no CLI verb: it is the `reset_instance` command of the session protocol (§9), and it closes
  the epoch's execution — a run waiting on a turn in the old epoch ends at once with exit `1` and a `failure`
  naming the epoch move (D-100).
- Ids must name a listed instance or task (full id or an unambiguous prefix); `--json` prints the raw report
  for scripts. Exit codes: `0` done, `1` the session refused it, `2` usage or no session.

## 5. Skills and MCP

- Skills live under the registration root `~/.agents/skills` (searched and read on demand with
  `skill search/read`; a skill's instructions can never widen execution permissions). `skills_paths` and
  `instruction_files` accept `~/…`, and **`doctor` reports what they resolve to** (`skills  3 skill(s) under
  1 configured root(s)`, or a WARN naming a root that does not exist) — a path that is not there is ignored,
  so without that row a typo would look like "no skills" (D-66). **`instruction_files` itself does not change
  a prompt in this release**: the files are validated and reported, but nothing reads their contents into a
  member's system text, and `doctor` says `1 declared, not applied` rather than promising a prompt (D-102).
  Wire it if you need it: a member's instructions come from its profile, so the Leader can pass the rules in
  its `spawn`/`delegate` text today.
- The tools themselves — every function a model may be offered, with its parameters and the model-facing
  description, split by the layer that decides it (the profile's tools, the instance's grants, the session's
  bindings) — are catalogued in [docs/TOOLS.md](TOOLS.md), generated from the schemas.
- MCP: bound services load at startup (a required service fails loudly, an optional one only drops its
  capability). "Fails loudly" means: the instance whose driver cannot boot is **parked with the runtime's
  reason** — a `bearer_token_env_var` that is not set, an unreachable endpoint or a missing command. The
  session keeps running, but **nothing drives that instance**: a `required = true` service that cannot start
  leaves every member unstarted, so a headless run (D-164) ends on the park instead of waiting out its
  deadline and reports the runtime's own words (`failure`, e.g. `required tool service "x" is unavailable:
  …`), the TUI writes the same sentence into its system notes, and `doctor` names a *static* cause before any
  session starts (a command that is missing or not executable, `${VAR}` in a command — nothing expands one —
  an `env` value naming an unset variable, an unset bearer variable). Fix the cause and pull the lever the
  client names (`teamagents instances resume --id …`, or `r` in the TUI instances panel): `teamagents instances`
  and the TUI's instances panel both show the park **and its reason** (D-165), so the answer is on the surface a
  user looks at first. Measured live over the HTTP
  transport (`python3 review/dogfood/mcp_http.py`, D-104). Calls go through the same permission, approval, budget, cancellation and receipt entry
  points. A remote call dispatched before a crash is recorded as `OUTCOME_UNKNOWN` after recovery and is
  **never replayed**.
- **Binding one is writing it down** (D-74): a `[tools.<name>] kind = "mcp"` entry in your config *is* the
  binding, exactly like `[tools.web]` for the web tools — the service starts with the session and its tools
  reach every member's model surface as `<name>_<tool>`. Only your **user** config is read today (a
  repository-local project file is not — §2), so a cloned project cannot bind a service at all yet; when the
  project loader is wired, its `tools` will need `[permissions] trust_project_tools = true`. `required = true` means
  the session must not start without it; `doctor` lists each declared service and whether its command can run,
  so a typo shows up there instead of in `daemon.log`.
- **Where a stdio server runs** is your choice, and the default is the safe one (D-106):
  `mcp_execution = "workspace"` starts the server inside bubblewrap, with the member's workspace as its working
  directory — it cannot read or write host paths outside that workspace, its `HOME` is a private directory, and
  it has no network at all unless you also set `mcp_network = true` (a server that must call an API needs that
  switch; the default keeps a package-manager or config-fetching server from reaching the network by itself).
  `mcp_execution = "host"` runs the server as yourself, with your whole filesystem and network: only for a server
  you trust, and the same choice a shell makes when the session runs `--full-auto` (D-41). Without bubblewrap the
  workspace mode **refuses to start** — the instance parks naming the isolation — rather than quietly running the
  server on the host.
- A server that stops answering is bounded the same way (D-108). `startup_timeout_s` (default 60) covers the
  handshake: if `initialize` does not answer inside it, the boot fails, the instance parks with
  `MCP … initialization failed: MCP initialize timed out`, and the server is killed instead of left behind.
  `tool_timeout_s` (default 120) covers one `tools/call`: a call that does not answer inside it is reported to
  the model as a timeout — the turn continues and the model can retry or work around it — rather than blocking
  the member until the server wakes up.

## 6. Recovery, compaction and cleanup

- Crash recovery is classified by persisted location: known results are reused, in-flight losses are
  recorded honestly (`OUTCOME_UNKNOWN`) and nothing is replayed on a guess. When the disk is full, new
  side-effect dispatch stops and the instance parks, reporting the in-flight loss (A31).
- Long-context compaction triggers on **real window usage**: the summary keeps the original request, user
  revisions, acceptance criteria and open questions, while the full text stays retrievable through
  `read_history`. Compaction calls count against the goal budget.
- Data cleanup is a one-off migration, not a lever this build has: the state of earlier releases (≤ v0.1.2)
  is **not migrated**, and that old state — sessions, preferences and caches — was removed deliberately,
  against a written inventory taken first (the upgrade notes in `docs/ACCEPTANCE.md` record it). Nothing in
  this build deletes your data on its own, and credentials, `~/.agents/skills`, `~/.codex` and the evidence
  under `review/` are always kept.

## 7. Troubleshooting

| Symptom | What to do |
|---|---|
| `exec: connect ... Connection refused` | Run `teamagents doctor` to inspect the state root; the next `teamagents`/`exec` starts the daemon automatically |
| `the daemon exited while starting (exit status: 1): …` | The reason is the daemon's own first words; the full log is `<state root>/daemon.log` (usually a missing/broken config or an unset credential) |
| `exec: the leader instance i-leader is PARKED …` | The leader stopped on a permanent failure and nothing drives it. The client reports the park as soon as it sees it, with the runtime's own reason (D-164; the same sentence is the TUI's system note and a `daemon.log` line): exit 1 when the input was already submitted (it is waiting — resuming the instance runs it) or exit 2 with "nothing was submitted" when the park was visible before submitting. Fix the cause, then `teamagents instances resume --id i-leader` (or `r` in the TUI instances panel), or start a fresh state root. A **TERMINATED** leader is different: termination is final, so `exec` says so and the only way on is a fresh state root (D-82) |
| `exec` reports `check 1: FAILED` | Your own `--check` command failed; its output is on stderr and in `<state root>/verification.json` |
| `exec` exits 3 | A tool call needs approval and a headless run cannot answer it. Approve it in the TUI and run `exec` again, or start the daemon with `--full-auto` |
| `doctor` reports the state root as FAIL | That path does not hold a current session database (the stamp does not match); use another `--state-root` or follow the message, and never edit the database by hand |
| The agent worked in the wrong directory | Its session was started with another workspace (or without `--cwd`): the client prints the live one. Stop that daemon (§1: SIGTERM to its pid) or start a fresh `--state-root` with `--cwd DIR` |
| The model returns 401/402 | Check the environment variable named by the profile's `api_key_env`; `doctor` lists the credential resolution result per profile |
| A task stays `RUNNING` while its assignee is idle | The assignee's model ended its turn without settling it (D-65): cancel the task — `c` in the tasks panel or `teamagents tasks cancel --id` — which releases the delegator's wait |
| A member is stuck in a long or endless command and cancelling its task changed nothing | `tasks cancel` is delegation-level and does not touch the assignee's operation (D-88). Stop the work with `teamagents instances terminate --id … --yes` (the process group dies within seconds, and the receipt says `class: cancelled`) or wait for the command's own tool timeout |
| An instance is parked | `teamagents instances` shows which; resume it with `instances resume --id` (or `r` in the TUI) when the reason is gone |
| A command under `approved_scope` waits for approval | Decide it with `teamagents approvals` (§4.1) or in the TUI approvals panel; `--full-auto` (host execution, D-41) skips the gate |
| A command left a service running (`dev-server &`) | By design the session does not manage it (D-41/D-112): it survives the client, the daemon and the member. Stop it yourself with the pid the command printed (`sleep 300 & echo $!`, then `kill <pid>`), or start such work in a command that exits when you are done. `instances terminate` stops an *operation that is still running*, not a service left behind by one that finished |
| A member says it cannot run shell commands although you granted it | Turn the offer into a witness: start the session with `TEAMAGENTS_LOG_SURFACE=1` and the driver writes one line per request into `<state root>/daemon.log` — `driver: surface <instance> shell=yes|no tools=…`. `shell=no` after a live `authority grant` is a bug worth reporting; `shell=yes` with no attempt means the model had the tool and did not use it (§3.1, D-143/D-157) |
| `exec: the daemon's socket was lost …` | The daemon stopped while this run was waiting — by your own `kill` (§1) or because it crashed. The session and its committed state are kept: start it again (`teamagents` or `exec`) and the work is where it was (D-150) |
| A `teamagents jobs-runner` process is still in `ps` | By design a runner outlives the session that started it: it is the live partner for a job whose outcome is unknown (§6.2). It costs no measurable CPU while it waits (D-153) and exits by itself once its job directory is gone; opening the session again (`teamagents`/`exec` on that state root) imports the receipt and retires it (D-112) |
| Start completely fresh | Stop the daemon (§1: SIGTERM to its pid) and run `teamagents --state-root <new directory>` for a clean session; the old database stays where it is |

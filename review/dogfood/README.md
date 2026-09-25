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
`k3-256k`, `task t1 SUCCEEDED`.

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
python3 review/dogfood/authority.py                     # fresh /tmp state root
python3 review/dogfood/authority.py --state-dir /tmp/ta-authority
```

1. the Leader is asked to spawn one worker and delegate a shell command (`printf granted > proof.txt` in the
   shared workspace) — a spawned worker holds no `shell@workspace` (§5.1), so the command cannot run and
   `proof.txt` must **not** exist;
2. `teamagents authority` reads the session (instances + grants with their ids) and `authority grant` gives
   that worker `shell@workspace`;
3. a second instruction asks the same worker to run the command again — now `proof.txt` must exist with the
   expected content;
4. `authority revoke` takes the capability back and the probe asserts no live shell grant is left.

It is a real-model check (same credential and native-window rules as `run.py`) and it is the probe that found
**D-62**: after the worker's first turn ended on an accepted `finish`, its next request was rejected by the
provider with `HTTP 400 ... must be followed by tool messages responding to each 'tool_call_id'` — the wire
projection now answers every call it carries. Everything it writes stays under `--state-dir`.

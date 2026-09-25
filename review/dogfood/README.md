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

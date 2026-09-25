# Evaluation

## Current entry point: fixed-task A/B/C comparison (real models)

```bash
python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/<new date>
python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/<new date>
```

- Three groups: A = the direct reference loop (`engine/examples/eval_group_a.rs`), B = the persistent single
  instance (`engine/examples/eval_group_b.rs`), C = B plus the collaboration surface. The shared catalog key,
  permissions and timeouts are frozen before a run in [`r2-p6/design.md`](r2-p6/design.md).
- Every trial gets a fresh working directory and state directory; results go to `runs/<date>/results.jsonl`
  and each trial's session database and artifacts stay in `runs/<date>/{state,work}/`. Compile caches and
  SQLite temporaries of those directories are never committed (`.gitignore` plus `make hygiene` enforce it).
- Always use the model's native context length and record the value and its source; conclusions follow the
  pre-registered criteria only, and too few samples means "not confirmed".
- Re-run commands, costs and limits are in each run's `runs/<date>/REPORT.md`; the latest one is
  [`r2-p6/REPORT.md`](r2-p6/REPORT.md).

## Evaluation discipline

- No report may contain a model, command or metric that was not actually run; fake services and the subject
  under test are recorded separately.
- Grading runs inside the trial's own working directory from `checks.txt`, failures are classified rather than
  cherry-picked, and a trial is never re-run just to look better.
- Existing evidence is never cleaned automatically; during a run the inputs, grading and candidates stay
  untouched, and a historical failure is never re-recorded as a success because storage moved.

Earlier fixed-task runners, their task sets, graders and hidden-test fixtures left the tree with the rest of
the previous material; their raw results are reachable through Git history (`git log -- review/eval`).

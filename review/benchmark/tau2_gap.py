#!/usr/bin/env python3
"""Read a τ³-bench run's trajectories and say *how* the database ended up wrong (D-420).

The reward is the environment's DB check; this script answers the product question behind it, per dialogue:

* how many calls the domain tools served, and how many of them the environment **answered with an error**;
* whether the agent **retried after an error** or kept going (the failure mode D-420 recorded: an order id the
  environment does not know was used for the rest of the dialogue, including the call that was supposed to change
  the database, and the user was told the change was confirmed);
* the task's **reference action count**, for scale (the reference is one path to an equivalent end state, not a
  per-call requirement — `docs/evaluation.md`).

Usage: `python3 review/benchmark/tau2_gap.py /tmp/tau3-retail10`
It reads only local run directories, so it costs nothing and can run while another benchmark is going.
"""

from __future__ import annotations

import json
import pathlib
import sys


def analyse(run: pathlib.Path) -> dict:
    calls = json.loads((run / "calls.json").read_text()) if (run / "calls.json").exists() else []
    reward = json.loads((run / "reward.json").read_text()) if (run / "reward.json").exists() else {}
    # A call recorded before the bridge logged responses has **no receipt** for this question: without the
    # `response` key the answer is unknown, not "no error" — the same distinction D-419's runs taught one
    # instrument earlier. Runs with no receipts are reported and excluded from the conclusion.
    with_receipt = [c for c in calls if "response" in c]
    receipt = bool(calls) and len(with_receipt) == len(calls)
    errors = [c for c in with_receipt if c.get("isError") or str(c.get("response", "")).startswith("Error")]
    # "kept going after an error": an error at call i and a later call whose name repeats a failed one, or any
    # call after an error that is not a retry of the failed name
    continued = 0
    for index, call in enumerate(with_receipt):
        if not (call.get("isError") or str(call.get("response", "")).startswith("Error")):
            continue
        later = with_receipt[index + 1:]
        if later and all(other["name"] != call["name"] for other in later):
            continued += 1
    return {
        "task": run.name,
        "reward": reward.get("reward"),
        "receipt": receipt,
        "calls": len(calls),
        "errors": len(errors),
        "errors_then_moved_on": continued,
        "tools": sorted({c["name"] for c in calls}),
        "first_error": (errors[0]["response"][:90] if errors else None),
    }


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    rows = [analyse(path) for path in sorted(pathlib.Path(sys.argv[1]).iterdir()) if path.is_dir()]
    print(f"{'task':6} {'reward':>7} {'calls':>6} {'errors':>7} {'moved on':>9}  first error")
    for row in rows:
        print(f"{row['task']:6} {str(row['reward']):>7} {row['calls']:>6} {row['errors']:>7} "
              f"{row['errors_then_moved_on']:>9}  {row['first_error'] or ''}")
    receipts = [row for row in rows if row["receipt"]]
    errors = sum(row["errors"] for row in receipts)
    if rows and not receipts:
        print("\nNO RECEIPT: these runs predate the bridge logging responses, so 'errors' is unknown here, not "
              "zero — re-run them (review/benchmark/README.md) before reading anything into the column.")
    elif receipts and errors == 0:
        print(f"\n{len(receipts)} dialogue(s) with a receipt and no tool error: on this sample the DB failures "
              "are not 'ignored an error' but 'right calls, wrong outcome' — the two need different answers.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

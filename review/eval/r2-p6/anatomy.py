#!/usr/bin/env python3
"""Where a delegated trial's model requests actually go (D-256), read from a recorded batch.

    python3 review/eval/r2-p6/anatomy.py --runs review/eval/r2-p6/runs/2026-09-27-r4-pilot
    python3 review/eval/r2-p6/anatomy.py --trial multi-step.D.1 --runs DIR
    python3 review/eval/r2-p6/anatomy.py --self-check

Why it exists: the round-4 pilot spent **20-21 model requests** in group D against **7** in group B on the same
two-file tasks, and its wall clock followed the requests. The checkpoints each trial commits carry
`context_entries`, so the sequence of tool calls per instance is readable after the fact: this script classifies
every request by what it did and prints the leader's and the members' splits. What it was written to answer
(D-256): a spawned member holds no `shell@workspace` (section 5.1), so it cannot run its task's acceptance check
and works by inspection alone, and the leader then re-runs every check itself - the `execute` row per instance is
what shows that.

Ceiling: the buckets come from tool *names*, so a `read_file` that re-verifies is indistinguishable from one
that explores, and a trial whose checkpoint pruned `completion_json` is still readable here because the context
entries are what this reads. The classification describes the harness's groups, not a claim about intent.
"""
import argparse
import json
import pathlib
import sqlite3
import sys

REPO = pathlib.Path(__file__).resolve().parents[3]
DEFAULT_RUNS = REPO / "review/eval/r2-p6/runs/2026-09-27-r4-pilot"

# Tool name -> the bucket its request falls in. The collaboration vocabulary is separate on purpose: spawning
# and delegating is the leader's overhead, and `wait` parks the instance without a model call of its own.
BUCKETS = {
    "ls": "inspect", "glob": "inspect", "grep": "inspect", "read_file": "inspect", "read_history": "inspect",
    "write_file": "edit", "edit_file": "edit", "delete": "edit",
    "shell": "execute", "run": "execute",
    "spawn": "orchestrate", "delegate": "orchestrate", "send": "orchestrate",
    "finish": "settle",
}
ORDER = ["inspect", "edit", "execute", "orchestrate", "settle", "wait", "other"]


def buckets_of(message: dict) -> list:
    """The buckets one assistant entry's tool calls fall in (`wait` is its own, and is not a tool call)."""
    out = []
    for call in message.get("tool_calls") or []:
        name = (call.get("function") or {}).get("name") or ""
        out.append(BUCKETS.get(name, "other"))
    if message.get("wait"):
        out.append("wait")
    return out


def empty_slot() -> dict:
    return {"requests": 0, "model_ms": 0, "buckets": {b: 0 for b in ORDER}, "entries": 0}


def trial_anatomy(db: pathlib.Path) -> dict:
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    instances = {}
    for (instance_id,) in conn.execute("SELECT id FROM instances ORDER BY id"):
        instances[instance_id] = empty_slot()
    for instance_id, count, ms in conn.execute(
        """SELECT mr.instance_id, COUNT(*), COALESCE(SUM(a.elapsed_ms), 0)
           FROM model_requests mr LEFT JOIN attempts a ON a.request_id = mr.request_id
           GROUP BY mr.instance_id"""
    ):
        slot = instances.setdefault(instance_id, empty_slot())
        slot["requests"] = count
        slot["model_ms"] = ms
    for instance_id, message_json in conn.execute(
        """SELECT instance_id, message_json FROM context_entries
           WHERE kind = 'assistant' ORDER BY instance_id, epoch, idx"""
    ):
        slot = instances.setdefault(instance_id, empty_slot())
        slot["entries"] += 1
        try:
            message = json.loads(message_json or "{}")
        except ValueError:
            message = {}
        for bucket in buckets_of(message):
            slot["buckets"][bucket] += 1
    conn.close()
    return instances


def report(runs: pathlib.Path, only: str) -> int:
    index = runs / "results.jsonl"
    if not index.exists():
        print(f"{index} does not exist: that directory is not a recorded batch", file=sys.stderr)
        return 2
    rows = [json.loads(line) for line in index.read_text(encoding="utf-8").splitlines() if line.strip()]
    if only:
        rows = [row for row in rows if f'{row["task"]}.{row["group"]}.{row["repeat"]}' == only]
    if not rows:
        print("no trial matched", file=sys.stderr)
        return 2
    for row in sorted(rows, key=lambda r: (r["task"], r["group"], r["repeat"])):
        tag = f'{row["task"]}.{row["group"]}.{row["repeat"]}'
        db = runs / "state" / tag / "session.sqlite"
        print(
            f'=== {tag}  status={row.get("status")} wall={row.get("wall_s")}s '
            f'checks={"ok" if row.get("checks_ok") else "FAIL"}'
        )
        if not db.exists():
            print("    (no committed session.sqlite: the checkpoint did not carry one)")
            continue
        for instance_id, slot in sorted(trial_anatomy(db).items()):
            counts = " ".join(f"{b}={slot['buckets'][b]}" for b in ORDER if slot["buckets"][b])
            note = ""
            if slot["requests"] != slot["entries"]:
                note = f"  [!! {slot['requests']} requests vs {slot['entries']} entries]"
            print(
                f"    {instance_id:<16} requests={slot['requests']:<3} "
                f"model_ms={slot['model_ms'] / 1000:7.1f}  {counts}{note}"
            )
    return 0


def self_check() -> int:
    """The classifier's controls, on a synthetic store: no batch is needed to run the gate."""
    import tempfile

    findings = []
    leader = {"role": "assistant", "tool_calls": [{"function": {"name": "shell"}}], "content": ""}
    member = {"role": "assistant", "tool_calls": [{"function": {"name": "edit_file"}}], "content": ""}
    parked = {"role": "assistant", "content": "parked", "wait": {"mode": "ANY", "conditions": []}}
    if buckets_of(leader) != ["execute"]:
        findings.append(f"a shell call was bucketed {buckets_of(leader)}")
    if buckets_of(member) != ["edit"]:
        findings.append(f"an edit_file call was bucketed {buckets_of(member)}")
    if buckets_of(parked) != ["wait"]:
        findings.append(f"a parking response was bucketed {buckets_of(parked)}")
    if buckets_of({"role": "assistant", "tool_calls": [{"function": {"name": "unknown_tool"}}]}) != ["other"]:
        findings.append("an unknown tool did not land in `other`")
    with tempfile.TemporaryDirectory() as tmp:
        db = pathlib.Path(tmp) / "session.sqlite"
        conn = sqlite3.connect(db)
        conn.executescript(
            """CREATE TABLE instances (id TEXT PRIMARY KEY, session_id TEXT, lifecycle TEXT, phase TEXT);
               CREATE TABLE model_requests (request_id TEXT PRIMARY KEY, instance_id TEXT);
               CREATE TABLE attempts (attempt_id TEXT PRIMARY KEY, request_id TEXT, elapsed_ms INTEGER);
               CREATE TABLE context_entries (instance_id TEXT, epoch INTEGER, idx INTEGER, kind TEXT,
                                             message_json TEXT);"""
        )
        conn.execute("INSERT INTO instances VALUES ('i-leader', 's', 'ACTIVE', 'READY')")
        conn.execute("INSERT INTO instances VALUES ('w1', 's', 'ACTIVE', 'READY')")
        for n, (who, message) in enumerate([("i-leader", leader), ("w1", member)], start=1):
            conn.execute("INSERT INTO model_requests VALUES (?, ?)", (f"r{n}", who))
            conn.execute("INSERT INTO attempts VALUES (?, ?, ?)", (f"a{n}", f"r{n}", 1500))
            conn.execute(
                "INSERT INTO context_entries VALUES (?, 0, ?, 'assistant', ?)", (who, n, json.dumps(message))
            )
        conn.commit()
        conn.close()
        got = trial_anatomy(db)
        if got["i-leader"]["buckets"]["execute"] != 1 or got["w1"]["buckets"]["edit"] != 1:
            findings.append(f"the synthetic store decoded to {got}")
        if got["i-leader"]["model_ms"] != 1500:
            findings.append(f"model_ms was summed as {got['i-leader']['model_ms']}, not 1500")
    if report(pathlib.Path("/nonexistent-batch"), "") != 2:
        findings.append("a directory that is not a recorded batch was not refused")
    for finding in findings:
        print(f"FAIL: {finding}")
    print(f"anatomy self-check: {len(findings)} finding(s)")
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", default=str(DEFAULT_RUNS))
    parser.add_argument("--trial", default="")
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    return report(pathlib.Path(args.runs), args.trial)


if __name__ == "__main__":
    sys.exit(main())

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
import re
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


def unit_timeline(db: pathlib.Path, units: list) -> dict:
    """When each unit of an independent-units task was *written*, *settled* and *greened*, from the session.

    The three shapes are what the arms actually produce (D-261): a solo arm **writes** a unit (a write/edit call
    naming `units/<name>/`) and later **greens** it (a pytest result that reports passes and no failures, which
    covers every unit the call named — a batched `pytest units/a units/b` greens them together); a team arm
    **settles** it (its assignee names the unit). Times are seconds from the session's first event.
    """
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    start = next(iter(conn.execute("SELECT MIN(created) FROM events")))[0]
    written, settled, greened = {}, {}, {}
    settlements = []
    for assignee, created in conn.execute(
        """SELECT t.assignee, e.created FROM events e JOIN tasks t ON t.id = json_extract(e.payload_json, '$.task_id')
           WHERE e.kind = 'task_completed' AND json_extract(e.payload_json, '$.status') = 'SUCCEEDED'"""
    ):
        settlements.append(created - start)
        # the assignee *may* name its unit (the model often does), which is a witness for the mapping; when it
        # does not (`w01`, `w02`, …) the count still carries the timing, and every unit is done when the last
        # task of a full set settles — the frozen checks confirm the set covers the units (D-264)
        for unit in units:
            if assignee and unit in assignee:
                settled.setdefault(unit, created - start)
    if settlements and len(settlements) >= len(units):
        settled_all = max(settlements)
    else:
        settled_all = None
    runs, wrote = {}, set()
    for created, message_json in conn.execute(
        """SELECT created, message_json FROM context_entries WHERE kind = 'assistant' ORDER BY created"""
    ):
        message = json.loads(message_json or "{}")
        for call in message.get("tool_calls") or []:
            name = (call.get("function") or {}).get("name") or ""
            args = str((call.get("function") or {}).get("arguments") or "")
            for unit in units:
                if name in ("write_file", "edit_file") and f"units/{unit}/" in args:
                    written.setdefault(unit, created - start)
                    wrote.add(unit)
                # the command may name a unit as a path (`units/x`), after `cd`, or as a bare word in a loop;
                # all three are the same evidence, so the match is on the whole word (D-261's tool fix)
                if ("pytest" in args or "check.py" in args) and re.search(rf"(?<!\w){re.escape(unit)}(?!\w)", args):
                    runs.setdefault(call.get("id"), set()).add(unit)
    for created, message_json in conn.execute(
        """SELECT created, message_json FROM context_entries WHERE kind = 'tool_result' ORDER BY created"""
    ):
        message = json.loads(message_json or "{}")
        text = str(message.get("content") or "")
        if "passed" not in text or "failed" in text or "error" in text.lower():
            continue
        for unit in runs.get(message.get("tool_call_id"), ()):
            greened.setdefault(unit, created - start)
    conn.close()
    return {"written": written, "settled": settled, "settled_all": settled_all,
            "settlements": len(settlements), "greened": greened}


def report_units(runs: pathlib.Path, only: str, units: list) -> int:
    """Print each trial's unit timeline: what the arms reach, and when."""
    index = runs / "results.jsonl"
    if not index.exists():
        print(f"{index} does not exist: that directory is not a recorded batch", file=sys.stderr)
        return 2
    rows = [json.loads(line) for line in index.read_text(encoding="utf-8").splitlines() if line.strip()]
    for row in sorted(rows, key=lambda r: (r["task"], r["group"], r["repeat"])):
        tag = f'{row["task"]}.{row["group"]}.{row["repeat"]}'
        if only and only != tag:
            continue
        db = runs / "state" / tag / "session.sqlite"
        if not db.exists():
            continue
        t = unit_timeline(db, units)
        def last(d):
            return f"{max(d.values()):.1f}s" if d else "-"
        print(f'=== {tag}  wall={row.get("wall_s")}s checks={"ok" if row.get("checks_ok") else "FAIL"}')
        all_settled = f'{t["settled_all"]:.1f}s' if t["settled_all"] is not None else "-"
        print(f'    written {len(t["written"])}/{len(units)} (last {last(t["written"])})   '
              f'settled {t["settlements"]}/{len(units)} tasks (all by {all_settled})   '
              f'greened {len(t["greened"])}/{len(units)} (last {last(t["greened"])})')
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
                                             message_json TEXT, created REAL);"""
        )
        conn.execute("INSERT INTO instances VALUES ('i-leader', 's', 'ACTIVE', 'READY')")
        conn.execute("INSERT INTO instances VALUES ('w1', 's', 'ACTIVE', 'READY')")
        for n, (who, message) in enumerate([("i-leader", leader), ("w1", member)], start=1):
            conn.execute("INSERT INTO model_requests VALUES (?, ?)", (f"r{n}", who))
            conn.execute("INSERT INTO attempts VALUES (?, ?, ?)", (f"a{n}", f"r{n}", 1500))
            conn.execute(
                "INSERT INTO context_entries VALUES (?, 0, ?, 'assistant', ?, ?)",
                (who, n, json.dumps(message), 100.0 + n),
            )
        conn.commit()
        conn.close()
        got = trial_anatomy(db)
        if got["i-leader"]["buckets"]["execute"] != 1 or got["w1"]["buckets"]["edit"] != 1:
            findings.append(f"the synthetic store decoded to {got}")
        if got["i-leader"]["model_ms"] != 1500:
            findings.append(f"model_ms was summed as {got['i-leader']['model_ms']}, not 1500")
        # D-261: the unit timeline's three shapes, on the same synthetic session
        conn = sqlite3.connect(db)
        conn.execute("CREATE TABLE tasks (id TEXT, assignee TEXT)")
        conn.execute("CREATE TABLE events (session_id TEXT, sequence INTEGER, kind TEXT, payload_json TEXT, created REAL)")
        conn.execute("INSERT INTO tasks VALUES ('t-a', 'w-alpha')")  # the assignee names the unit, as the trials do
        conn.execute("INSERT INTO events VALUES ('s', 1, 'instance_created', '{}', 100.0)")
        conn.execute("INSERT INTO events VALUES ('s', 2, 'task_completed', '{\"task_id\": \"t-a\", \"status\": \"SUCCEEDED\"}', 130.0)")
        conn.execute("INSERT INTO context_entries VALUES ('w1', 0, 2, 'assistant', ?, 110.0)",
                     (json.dumps({"role": "assistant", "tool_calls": [
                         {"id": "c1", "function": {"name": "write_file", "arguments": "units/alpha/alpha.py"}},
                         {"id": "c2", "function": {"name": "shell", "arguments": "cd units/alpha && python3 -m pytest -q"}}]}),))
        conn.execute("INSERT INTO context_entries VALUES ('w1', 0, 3, 'tool_result', ?, 120.0)",
                     (json.dumps({"role": "tool", "tool_call_id": "c2", "content": "{\"output\":\"2 passed in 0.02s\"}"}),))
        conn.commit(); conn.close()
        units = unit_timeline(db, ["alpha", "beta"])
        if units["written"].get("alpha") != 10.0 or "beta" in units["written"]:
            findings.append(f"the write timeline decoded to {units['written']}")
        if units["settled"].get("alpha") != 30.0:
            findings.append(f"the settle map decoded to {units['settled']}")
        # the timing needs a *full* task set: one settlement covers one unit, not two (D-264)
        if units["settled_all"] is not None:
            findings.append(f"a partial task set produced a timing: {units['settled_all']}")
        if unit_timeline(db, ["alpha"])["settled_all"] != 30.0:
            findings.append("a full task set did not produce its last settlement as the timing")
        if units["greened"].get("alpha") != 20.0 or units["settled"].get("beta") is not None:
            findings.append(f"the green timeline decoded to {units['greened']}")
        if report_units(pathlib.Path("/nonexistent-batch"), "", ["alpha"]) != 2:
            findings.append("--units on a directory that is not a recorded batch was not refused")
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
    parser.add_argument("--units", action="store_true",
                        help="the per-unit timeline view (D-261): written / settled / greened, with times")
    parser.add_argument("--unit-names", default="",
                        help="comma-separated unit directory names for --units")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if args.units:
        names = [name for name in args.unit_names.split(",") if name]
        if not names:
            print("--units needs --unit-names (the unit directories of that task)", file=sys.stderr)
            return 2
        return report_units(pathlib.Path(args.runs), args.trial, names)
    return report(pathlib.Path(args.runs), args.trial)


if __name__ == "__main__":
    sys.exit(main())

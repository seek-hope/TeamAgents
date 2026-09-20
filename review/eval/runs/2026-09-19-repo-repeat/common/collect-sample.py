"""Collect a finished frozen sample without changing its candidate or session."""

import argparse
from collections import Counter
from datetime import datetime
import difflib
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import stat
import subprocess
import tempfile


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def fingerprint(path):
    info = path.lstat()
    item = {"mode": stat.S_IMODE(info.st_mode), "bytes": info.st_size}
    if stat.S_ISREG(info.st_mode):
        item["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    else:
        item["file_type"] = "symlink" if path.is_symlink() else "special"
    return item


def walk_candidate(root):
    excluded = {".git", "target", "core/target", "engine/target", "tui/target"}
    found = {}
    for directory, names, files in os.walk(root, followlinks=False):
        parent = Path(directory)
        for name in list(names):
            path = parent / name
            rel = path.relative_to(root).as_posix()
            if rel in excluded:
                names.remove(name)
            elif path.is_symlink():
                found[rel] = fingerprint(path)
                names.remove(name)
        for name in files:
            path = parent / name
            found[path.relative_to(root).as_posix()] = fingerprint(path)
    return dict(sorted(found.items()))


def test_summaries(output):
    return [
        {"status": status, "passed": int(passed), "failed": int(failed), "ignored": int(ignored)}
        for status, passed, failed, ignored in re.findall(
            r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", output
        )
    ]


def collect(base, number):
    source = base / f"sample-{number}"
    execution = json.loads((source / "execution.json").read_text())
    assert execution["status"] == "finished", "never collect a running candidate"
    assert execution["frozen_inputs_unchanged"] is True
    output = base / "collected" / source.name
    assert not output.exists(), "do not overwrite collected evidence"
    output.mkdir(parents=True)
    run = source / "run"
    for old, new in [
        (source / "execution.json", "execution.json"),
        (source / "runner.log", "runner.log"),
        (run / "repo-session-fork.jsonl", "teamagents.jsonl"),
        (run / "repo-session-fork.stderr", "teamagents.stderr"),
        (run / "repo-session-fork.grade.json", "grade.json"),
        (run / "repo-session-fork.grade.log", "grade.log"),
    ]:
        if old.exists():
            shutil.copy2(old, output / new)
    records = [json.loads(line) for line in (output / "teamagents.jsonl").read_text().splitlines() if line.strip()]
    final = next((r for r in reversed(records) if r.get("type") == "result"), None)
    assert final == execution["result"]
    tools = [r for r in records if r.get("type") == "tool"]
    session = run / "state/teamagents/sessions" / execution["session_id"]
    for name in ["verification.json", "profiles.json", "model_overrides.json"]:
        if (session / name).exists():
            shutil.copy2(session / name, output / name)
    snapshots = []
    checkpoint_steps = {}
    for member in sorted((session / "members").iterdir()):
        if not member.is_dir() or member.is_symlink():
            continue
        paths = [member / name for name in ["chat_tree.json", "chat_history.json", "usage.json", "plan.json"]]
        paths.extend(sorted((member / "turns").glob("*.json")))
        paths.extend(sorted((member / "tool-output").glob("*.log")))
        for path in paths:
            if not path.is_file() or path.is_symlink():
                continue
            relative = path.relative_to(session)
            target = output / "session" / (str(relative) + ".gz")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(gzip.compress(path.read_bytes(), mtime=0))
            if path.parent.name == "turns":
                checkpoint = json.loads(path.read_text())
                snapshots.append((member.name, path.stem, checkpoint))
                checkpoint_steps[path.stem] = checkpoint["model_steps"]
    for directory in [session / "artifacts", run / "artifacts"]:
        if directory.exists():
            for path in sorted(directory.rglob("*")):
                if path.is_file() and not path.is_symlink():
                    target = output / "model-artifacts" / path.relative_to(directory)
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, target)
    con = sqlite3.connect((session / "team.db").absolute().as_uri() + "?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    state = {table: [dict(row) for row in con.execute(f"SELECT * FROM {table}")]
             for table in ["sessions", "turn_runs", "tasks", "agent_runtime"]}
    state["events"] = [dict(row) for row in con.execute("SELECT * FROM events ORDER BY sequence")]
    con.close()
    save(output / "final-state.json", state)

    # A resumed turn can contain older history. Match receipts to the observed
    # call identity, preferring the checkpoint of the run that emitted the call.
    receipts = {}
    fallbacks = {}
    for agent, run_id, checkpoint in snapshots:
        for message in checkpoint.get("history", []):
            if message.get("role") == "tool" and "tool_call_id" in message:
                identity = (agent, message["tool_call_id"])
                receipts[(run_id, *identity)] = message.get("content")
                fallbacks[identity] = message.get("content")
    shell = []
    for tool in tools:
        if tool["tool"] != "shell":
            continue
        identity = (tool["agent_id"], tool["call_id"])
        raw = receipts.get((tool["run_id"], *identity), fallbacks.get(identity))
        decoded = False
        text = ""
        if isinstance(raw, str):
            try:
                content = json.loads(raw)
                if isinstance(content, dict) and isinstance(content.get("output"), str):
                    decoded = True
                    text = content["output"]
            except json.JSONDecodeError:
                pass
        terminal = re.search(r"\n\(exit (-?\d+)\)\s*$", text)
        shell.append({
            "agent_id": identity[0], "run_id": tool["run_id"], "call_id": identity[1],
            "arguments_preview": tool.get("arguments"), "structured_ok": tool["ok"],
            "full_receipt_found": raw is not None, "output_decoded": decoded,
            "output_bytes": len(text.encode()),
            "terminal_exit_marker": int(terminal[1]) if terminal else None,
            "has_failed_test_output": "test result: FAILED." in text,
            "error_excerpt": text[-1400:] if terminal or "test result: FAILED." in text else None,
        })
    save(output / "full-shell-diagnostics.json", shell)

    fixture = json.loads((base / "fixture-manifest.json").read_text())
    candidate = run / "work/repo-session-fork"
    manifest = walk_candidate(candidate)
    save(output / "candidate-manifest.json", manifest)
    allowed = {"engine/src/session.rs", "engine/src/worker.rs"}
    extra = sorted(set(manifest) - set(fixture))
    missing = sorted(set(fixture) - set(manifest))
    protected = sorted(name for name in set(fixture) & set(manifest) - allowed if fixture[name] != manifest[name])
    mode_changes = sorted(name for name in set(fixture) & set(manifest) if fixture[name]["mode"] != manifest[name]["mode"])
    patch = []
    for name in sorted(allowed):
        if (candidate / name).is_file() and not (candidate / name).is_symlink():
            target = output / "candidate" / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(candidate / name, target)
            patch.extend(difflib.unified_diff(
                (base / "calibration/negative" / name).read_text().splitlines(keepends=True),
                (candidate / name).read_text().splitlines(keepends=True),
                fromfile=f"a/{name}", tofile=f"b/{name}",
            ))
    (output / "candidate.patch").write_text("".join(patch))
    with tempfile.TemporaryDirectory(prefix="ta-repeat-patch-replay-") as scratch:
        root = Path(scratch)
        for name in sorted(allowed):
            target = root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(base / "calibration/negative" / name, target)
        applied = subprocess.run(["git", "apply", str(output / "candidate.patch")], cwd=root, capture_output=True, text=True)
        replay = {"exit_code": applied.returncode, "stdout": applied.stdout, "stderr": applied.stderr}
        if applied.returncode == 0:
            replay["hashes"] = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in sorted(allowed)}
            replay["matches_candidate"] = all((root / name).read_bytes() == (candidate / name).read_bytes() for name in sorted(allowed))
        save(output / "patch-replay.json", replay)
    for name in extra + protected:
        path = candidate / name
        if path.is_file() and not path.is_symlink() and path.stat().st_size < 10 * 1024 * 1024:
            target = output / "candidate-other" / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
    grade = json.loads((output / "grade.json").read_text()) if (output / "grade.json").exists() else None
    usage_rows = final.get("usage", []) if final else []
    assert isinstance(usage_rows, list), "inspect unknown usage representation"
    counters = ["calls", "prompt_tokens", "completion_tokens", "total_tokens", "cached_input_tokens", "unknown_usage_calls"]
    usage = {key: sum(row.get("usage", {}).get(key, 0) or 0 for row in usage_rows) for key in counters}
    integrity = not (extra or missing or protected or mode_changes)
    metrics = {
        "sample": number, "collected_at": datetime.now().astimezone().isoformat(),
        "session_id": execution["session_id"], "runner_exit": execution["exit_code"],
        "runner_elapsed_s": execution["elapsed_s"],
        "cli_status": final.get("status") if final else None,
        "cli_exit": final.get("exit_code") if final else None,
        "duration_ms": final.get("duration_ms") if final else None,
        "completed_and_verified": execution["completed_and_verified"],
        "independent_grade_ok": grade.get("ok") if grade else None,
        "grade_test_summaries": test_summaries(grade.get("output", "")) if grade else [],
        "public_test_summaries": [test_summaries(item.get("output", "")) for item in final.get("verification", [])] if final else [],
        "tool_calls": len(tools), "tool_counts": dict(sorted(Counter(tool["tool"] for tool in tools).items())),
        "tool_counts_by_agent": dict(sorted(Counter(tool["agent_id"] for tool in tools).items())),
        "structured_tool_failures": sum(tool["ok"] is False for tool in tools),
        "shell_receipts_decoded": sum(row["output_decoded"] for row in shell),
        "shell_nonzero_exit_markers": sum(row["terminal_exit_marker"] not in [None, 0] for row in shell),
        "shell_failed_test_output_records": sum(row["has_failed_test_output"] for row in shell),
        "usage": usage, "usage_by_agent": usage_rows, "checkpoint_steps_by_run": checkpoint_steps,
        "extra_files": extra, "missing_files": missing, "protected_changed": protected,
        "mode_changes": mode_changes, "final_file_protection": integrity,
    }
    if execution["completed_and_verified"]:
        assert integrity
    save(output / "metrics.json", metrics)
    print(json.dumps({k: metrics[k] for k in ["sample", "completed_and_verified", "cli_status", "duration_ms", "tool_calls", "final_file_protection"]}, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("sample", type=int, choices=[1, 2, 3])
    args = parser.parse_args()
    collect(Path(__file__).resolve().parent, args.sample)

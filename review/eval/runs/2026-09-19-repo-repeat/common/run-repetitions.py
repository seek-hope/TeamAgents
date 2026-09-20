from pathlib import Path
import datetime
import hashlib
import json
import os
import shutil
import subprocess
import time
import tomllib

ROOT = Path(__file__).resolve().parents[3]
BASE = Path(__file__).resolve().parent
TASK = ROOT / "review/eval/tasks/repo-session-fork"
PROFILE = BASE / "config/teamagents/config.toml"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path, value):
    staged = path.with_suffix(path.suffix + ".tmp")
    staged.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    staged.replace(path)


def now():
    return datetime.datetime.now().astimezone().isoformat()


for name in ["make-check", "grader-contracts", "runner-contracts", "calibration-positive"]:
    assert json.loads((BASE / f"{name}-result.json").read_text())["exit_code"] == 0, name
assert json.loads((BASE / "calibration-positive.json").read_text())["ok"] is True
assert json.loads((BASE / "calibration-negative.json").read_text())["ok"] is False
profile = tomllib.loads(PROFILE.read_text())["models"]["leader_main"]
assert profile["model"] == "deepseek-flash"
assert profile["context_window"] == 1_000_000
assert profile["generation_options"]["reasoning_effort"] == "high"
assert (profile["timeout"], profile["max_retries"]) == (120, 5)
assert os.environ.get(profile["api_key_env"]), "missing credential"

binary = BASE / "bin/teamagents"
binary.parent.mkdir(exist_ok=True)
shutil.copy2(ROOT / "engine/target/debug/teamagents", binary)
frozen = {str(PROFILE.relative_to(ROOT)): digest(PROFILE), str(binary.relative_to(ROOT)): digest(binary)}
for crate in ["core", "engine"]:
    for source in (ROOT / crate / "src").rglob("*.rs"):
        frozen[str(source.relative_to(ROOT))] = digest(source)
    for suffix in ["toml", "lock"]:
        source = ROOT / crate / f"Cargo.{suffix}"
        frozen[str(source.relative_to(ROOT))] = digest(source)
for path in [ROOT / "engine/tests/eval_grader.rs", ROOT / "review/eval/run.sh", ROOT / "rust-toolchain.toml"]:
    frozen[str(path.relative_to(ROOT))] = digest(path)
for source in TASK.rglob("*"):
    if source.is_file():
        frozen[str(source.relative_to(ROOT))] = digest(source)

registration = BASE / "repetitions-registration.json"
assert not registration.exists(), "do not restart or overwrite a registered experiment"
save(registration, {
    "prepared_at": now(),
    "samples_planned": 3,
    "scheduling": "sequential; fresh workspace and session per sample",
    "task": "repo-session-fork",
    "environment_revision": "explicit-fixture-config-after-private-home",
    "task_timeout_s": 1200,
    "provider": profile["provider"],
    "protocol": profile["protocol"],
    "model": profile["model"],
    "native_context_window": profile["context_window"],
    "context_window_source": "用户确认；docs/DECISIONS.md D-36",
    "reasoning_effort": "high",
    "request_timeout_s": 120,
    "max_retries": 5,
    "git_head": subprocess.check_output(["git", "rev-parse", "--verify", "HEAD"], cwd=ROOT, text=True).strip(),
    "frozen_sha256": frozen,
    "rules": [
        "No changes to the binary, task, candidate, prompts, profile, or grader during the experiment.",
        "Keep every started sample, including timeouts and failed grades; no success-conditioned stopping.",
        "Hidden tests are injected only into independent grading copies after the model process exits.",
        "Historical samples have a different environment declaration and are not pooled into this rate.",
    ],
})

results = []
for number in range(1, 4):
    for name, expected in frozen.items():
        assert digest(ROOT / name) == expected, f"frozen input changed: {name}"
    sample = BASE / f"sample-{number}"
    sample.mkdir()
    output = sample / "run"
    command = [str(ROOT / "review/eval/run.sh"), "--bin", str(binary), "--only", "repo-session-fork", "--out", str(output)]
    environment = os.environ.copy()
    environment.update(XDG_CONFIG_HOME=str(BASE / "config"), TMPDIR=str(BASE / "scratch"))
    started = time.monotonic()
    metadata = {"sample": number, "started_at": now(), "command": command, "status": "running"}
    with (sample / "runner.log").open("w") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        metadata["pid"] = process.pid
        save(sample / "execution.json", metadata)
        print(json.dumps({"sample": number, "status": "running", "pid": process.pid, "started_at": metadata["started_at"]}), flush=True)
        return_code = process.wait()
    metadata.update(status="finished", finished_at=now(), exit_code=return_code, elapsed_s=round(time.monotonic()-started, 3))
    for name, expected in frozen.items():
        assert digest(ROOT / name) == expected, f"frozen input changed: {name}"
    metadata["frozen_inputs_unchanged"] = True
    records_file = output / "repo-session-fork.jsonl"
    records = [json.loads(line) for line in records_file.read_text().splitlines() if line.strip()] if records_file.exists() else []
    final = records[-1] if records and records[-1].get("type") == "result" else None
    grade_path = output / "repo-session-fork.grade.json"
    grade = json.loads(grade_path.read_text()) if grade_path.exists() else None
    metadata["model_started"] = any(row.get("type") == "session" for row in records)
    metadata["session_id"] = next((row["session_id"] for row in records if row.get("type") == "session"), None)
    metadata["result"] = final
    metadata["grade_ok"] = grade.get("ok") if grade else None
    metadata["grade_reason"] = grade.get("reason") if grade else "grade report missing"
    metadata["completed_and_verified"] = bool(return_code == 0 and final and final.get("status") == "completed" and final.get("exit_code") == 0 and len(final.get("verification", [])) == 1 and all(check.get("ok") is True for check in final["verification"]) and grade and grade.get("ok") is True)
    save(sample / "execution.json", metadata)
    results.append(metadata)
    save(BASE / "repetitions-results.json", {"planned": 3, "finished": len(results), "completed_and_verified": sum(row["completed_and_verified"] for row in results), "samples": results})
    print(json.dumps({"sample": number, "status": "finished", "exit_code": return_code, "completed_and_verified": metadata["completed_and_verified"], "elapsed_s": metadata["elapsed_s"], "grade_reason": metadata["grade_reason"]}, ensure_ascii=False), flush=True)

print(json.dumps({"status": "finished", "samples": 3, "completed_and_verified": sum(row["completed_and_verified"] for row in results)}), flush=True)

#!/usr/bin/env python3
"""A real-model check that a team can span two providers (A27).

One session, two models: the Leader runs on DeepSeek Flash (native 1M window, D-36) and spawns one worker
with `spawn(model = "worker_kimi")`, which is the Kimi entry from the user's own catalog shape (262,144
tokens). The worker writes a file in the shared workspace; the Leader waits for the task and reports.

The artifact decides: `answer.txt` must contain exactly what the task asked for. The probe also asserts the
session really is heterogeneous — the two instances carry different resolved model names — and that the
delegation exchanged a task assignment and a task start. The task result is expected as a `task_completed`
event too, but the worker is a model, so the probe accepts the one recorded shape where it is missing (the
worker answers with prose, its task stays open, and the leader settles the goal on the artifact itself) and
reports that as the known gap it is — any other shape fails. `--self-check` checks that classification
without a model, a network or credentials.

    python3 review/dogfood/providers.py                  # fresh /tmp state root
    python3 review/dogfood/providers.py --state-dir /tmp/ta-providers --timeout 600
    python3 review/dogfood/providers.py --self-check     # the shapes above, no model needed

It is a real-model check: it needs `DEEPSEEK_API_KEY` and `KIMI_API_KEY`, uses each model's native window,
and writes only under `--state-dir`.
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
BIN = REPO / "engine/target/debug/teamagents"

CONFIG = """# Two-provider dogfood (A27): the Leader on DeepSeek Flash, one worker on Kimi.
# Each model keeps its native context window (D-36).
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }

[models.worker_kimi]
provider = "kimi"
protocol = "responses"
model = "k3-256k"
base_url = "https://api.kimi.com/coding/v1"
api_key_env = "KIMI_API_KEY"
timeout = 300
max_retries = 1
generation_options = { reasoning_effort = "low" }
context_window = 262144
"""

PROMPT = """Work with a teammate on this task.

1. Spawn one worker with `model` set to `worker_kimi` (that key is in the catalog) and instructions
   "write the file the task asks for".
2. Delegate one task to that worker: write the file answer.txt in the shared workspace whose content is
   exactly the line `kimi wrote this`.
3. Wait for the task to settle, then report what the worker said and whether the file is there."""


def session_facts(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    instances = [
        {"id": row[0], "lifecycle": row[1], "model": json.loads(row[2]).get("model", "")}
        for row in db.execute("SELECT id, lifecycle, profile_json FROM instances ORDER BY id")
    ]
    tasks = list(db.execute("SELECT id, assignee, status, requester FROM tasks ORDER BY rowid"))
    kinds = [row[0] for row in db.execute("SELECT kind FROM events ORDER BY sequence")]
    requests = list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]
    return {"instances": instances, "tasks": tasks, "events": kinds, "requests": requests}


def classify_task_result(events: list, tasks: list, artifact_ok: bool, goal_status) -> tuple:
    """Classify how the delegated task ended: `completed`, `known_gap`, or `unexpected`.

    Split out of `main` so the three shapes are checkable without a model or credentials (`--self-check`).
    `tasks` is `session_facts()`'s `(id, assignee, status, requester)` rows.
    """
    if "task_completed" in events:
        return "completed", "the worker settled its task (task_completed event present)"
    unsettled = [task for task in tasks if task[2] in ("PENDING", "RUNNING")]
    if artifact_ok and goal_status == "SUCCEEDED" and unsettled:
        return "known_gap", (
            "known gap (docs/ACCEPTANCE.md, 'A model that stops settling its task leaves a visible wait'): "
            f"the worker answered with prose instead of calling `finish`, so task {unsettled[0][0]} stayed "
            f"{unsettled[0][2]} while the leader verified the artifact itself and settled the goal "
            "SUCCEEDED — the runtime does not resolve an unsettled task"
        )
    return "unexpected", (
        "no task_completed event and not the recorded known-gap shape: "
        f"artifact_ok={artifact_ok} goal_status={goal_status!r} "
        f"unsettled_tasks={[(task[0], task[2]) for task in unsettled]}"
    )


def self_check() -> int:
    """The three shapes the classifier must separate, without a model, a network or credentials."""
    settled = [("t-1", "i-worker", "SUCCEEDED", "i-leader")]
    running = [("t-1", "i-worker", "RUNNING", "i-leader")]
    cases = [
        (["task_completed"], settled, True, "SUCCEEDED", "completed"),
        # the recorded known gap: prose answer, artifact still exact, goal settled with the task open
        (["task_delegated"], running, True, "SUCCEEDED", "known_gap"),
        # every other shape stays a failure, including a settled task with no completion event
        (["task_delegated"], settled, True, "SUCCEEDED", "unexpected"),
        (["task_delegated"], running, False, "SUCCEEDED", "unexpected"),
        (["task_delegated"], running, True, "BLOCKED", "unexpected"),
        (["task_delegated"], running, True, None, "unexpected"),
    ]
    for events, tasks, artifact_ok, goal_status, expected in cases:
        got, detail = classify_task_result(events, tasks, artifact_ok, goal_status)
        if got != expected:
            print(f"FAIL: {events} {tasks} artifact_ok={artifact_ok} goal={goal_status!r} -> {got}, want {expected}")
            return 1
    print(f"self-check ok: {len(cases)} task-result shapes classified as expected")
    return 0



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    leak_guard.stop_daemons(state_root)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-providers)")
    parser.add_argument("--timeout", type=int, default=600, help="exec --timeout in seconds")
    parser.add_argument("--self-check", action="store_true", help="check the task-result shapes and exit")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    for key in ("DEEPSEEK_API_KEY", "KIMI_API_KEY"):
        if not os.environ.get(key, "").strip():
            raise SystemExit(f"{key} is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-providers")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), PROMPT],
        capture_output=True, text=True, env=env,
    )
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} goal={report.get('goal_status')}")
    if run.stderr.strip():
        print("stderr:", run.stderr.strip()[:400])

    facts = session_facts(state_root)
    print(f"model requests: {facts['requests']}")
    for instance in facts["instances"]:
        print(f"  instance {instance['id']}: {instance['lifecycle']} on model {instance['model'] or '(none)'}")
    for task in facts["tasks"]:
        print(f"  task {task[0]}: {task[2]} (assignee {task[1]}, requester {task[3]})")

    # 1. the team really spanned the two providers: each member's stored profile
    #    names its resolved model (D-69), so this is readable from the session itself
    models = {instance["id"]: instance["model"] for instance in facts["instances"]}
    expected = {"i-leader": "deepseek-flash"}
    print(f"members and their models: {models}")
    if models.get("i-leader") != "deepseek-flash":
        failures.append(f"the leader is not recorded on its configured model: {models}")
    workers = {model for instance, model in models.items() if instance != "i-leader"}
    if workers != {"k3-256k"}:
        failures.append(f"the worker did not run the Kimi entry: {models}")

    # 2. the delegation exchanged a task assignment and a task start
    for kind in ("task_delegated", "task_started"):
        if kind not in facts["events"]:
            failures.append(f"no {kind} event: {facts['events']}")

    # 3. the artifact is the contract
    answer = (workspace / "answer.txt")
    artifact_ok = answer.is_file() and answer.read_text().strip() == "kimi wrote this"
    if artifact_ok:
        print("answer.txt present with the expected content")
    else:
        failures.append(f"answer.txt is missing or wrong: {answer.read_text() if answer.is_file() else '(absent)'!r}")

    # 4. the task result, read together with the artifact and the goal: `task_completed` is expected
    #    (the worker called `finish`), and the one recorded shape where it is missing is reported as the
    #    known gap instead of passing silently. `classify_task_result` holds the rule and `--self-check`
    #    checks it without a model.
    verdict, detail = classify_task_result(facts["events"], facts["tasks"], artifact_ok, report.get("goal_status"))
    print(detail)
    if verdict == "unexpected":
        failures.append(detail)

    # 5. the goal settled on its own report
    if report.get("goal_status") not in (None, "SUCCEEDED") and report.get("end") != "reply":
        failures.append(f"the goal did not settle successfully: {report.get('goal_status')} / {report.get('end')}")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""A real-model check that a `git_worktree` member works in its own tree (§12.3/D-46).

The probe makes a real git repository, asks the model to spawn one worker with
`workspace = "git_worktree"` and delegate a file write to it, and then walks the documented
lifecycle of that workspace:

1. the worker's tools really work *inside its own worktree* - the file it writes is there and
   **not** in the shared project (the D-57 class: a member that silently edits the shared
   checkout is a wrong-tree bug no unit test sees);
2. terminating the instance while the work is uncommitted **keeps** the worktree and reports
   why (nothing is deleted on a guess);
3. after the branch is committed and merged, the running supervisor retires the worktree by
   itself (its next discovery pass), record included.

    python3 review/dogfood/workspace.py                    # DeepSeek
    python3 review/dogfood/workspace.py --provider kimi    # over `responses`

It is a real-model check: it needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi), uses each
model's native window (D-36), writes only under `--state-dir`, and is not part of `make check`.
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
BIN = REPO / "engine/target/debug/teamagents"

MODELS = {
    "deepseek": """[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }
""",
    "kimi": """[models.leader_main]
provider = "kimi"
protocol = "responses"
model = "k3-256k"
base_url = "https://api.kimi.com/coding/v1"
api_key_env = "KIMI_API_KEY"
timeout = 300
max_retries = 1
generation_options = { reasoning_effort = "low" }
context_window = 262144
""",
}

MARKER = "written inside the worktree"
PROMPT = f"""Work with a teammate whose own workspace is a git worktree.

1. Spawn one worker with `workspace` set to `git_worktree` and instructions "write the file your task asks for".
2. Delegate this task to it: create the file report.md in your workspace containing exactly the line `{MARKER}`.
3. Wait for the task, then report the task's result as finished."""


def git(cwd: pathlib.Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True)



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    leak_guard.stop_daemons(state_root)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-workspace)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-workspace-{args.provider}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    project = root / "project"
    shutil.rmtree(root, ignore_errors=True)
    project.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text("skills_paths = []\n\n" + MODELS[args.provider])
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    # a clean git repository: what `git_worktree` requires
    for command in (
        ("init", "-q"),
        ("config", "user.email", "probe@example.invalid"),
        ("config", "user.name", "workspace probe"),
        ("config", "commit.gpgsign", "false"),
    ):
        done = git(project, *command)
        if done.returncode != 0:
            raise SystemExit(f"git {command[0]} failed: {done.stderr.strip()}")
    (project / "README.md").write_text("the shared project\n")
    git(project, "add", "-A")
    git(project, "commit", "-q", "-m", "initial")

    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(project), PROMPT],
        capture_output=True, text=True, env=env,
    )
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"provider={args.provider} exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} "
          f"goal={report.get('goal_status')}")
    if run.stderr.strip().startswith("exec:"):
        print("stderr:", run.stderr.strip()[:300])

    # 1. the goal settled and a worktree member exists
    if report.get("goal_status") != "SUCCEEDED":
        failures.append(f"the delegation did not settle: {report.get('end')} {report.get('goal_status')!r} "
                        f"{report.get('failure')!r}")
    members = []
    for member in sorted((state_root / "instances").glob("*")) if (state_root / "instances").is_dir() else []:
        record = member / "workspace.json"
        if record.is_file():
            members.append((member.name, json.loads(record.read_text())))
    worktrees = [(name, info) for name, info in members if info.get("policy") == "git_worktree"]
    print(f"  member workspaces: {[(name, info.get('policy')) for name, info in members]}")
    if not worktrees:
        failures.append("no member ran in a git worktree (did the model ask for one?)")
        for failure in failures:
            print("FAIL:", failure)
        return 1
    worker, record = worktrees[0]
    worktree = pathlib.Path(record["path"])

    # 2. the work happened *there*, not in the shared project
    written = worktree / "report.md"
    shared = project / "report.md"
    if not written.is_file() or MARKER not in written.read_text():
        failures.append(f"the worker did not write {written}")
    if shared.exists():
        failures.append(f"the worker wrote into the shared project instead of its worktree: {shared}")
    listing = git(project, "worktree", "list").stdout
    if str(worktree) not in listing:
        failures.append(f"git does not list {worktree} as a worktree: {listing}")

    def terminate() -> tuple[int, str]:
        done = subprocess.run(
            [str(BIN), "instances", "terminate", "--id", worker, "--yes", "--state-root", str(state_root)],
            capture_output=True, text=True, env=env,
        )
        return done.returncode, (done.stdout + done.stderr).strip()

    # 3. uncommitted work is never deleted: the worktree stays and the reason is reported
    code, output = terminate()
    print(f"  terminate with uncommitted work: exit={code} {output[:140]}")
    time.sleep(1.5)
    if not worktree.exists():
        failures.append("the worktree was deleted although its work was uncommitted")
    log = (state_root / "daemon.log").read_text() if (state_root / "daemon.log").is_file() else ""
    if "workspace of" not in log or "kept" not in log:
        failures.append("the refusal was not reported (daemon.log has no 'workspace of … kept' line)")

    # 4. commit and merge: the running supervisor retires the worktree on its next pass
    git(worktree, "add", "-A")
    committed = git(worktree, "commit", "-q", "-m", "worktree result")
    if committed.returncode != 0:
        failures.append(f"the probe could not commit in the worktree: {committed.stderr.strip()}")
    branch = git(worktree, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
    merged = git(project, "merge", "--no-edit", branch)
    print(f"  merged {branch}: {'ok' if merged.returncode == 0 else merged.stderr.strip()[:120]}")
    # Wait for the *pair*: the retirement removes the worktree directory and then its two records
    # (`workspace.json` beside the member and `worktree.json` beside the worktree), so sampling once as soon as
    # the directory disappears reports a race. Measured 2026-09-26: two failures in five runs, one of which left
    # state showing both records gone a moment later (D-142).
    record_path = pathlib.Path(record["path"]).parent / "workspace.json"
    retired, settled = False, False
    for _ in range(120):
        retired = not worktree.exists()
        if retired and not record_path.exists():
            settled = True
            break
        time.sleep(0.25)
    if not retired:
        failures.append(f"the merged worktree was not retired by the running session: {worktree}")
    elif not settled:
        failures.append(f"the worktree was retired but its record is still there after 30 s: {record_path}")
    if failures:
        # Say what the *engine's* bookkeeping looks like when this fails: whether git still lists the worktree
        # (the retirement's git half) and which of the two records survived (the half after it). One failure in
        # the 2026-09-26 sweeps had the directory gone, both records present and only the earlier *refusal* in
        # the daemon log, which is not a shape the code explains (D-142); this dump is what settles it.
        print(f"  git worktree list: {git(project, 'worktree', 'list').stdout.strip()[:300]}")
        print(f"  records: {record_path.exists()=}, {(pathlib.Path(record['path']).parent / 'worktree.json').exists()=}")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

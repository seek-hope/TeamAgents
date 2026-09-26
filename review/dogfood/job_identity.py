#!/usr/bin/env python3
"""A running command's identity, and its one-start rule, checked against the machine (A15/A10).

Two claims live in the runner's journal and neither had a live witness:

* **A15 — environment identity is traceable.** The journal records the child's pid, its `/proc/<pid>/stat`
  start ticks and the machine's boot id, because "the pid is alive" never proves it is the *same* process
  (pids are recycled). The probe reads the journal while the command runs and re-derives all three from
  `/proc` itself, so the check compares the runner's record with the machine, not with itself.
* **A10 — duplicate dispatch / GO.** GO is idempotent: a second GO returns the same journal and starts no
  second command. The probe sends one over the runner's own socket — the same request the driver sends —
  and requires `starts` to stay at 1 with the same pid. The same connection is also the negative control for
  §6.2's job token: a *guessed* token must not reach the runner at all.

The flow to get there is the one `cancel.py` uses: the Leader hires a worker, the user grants it
`shell@workspace` (§5.1 gives a spawned worker none), and the user sends the command to that member directly
(A21). The command loops forever; the probe stops the member when it is done with it.

    python3 review/dogfood/job_identity.py
    python3 review/dogfood/job_identity.py --state-dir /tmp/ta-jobid

It is a real-model check (`DEEPSEEK_API_KEY`, native window D-36) and runs the session in `full_auto`. Not
part of `make check`; everything it writes stays under `--state-dir` and it stops the daemon it started.
"""
import argparse
import atexit
import hashlib
import json
import os
import pathlib
import shutil
import socket as socket_module
import subprocess
import sys
import time
import uuid

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"

CONFIG = """# Job-identity probe (A15/A10): a real model on the native context window (D-36).
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 120
max_retries = 2
generation_options = { reasoning_effort = "high" }
"""

FIRST = """Spawn exactly one worker and reply with its instance id. Do not delegate or ask it anything yet —
just report the id and end your turn without settling the goal."""

INSTRUCTION = """Run exactly this shell command in the shared workspace `{workspace}` and report what it printed:

{command}

Pass the command through unchanged and do not pass a `timeout` argument — the command is expected to keep
running, and the runtime, not the model, decides when it stops."""


def call(bin_args: list[str], env: dict, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def protocol(socket_path: pathlib.Path, method: str, params: dict, command_id: str | None = None) -> dict:
    connection = socket_module.socket(socket_module.AF_UNIX)
    connection.settimeout(30)
    connection.connect(str(socket_path))
    stream = connection.makefile("rw")
    stream.readline()  # greeting
    request = {"protocol_version": 1, "request_id": f"probe-{uuid.uuid4()}", "method": method, "params": params}
    if command_id:
        request["command_id"] = command_id
    stream.write(json.dumps(request) + "\n")
    stream.flush()
    reply = json.loads(stream.readline())
    connection.close()
    return reply


def socket_name(token: str) -> str:
    """§6.2: the abstract socket name is derived from the job token, never guessed."""
    digest = hashlib.sha256(b"teamagents-job:" + token.encode()).hexdigest()
    return f"teamagents-job-{digest[:32]}"


def runner_request(name: str, token: str, method: str) -> dict:
    connection = socket_module.socket(socket_module.AF_UNIX)
    connection.settimeout(10)
    connection.connect("\0" + name)  # abstract namespace
    stream = connection.makefile("rw")
    stream.write(json.dumps({"version": 1, "method": method, "token": token}) + "\n")
    stream.flush()
    reply = json.loads(stream.readline())
    connection.close()
    return reply


def job_dirs(state_root: pathlib.Path) -> list[pathlib.Path]:
    """Every job the session started, under `<state root>/instances/<instance>/jobs/<operation>`."""
    return sorted(p.parent for p in (state_root / "instances").rglob("jobs/*/journal.json"))


def read_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text()) if path.is_file() else {}


def proc_start_ticks(pid: int) -> int | None:
    """The reference implementation for the journal's `start_ticks` (field 22 of /proc/<pid>/stat)."""
    try:
        text = pathlib.Path(f"/proc/{pid}/stat").read_text()
    except OSError:
        return None
    return int(text.rsplit(")", 1)[1].split()[19])


def wait_until(predicate, timeout: float, step: float = 0.5) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(step)
    return predicate()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-jobid)")
    parser.add_argument("--timeout", type=int, default=180, help="seconds to wait for the command to run")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-jobid")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    heartbeat = workspace / "heartbeat.txt"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    common = ["--state-root", str(state_root)]
    command = f"while true; do echo tick >> {heartbeat}; sleep 0.2; done"
    failures: list[str] = []
    daemon = None
    worker: str | None = None
    log = open(root / "daemon.log", "w")
    try:
        daemon = subprocess.Popen([str(BIN), "daemon", *common, "--cwd", str(workspace), "--full-auto"],
                                  env=env, stdout=log, stderr=subprocess.STDOUT, text=True)
        socket_path = state_root / "daemon.sock"
        if not wait_until(lambda: socket_path.exists() or daemon.poll() is not None, 30):
            failures.append("the daemon never bound its socket")
            return 1
        if not socket_path.exists():
            failures.append(f"the daemon exited: {log.read()[-300:]}")
            return 1

        promoted = call(["exec", *common, "--full-auto", "--json", "--timeout", "180", "--cwd", str(workspace),
                         FIRST], env, timeout=300)
        view = json.loads(call(["authority", *common, "--json"], env, timeout=60).stdout)
        workers = [i["id"] for i in view["instances"] if i["id"] != "i-leader"]
        if len(workers) != 1:
            failures.append(f"the Leader did not spawn exactly one worker ({workers})")
            return 1
        worker = workers[0]
        granted = call(["authority", *common, "grant", "--subject", worker, "--action", "shell",
                        "--scope", "workspace", "--json"], env, timeout=60)
        if granted.returncode != 0:
            failures.append(f"the shell grant was refused: {granted.stdout or granted.stderr}")
            return 1
        print(f"  worker {worker} holds shell@workspace; spawn turn exit={promoted.returncode}")

        envelope = f"probe-{uuid.uuid4()}"
        submitted = protocol(socket_path, "submit_input",
                             {"instance_id": worker, "envelope_id": envelope,
                              "text": INSTRUCTION.format(workspace=workspace, command=command)},
                             f"input-{envelope}")
        if not submitted.get("ok"):
            failures.append(f"submit_input was refused: {submitted.get('error')}")
            return 1

        # --- the job is running: its journal must describe the real process ------
        if not wait_until(lambda: any(read_json(d / "journal.json").get("state") == "RUNNING"
                                      for d in job_dirs(state_root)), args.timeout):
            seen = [(str(d), read_json(d / "journal.json").get("state")) for d in job_dirs(state_root)]
            failures.append(f"no job reached RUNNING; the jobs seen were {seen}"
                            " (a model that passes its own `timeout` lets the command end before the probe"
                            " can look at the running job)")
            return 1
        running = [d for d in job_dirs(state_root) if read_json(d / "journal.json").get("state") == "RUNNING"]
        if len(running) != 1:
            failures.append(f"expected exactly one running job, saw {[str(d) for d in running]}")
            return 1
        job = running[0]
        journal = read_json(job / "journal.json")
        spec = read_json(job / "job.json")
        pid = journal.get("pid")
        print(f"  job {journal.get('job_id')} is RUNNING: pid={pid} starts={journal.get('starts')} "
              f"state={journal.get('state')}")

        if journal.get("starts") != 1:
            failures.append(f"the command started {journal.get('starts')} times, expected exactly once")
        if not isinstance(pid, int):
            failures.append(f"the journal carries no pid: {journal}")
            return 1
        if not pathlib.Path(f"/proc/{pid}").exists():
            failures.append(f"the recorded pid {pid} is not a live process")
        machine_ticks = proc_start_ticks(pid)
        if machine_ticks is None or journal.get("start_ticks") != machine_ticks:
            failures.append(f"start_ticks {journal.get('start_ticks')} != /proc/{pid}/stat {machine_ticks}")
        else:
            print(f"  the recorded start ticks match /proc/{pid}/stat ({machine_ticks})")
        machine_boot = pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        if journal.get("boot_id") != machine_boot:
            failures.append(f"boot_id {journal.get('boot_id')!r} != {machine_boot!r}")
        else:
            print(f"  the recorded boot id matches this machine ({machine_boot[:8]}…)")

        # --- the runner answers on its own token-derived socket -----------------
        token = spec.get("token", "")
        name = socket_name(token)
        status = runner_request(name, token, "status")
        if not status.get("ok"):
            failures.append(f"the runner refused a status request: {status.get('error')}")
        else:
            reported = status["journal"]
            same = (reported.get("pid"), reported.get("start_ticks")) == (journal.get("pid"), journal.get("start_ticks"))
            print(f"  the runner answers on the socket derived from its token ({'same identity' if same else 'DIFFERENT'})")
            if not same:
                failures.append("the runner's own view of the process identity differs from its journal")

        # --- A10: a duplicate GO starts nothing ---------------------------------
        duplicate = runner_request(name, token, "go")
        if not duplicate.get("ok"):
            failures.append(f"a duplicate GO was refused: {duplicate.get('error')}")
        else:
            after = duplicate["journal"]
            if after.get("starts") != 1:
                failures.append(f"a duplicate GO started the command again (starts={after.get('starts')})")
            elif after.get("pid") != pid:
                failures.append(f"a duplicate GO changed the pid ({pid} → {after.get('pid')})")
            else:
                print(f"  a duplicate GO returned the same journal: starts is still 1, pid unchanged ({pid})")

        # --- §6.2: a guessed token cannot reach the runner ----------------------
        try:
            runner_request(socket_name(token + "-wrong"), token + "-wrong", "status")
            failures.append("a guessed job token reached a runner")
        except OSError as error:
            print(f"  a guessed token cannot reach the runner ({error.__class__.__name__})")

        # --- cleanup: stop the member the probe hired ---------------------------
        stopped = call(["instances", *common, "terminate", "--id", worker, "--yes", "--json"], env, timeout=60)
        print(f"  cleanup: terminate {worker} exit={stopped.returncode}")
    finally:
        if worker and daemon is not None and daemon.poll() is None:
            call(["instances", *common, "terminate", "--id", worker, "--yes", "--json"], env, timeout=60)
        if daemon is not None and daemon.poll() is None:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
        subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)
        log.close()
        for failure in failures:
            print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

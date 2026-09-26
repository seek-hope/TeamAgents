#!/usr/bin/env python3
"""What one running command costs the machine (the measurement behind D-116).

Every command a member runs has a runner process next to it and a driver polling that runner. Neither is free,
and both are paid for as long as the command runs, so a long build (or a team of members building in parallel)
pays for it in proportion to how sensible the cadences are:

* the runner's own tick (how often it looks at its child),
* the driver's `status` poll (one socket round trip per poll).
* and, since D-153, the cost of an **idle** runner: one that has no child and has a client, i.e. the state every
  crashed or abandoned job leaves behind. It used to keep ticking at the same cadence forever, which is how one
  machine accumulated 1,397 live runners burning CPU (measured 2026-09-26 at ~0.8 % of a core each).

This probe measures both on this machine, at the cadences the code uses and at the ones it used to use, and it
needs no model and no daemon: it writes one job directory, starts `teamagents jobs-runner` itself, sends GO over
the runner's own socket (the abstract name derives from the token, §6.2) and samples `/proc/<pid>/stat`:

    python3 review/runner_cost.py            # ~60 s: one 45 s command, then the idle window after it exits

The numbers are machine-dependent, so the probe prints the table rather than asserting a threshold; the
*relationship* (a finer cadence costs proportionally more) is what the code's constants are chosen against, and
`jobs::tests::the_runner_tick_trades_cpu_for_a_bounded_latency` plus
`v2::driver::cadence::the_job_poll_is_coarse_but_prompt` pin the bounds at compile time.
"""
import hashlib
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[1]
BIN = REPO / "engine/target/debug/teamagents"
STATE = "/tmp/ta-runner-cost"


def cpu_ticks(pid):
    fields = pathlib.Path(f"/proc/{pid}/stat").read_text().split()
    return int(fields[13]) + int(fields[14])  # utime + stime, in 100 Hz ticks


def main():
    if not BIN.exists():
        print(f"{BIN} is missing: run `make build` first")
        return 2
    root = pathlib.Path(STATE)
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir(parents=True)
    # a token of its own per run: the runner's socket is an *abstract* name derived from it, so a leaked
    # runner from an earlier run would otherwise shadow this one and the probe would fail confusingly
    token = f"runner-cost-{os.getpid()}"
    (root / "job.json").write_text(json.dumps({
        "job_id": "runner-cost", "program": "/bin/sh", "args": ["-c", "sleep 45"], "cwd": str(root),
        "env": [("PATH", os.environ.get("PATH", ""))],
        "deadline_ms": int(time.time() * 1000) + 120_000, "token": token,
    }))
    runner = subprocess.Popen([BIN, "jobs-runner", str(root)])
    name = "teamagents-job-" + hashlib.sha256(b"teamagents-job:" + token.encode()).hexdigest()[:32]

    def request(method):
        connection = socket.socket(socket.AF_UNIX)
        connection.settimeout(5)
        connection.connect("\0" + name)  # abstract namespace
        connection.sendall((json.dumps({"version": 1, "method": method, "token": token}) + "\n").encode())
        reply = json.loads(connection.recv(65536).decode())
        connection.close()
        return reply

    try:
        time.sleep(0.4)
        request("go")
        time.sleep(0.5)
        pid = runner.pid
        print(f"runner pid {pid}; one sleep 45 command; windows of 10 s each")

        start, end = cpu_ticks(pid), None
        time.sleep(10)
        end = cpu_ticks(pid)
        print(f"  tick only (no poller):        {end - start} ticks = {(end - start) / 1000 * 100:.2f}% of a core")

        for label, schedule in (("poll 25 ms (pre-D-116)", lambda t: 0.025),
                                ("poll 50 ms", lambda t: 0.050),
                                ("poll 50 ms then 250 ms (D-116)", lambda t: 0.05 if t < 1 else 0.25)):
            start, sent, window = cpu_ticks(pid), 0, time.time()
            until = window + 10
            while time.time() < until:
                request("status")
                sent += 1
                time.sleep(schedule(time.time() - window))
            end = cpu_ticks(pid)
            seconds = (end - start) / 100
            print(f"  {label:30} {sent:4d} requests, {end - start} ticks = "
                  f"{seconds / 10 * 100:.2f}% of a core ({seconds / sent * 1e6:.0f} µs per request)")

        # The state every abandoned job leaves behind: no child, no client, nothing to notice on a timer. The
        # loop only needs to answer a client that connects (and, since D-153, to notice that its job directory
        # is gone), so this window is the price of a *leaked* runner — the shape measured at 1,397 processes on
        # one machine after two days.
        for _ in range(200):
            if json.loads((root / "journal.json").read_text())["state"] not in ("RUNNING", "CANCEL_REQUESTED"):
                break
            time.sleep(0.25)
        start = cpu_ticks(pid)
        time.sleep(10)
        idle = cpu_ticks(pid) - start
        print(f"  {'idle, no child (D-153)':30}     — requests, {idle} ticks = {idle / 1000 * 100:.2f}% of a core "
              f"(a leaked runner pays this forever)")
    finally:
        runner.terminate()
        runner.wait(timeout=10)
        shutil.rmtree(root, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())

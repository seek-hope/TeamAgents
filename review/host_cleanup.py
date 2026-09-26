#!/usr/bin/env python3
"""The host's leftover runners and session daemons: the inventory a cleanup decision rests on (D-189).

`teamagents jobs-runner <job dir>` serves one shell command (§6.2). D-112 made a settled job's runner retire
itself, and D-153 made an idle one cost nothing and exit once its job directory disappears — but processes
started by *earlier* builds keep ticking. Measured 2026-09-27 on this machine: **1,398** of them, **1,397
orphaned** (reparented to init), the youngest 6.5 h old and none younger, so none came from the current build;
together they burned **12.8 cores continuously** (64.08 CPU-seconds per five-second sample), held **5.2 GiB**
resident, and had accumulated **664 CPU-hours**. That is what makes every wall-clock measurement on this
machine an upper bound (D-188).

    python3 review/host_cleanup.py                            # census, classes, burn rate
    python3 review/host_cleanup.py --json                     # the same, machine-readable
    python3 review/host_cleanup.py --class dir-gone --pids    # one class's pids, for a per-pid stop

What a leftover runner *serves* decides its class, never its age: `settled-journal` (the journal records a
finished command with its exit code), `unknown-outcome` (finished, but the outcome cannot be verified),
`dir-gone` (the job directory is gone), `unfinished` (no finish on disk — its command may still be running), and
`live-parent` (its parent is alive, i.e. it belongs to a live tree — on this machine the user's own session, so
it is never a candidate). Nothing here signals anything: the acting step is one `kill <pid>` per pid, taken from
a class list, on the operator's word (D-189 records the procedure and the reason no pattern kill is used).

Caveat: a sandboxed shell sees only its own PID namespace, where `review/leak_guard.py` — which counts runners
inside the tests' namespace — correctly reports zero while the host carries thousands. Run this **outside** the
sandbox; inside one it says so and prints nothing useful.
"""
import argparse
import json
import os
import pathlib
import statistics
import sys
import time

SETTLED_STATES = {"SUCCEEDED", "FAILED", "EXITED", "DONE"}
TERMINAL_STATES = SETTLED_STATES | {"CANCELLED", "CANCELED", "OUTCOME_UNKNOWN"}
CLOCK_TICKS = os.sysconf("SC_CLK_TCK")


def visible_processes() -> int:
    return sum(1 for entry in pathlib.Path("/proc").iterdir() if entry.name.isdigit())


def host_threads() -> int:
    """The host's total thread count, from `/proc/loadavg`'s `running/total` field — namespace-independent."""
    try:
        field = pathlib.Path("/proc/loadavg").read_text().split()[3]
        return int(field.split("/")[1])
    except (OSError, IndexError, ValueError):
        return 0


def uptime() -> float:
    try:
        return float(pathlib.Path("/proc/uptime").read_text().split()[0])
    except (OSError, IndexError, ValueError):
        return 0.0


def processes(subcommand: str) -> list[dict]:
    """Every live `teamagents <subcommand> …` process, with what the disk says about what it serves.

    `jobs-runner` processes carry the job directory as their third argument; `daemon` processes carry the state
    root after `--state-root`.
    """
    out = []
    now = uptime()
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            argv = [chunk.decode("utf-8", "replace") for chunk in (entry / "cmdline").read_bytes().split(b"\0") if chunk]
            if len(argv) < 3 or argv[1] != subcommand:
                continue
            stat = (entry / "stat").read_text()
            fields = stat.rsplit(")", 1)[1].split()
            cpu_seconds = (int(fields[11]) + int(fields[12])) / CLOCK_TICKS
            started = int(fields[19]) / CLOCK_TICKS
            rss_kib = 0
            for line in (entry / "status").read_text().splitlines():
                if line.startswith("VmRSS:"):
                    rss_kib = int(line.split()[1])
                    break
        except (OSError, ValueError, IndexError):
            continue
        if subcommand == "jobs-runner":
            served = argv[2]
        else:
            root = argv[argv.index("--state-root") + 1] if "--state-root" in argv else ""
            served = root
        out.append(
            {
                "pid": int(entry.name),
                "state": fields[0],
                "parent": int(fields[1]),
                "age_s": round(now - started, 1),
                "cpu_s": round(cpu_seconds, 2),
                "rss_kib": rss_kib,
                "serves": served,
                "journal": journal_state(pathlib.Path(served)) if subcommand == "jobs-runner" else "",
            }
        )
    return out


def user_state_roots() -> list[pathlib.Path]:
    """The state roots a user's own session uses: `$XDG_STATE_HOME/teamagents` and `~/.local/state/teamagents`.

    A daemon serving one of these is somebody's live session, not a leftover, and the census says so — killing it
    would close a session the user may be looking at (D-189).
    """
    home = pathlib.Path(os.path.expanduser("~"))
    root = pathlib.Path(os.environ.get("XDG_STATE_HOME") or home / ".local/state")
    return [root / "teamagents", home / ".local/state/teamagents"]


def daemon_class(process: dict) -> str:
    served = pathlib.Path(process["serves"]) if process["serves"] else None
    if served is not None and any(served == root or root in served.parents for root in user_state_roots()):
        return "user-session"
    if served is None or not served.exists():
        return "root-gone"
    return "root-present"


def journal_state(job_dir: pathlib.Path) -> str:
    """What the job directory says: `dir-gone`, `no-journal`, `unreadable`, `unfinished:<state>` or
    `finished:<state>`.
    """
    if not job_dir.is_dir():
        return "dir-gone"
    journal = job_dir / "journal.json"
    if not journal.is_file():
        return "no-journal"
    try:
        data = json.loads(journal.read_text())
    except (OSError, ValueError):
        return "unreadable"
    state = str(data.get("state", "")).upper()
    finished = bool(data.get("finished_ms")) or data.get("exit_code") is not None
    return f"{'finished' if finished else 'unfinished'}:{state or 'unnamed'}"


def classify(process: dict) -> str:
    """The class a leftover runner is in, from what it serves — never from its age.

    `live-parent` comes first and means *leave it alone*: a runner whose parent is still alive belongs to a live
    tree, and on this machine that is the user's own session (measured 2026-09-27: one runner under
    `~/.local/state/teamagents/v2`, whose parent is the user's daemon).
    """
    if process["parent"] != 1:
        return "live-parent"
    journal = process["journal"]
    if journal == "dir-gone":
        return "dir-gone"
    if journal.startswith("unfinished:") or journal in ("no-journal", "unreadable"):
        return "unfinished"
    # finished on disk: a settled command says what happened, an unverifiable outcome does not
    state = journal.split(":", 1)[1]
    return "settled-journal" if state in SETTLED_STATES else "unknown-outcome"


def burn_rate(seconds: float) -> dict:
    """CPU-seconds burned by the whole runner family in `seconds`, sampled around a sleep."""
    first = {p["pid"]: p["cpu_s"] for p in processes("jobs-runner")}
    time.sleep(seconds)
    second = {p["pid"]: p["cpu_s"] for p in processes("jobs-runner")}
    burned = sum(cpu - first[pid] for pid, cpu in second.items() if pid in first and cpu >= first[pid])
    active = sum(1 for pid, cpu in second.items() if pid in first and cpu - first[pid] > 0.05)
    return {"sample_seconds": seconds, "cpu_seconds": round(burned, 2),
            "cores": round(burned / seconds, 2), "active_processes": active}


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print the census as JSON")
    parser.add_argument("--class", dest="wanted", help="restrict the output to one class")
    parser.add_argument("--pids", action="store_true", help="print only the pids of that class")
    parser.add_argument("--sample-seconds", type=float, default=5.0, help="burn-rate window (0 skips it)")
    args = parser.parse_args(argv)
    visible, host = visible_processes(), host_threads()
    if host and visible * 10 < host:
        print(f"note: /proc holds {visible} process(es) while the host reports {host} thread(s): this is a "
              "sandboxed PID namespace, and the leftovers are not visible here — run outside the sandbox")
    found = processes("jobs-runner")
    for process in found:
        process["class"] = classify(process)
    daemons = processes("daemon")
    for process in daemons:
        process["class"] = daemon_class(process)
    if args.wanted:
        selected = [p for p in found if p["class"] == args.wanted]
        if args.pids:
            print("\n".join(str(p["pid"]) for p in selected))
            return 0
        found = selected
    ages = sorted(p["age_s"] for p in found)
    summary = {
        "runners": len(found),
        "orphaned": sum(1 for p in found if p["parent"] == 1),
        "classes": {name: sum(1 for p in found if p["class"] == name) for name in
                    ("settled-journal", "unknown-outcome", "dir-gone", "unfinished", "live-parent")},
        "journal_states": {state: sum(1 for p in found if p["journal"].endswith(state)) for state in sorted(
            {p["journal"].split(":", 1)[-1] for p in found})},
        "rss_mib": round(sum(p["rss_kib"] for p in found) / 1024),
        "cpu_hours_total": round(sum(p["cpu_s"] for p in found) / 3600, 1),
        "age_hours_min_median_max": [round(ages[0] / 3600, 1), round(statistics.median(ages) / 3600, 1),
                                     round(ages[-1] / 3600, 1)] if ages else [],
        "visible_processes": visible,
        "host_threads": host,
        "daemons": {
            "count": len(daemons),
            "classes": {name: sum(1 for d in daemons if d["class"] == name) for name in
                        ("user-session", "root-gone", "root-present")},
            "oldest_hours": round(max((d["age_s"] for d in daemons), default=0) / 3600, 1),
        },
    }
    if args.sample_seconds > 0 and found:
        summary["burn"] = burn_rate(args.sample_seconds)
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
        return 0
    print(f"leftover jobs-runner processes: {summary['runners']} "
          f"({summary['orphaned']} orphaned, reparented to init)")
    print(f"  classes: {summary['classes']}")
    print("  the conservative stop is the settled-journal and dir-gone set — and never `live-parent` (a live "
          "session's own runner) or `unfinished` (its command has no finish on disk); `unknown-outcome` "
          "(finished, unverifiable) is the operator's call (D-189)")
    print(f"  journals: {summary['journal_states']}")
    print(f"  resident: {summary['rss_mib']} MiB | burned so far: {summary['cpu_hours_total']} CPU-hour(s)")
    print(f"  age hours (min/median/max): {summary['age_hours_min_median_max']}")
    if "burn" in summary:
        burn = summary["burn"]
        print(f"  now: {burn['cores']} core(s) continuously ({burn['cpu_seconds']} CPU-s in "
              f"{burn['sample_seconds']}s, {burn['active_processes']} process(es) active)")
    daemon_line = summary["daemons"]
    print(f"live session daemons: {daemon_line['count']} {daemon_line['classes']} (oldest "
          f"{daemon_line['oldest_hours']} h) — `user-session` is somebody's live session and is never a "
          "leftover; `root-gone` is a daemon whose state root disappeared, which is the stuck shape")
    if not found:
        print("nothing to clean here (or this shell cannot see the host's processes — see the note above)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

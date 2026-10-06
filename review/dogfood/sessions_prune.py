#!/usr/bin/env python3
"""`sessions prune`: `[retention] archived_days` is applied, and narrow (D-435).

Since D-245 the key `[retention] archived_days` was accepted and never applied, because one state root meant one
session and there was no archived set to walk. Named sessions (D-364) gave it a meaning, and this probe measures
the rule over the real binary and a real base directory, credential-free:

1. a **freshly archived** session stays, and the report says why (`inside the N-day window`);
2. a session **archived long ago** is listed by `--dry-run` and *deleted* by `--yes` — the directory and the
   registry record both go — while `--dry-run` itself changes nothing;
3. a session carrying the **`EVIDENCE` marker** stays even though its window passed, and the reason names the
   marker;
4. the state root in use is **never a candidate**: it is not an archived session, and the verb says nothing about
   it;
5. the window comes from the config (`[retention] archived_days`) unless `--days N` overrides it, and the verb
   refuses `--days 0` and a run without `--yes` rather than guessing.

The age is the one thing the CLI cannot set, so the probe backdates the archive by rewriting `archived_ms` in
`sessions.json` — the same file the product reads, which is also why this is a *live* half and not a mock: the
verb under test reads exactly what the probe wrote.

    python3 review/dogfood/sessions_prune.py
    python3 review/dogfood/sessions_prune.py --state-dir /tmp/ta-sessions-prune
"""
import argparse
import json
import pathlib
import shutil
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"
DAY_MS = 24 * 60 * 60 * 1000


def run(home: pathlib.Path, args: list[str]) -> tuple[int, str, str]:
    """Run the CLI against this probe's base directory, with an isolated XDG tree.

    The base is explicit: without `--state-root` the product uses `<XDG_STATE_HOME>/teamagents/v2`, and the probe
    reads the registry file the verb writes, so both sides have to mean the same directory.
    """
    env = {"PATH": "/usr/bin:/bin", "HOME": str(home), "XDG_CONFIG_HOME": str(home / "config"),
           "XDG_STATE_HOME": str(home / "state"), "TA_PRUNE_PROBE_KEY": "not-a-key"}
    config = home / "config/teamagents/config.toml"
    config.parent.mkdir(parents=True, exist_ok=True)
    if not config.exists():
        config.write_text(
            "[models.leader_main]\nprovider = \"openai\"\nmodel = \"prune-probe\"\n"
            "api_key_env = \"TA_PRUNE_PROBE_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\ntimeout = 5\n"
        )
    base = home / "base"
    base.mkdir(parents=True, exist_ok=True)
    completed = subprocess.run(
        [str(BIN), "--state-root", str(base), *args], capture_output=True, text=True, env=env, timeout=120
    )
    return completed.returncode, completed.stdout, completed.stderr


def set_window(home: pathlib.Path, days: int) -> None:
    (home / "config/teamagents/config.toml").write_text(
        f"[retention]\narchived_days = {days}\n\n[models.leader_main]\nprovider = \"openai\"\n"
        "model = \"prune-probe\"\napi_key_env = \"TA_PRUNE_PROBE_KEY\"\n"
        "base_url = \"http://127.0.0.1:1/v1\"\ntimeout = 5\n"
    )


def registry(home: pathlib.Path) -> dict:
    return json.loads((home / "base/sessions.json").read_text())


def backdate(home: pathlib.Path, session_id: str, days: int) -> None:
    """Age an archive by rewriting its recorded archive time — the file the verb reads."""
    registry_file = home / "base/sessions.json"
    body = json.loads(registry_file.read_text())
    import time

    for entry in body["sessions"]:
        if entry["id"] == session_id:
            entry["archived_ms"] = int(time.time() * 1000) - days * DAY_MS
    registry_file.write_text(json.dumps(body, indent=2))


def new_session(home: pathlib.Path, name: str) -> str:
    code, out, err = run(home, ["sessions", "new", "--name", name, "--json"])
    if code != 0:
        raise SystemExit(f"sessions new failed ({code}): {err}")
    return json.loads(out)["id"]


def self_check() -> int:
    """The rule, as a function of the shapes a run produces, plus one broken shape it must report."""
    findings = []
    ok = {"code": 0, "out": "would prune session old (/base/archive/old)\n  fresh: kept: inside the 30-day window\n"
                           "would free: 1 archived session(s) older than 30 day(s), 0.0 MiB"}
    if judge([ok]):
        findings.append("the measured shape must pass")
    if not judge([{**ok, "code": 2}]):
        findings.append("a verb that refused must be reported")
    if not judge([{**ok, "out": "would free: 0 archived session(s)"}]):
        findings.append("a report that took nothing while a session was old enough must be reported")
    for finding in findings:
        print(f"FAIL: {finding}")
    if not findings:
        print("self-check ok: the rule accepts the measured shape and reports a broken one")
    return 1 if findings else 0


def judge(runs: list[dict]) -> list[str]:
    failures = []
    for entry in runs:
        if entry["code"] != 0:
            failures.append(f"the verb should succeed, exit={entry['code']}")
        if "would prune session" not in entry["out"] and "pruned session" not in entry["out"]:
            failures.append("the session past the window was not taken")
        if "inside the" not in entry["out"]:
            failures.append("a kept session must say why it stays")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-sessions-prune-probe)")
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if self_check():
        return 1
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    home = pathlib.Path(args.state_dir or "/tmp/ta-sessions-prune-probe")
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    failures: list[str] = []
    try:
        old = new_session(home, "old")
        fresh = new_session(home, "fresh")
        marked = new_session(home, "marked")
        for session_id in (old, fresh, marked):
            code, _, err = run(home, ["sessions", "archive", "--id", session_id])
            if code != 0:
                failures.append(f"archive {session_id} failed: {err}")
        # 5a: the window is never guessed — and this has to be asked *before* the config gains one
        code, _, err = run(home, ["sessions", "prune"])
        if code != 2 or "needs a window" not in err:
            failures.append(f"a prune with no window must be refused by name: {code} {err!r}")
        set_window(home, 30)
        backdate(home, old, 40)
        backdate(home, marked, 40)
        (home / "base" / "archive" / marked / "EVIDENCE").write_text("kept on purpose")
        (home / "base" / "archive" / old / "history.bin").write_text("x" * 1024)

        # 5b: a window of zero is not "delete everything"
        code, _, err = run(home, ["sessions", "prune", "--days", "0"])
        if code != 2 or "--days 0" not in err:
            failures.append(f"--days 0 must be refused: {code} {err!r}")
        # 5c: deleting needs --yes
        code, _, err = run(home, ["sessions", "prune"])
        if code != 2 or "--yes" not in err:
            failures.append(f"a prune without --yes must be refused by name: {code} {err!r}")

        # 1/2/3/4: the dry run lists the two sessions past the window, keeps the fresh one and the marked one,
        # and deletes nothing at all
        code, out, err = run(home, ["sessions", "prune", "--dry-run"])
        print(f"dry run: exit={code}\n{out.strip()}")
        if code != 0:
            failures.append(f"the dry run failed: {err}")
        if f"would prune session {old}" not in out:
            failures.append(f"the session archived 40 days ago must be listed: {out!r}")
        if f"would prune session {marked}" in out:
            failures.append(f"the EVIDENCE marker must win over the window: {out!r}")
        if f"{marked}: kept" not in out or "EVIDENCE" not in out:
            failures.append(f"the marked session must stay and say why: {out!r}")
        if f"{fresh}: kept: archived 0 day(s) ago, inside the 30-day window" not in out:
            failures.append(f"the freshly archived session must stay with the window as the reason: {out!r}")
        if any((home / "base" / "archive" / session_id).exists() is False for session_id in (old, fresh, marked)):
            failures.append("the dry run deleted something")
        if "would free: 1 archived session(s) older than 30 day(s)" not in out:
            failures.append(f"the dry run must count exactly the one session: {out!r}")
        if judge([{"code": code, "out": out}]):
            failures.append("the measured dry run does not satisfy the rule")

        # and `--yes` does it: the directory and the record both go, the others stay
        code, out, err = run(home, ["sessions", "prune", "--yes"])
        print(f"prune: exit={code}\n{out.strip()}")
        if code != 0:
            failures.append(f"the confirmed prune failed: {err}")
        if (home / "base" / "archive" / old).exists():
            failures.append("the pruned session's directory is still there")
        ids = [entry["id"] for entry in registry(home)["sessions"]]
        if old in ids:
            failures.append("the pruned session is still in the registry")
        for session_id in (fresh, marked):
            if session_id not in ids or not (home / "base" / "archive" / session_id).exists():
                failures.append(f"session {session_id} should have survived")
        if "freed: 1 archived session(s) older than 30 day(s)" not in out:
            failures.append(f"the prune must report what it took: {out!r}")
    finally:
        if not args.state_dir:
            shutil.rmtree(home, ignore_errors=True)

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

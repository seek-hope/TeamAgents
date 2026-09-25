#!/usr/bin/env python3
"""A real-model check that the bound web tools work and their guard holds (A01/A25-adjacent).

Two turns in one session, with a `[tools.fetch]` binding (the credential-free half of the web
tools; `web_search` needs an API key and is covered by its own unit tests):

1. the model fetches a public page (`https://example.com`) and reports a phrase from it — the
   readable body really reaches the conversation;
2. the model is asked to fetch a **private address** and to report the tool's own answer: the
   runtime must refuse it (`refusing private address for 127.0.0.1`), because a bound fetch tool
   is otherwise an SSRF lever into the host's own network.

    python3 review/dogfood/web.py

It is a real-model check: it needs `DEEPSEEK_API_KEY`, uses the model's native window (D-36),
writes only under `--state-dir`, needs network for `example.com`, and is not part of `make check`.
"""
import argparse
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
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

PUBLIC = "Use the `web_fetch` tool on https://example.com and then reply with the page's title line."
PRIVATE = (
    "This is a test of the `web_fetch` tool's guard. Call `web_fetch` with exactly the url "
    "http://127.0.0.1:9/ and then report the tool's own answer verbatim."
)


def turn(state_root: pathlib.Path, workspace: pathlib.Path, env: dict, prompt: str, timeout: int):
    done = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(timeout), "--cwd", str(workspace), prompt],
        capture_output=True, text=True, env=env,
    )
    report = json.loads(done.stdout) if done.stdout.strip().startswith("{") else {}
    return done, report


def entries_of(state_root: pathlib.Path) -> list[str]:
    db = sqlite3.connect(state_root / "session.sqlite")
    return [row[0] for row in db.execute(
        "SELECT message_json FROM context_entries WHERE instance_id = 'i-leader' ORDER BY idx")]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-web)")
    parser.add_argument("--timeout", type=int, default=300, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-web-{args.provider}")
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    (root / "config/teamagents").mkdir(parents=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(
        "# the credential-free half of the web tools: fetching needs no API key (D-78 reports it)\n"
        'skills_paths = []\n\n[tools.fetch]\nkind = "web_fetch"\n\n' + MODELS[args.provider]
    )
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    started = time.time()
    done, report = turn(state_root, workspace, env, PUBLIC, args.timeout)
    print(f"provider={args.provider} turn 1 exit={done.returncode} {round(time.time() - started, 1)}s "
          f"end={report.get('end')} reply={str(report.get('reply'))[:80]!r}")
    entries = entries_of(state_root)
    fetched = [entry for entry in entries if "Example Domain" in entry]
    if not fetched:
        failures.append("the fetched page's text never reached the conversation")
    else:
        print("  the public page's body is in the conversation")
    if not any('"web_fetch"' in entry for entry in entries):
        failures.append("no `web_fetch` call is in the conversation")
    if report.get("end") not in {"reply", "completed"} or done.returncode != 0:
        failures.append(f"turn 1 ended {report.get('end')!r} (exit {done.returncode})")

    done, report = turn(state_root, workspace, env, PRIVATE, args.timeout)
    print(f"provider={args.provider} turn 2 exit={done.returncode} end={report.get('end')} "
          f"reply={str(report.get('reply'))[:80]!r}")
    entries = entries_of(state_root)
    refused = [entry for entry in entries if "refusing private address" in entry]
    if not refused:
        failures.append("the private-address fetch was not refused by the guard "
                        "(no 'refusing private address' in the conversation)")
    else:
        print(f"  the guard refused the private address: {refused[-1][:150]}")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

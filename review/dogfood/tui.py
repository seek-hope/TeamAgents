#!/usr/bin/env python3
"""A real-model check of the product's headline path: the TUI on a real session (A01/§9).

Every other harness here drives `exec`. This one drives what a user opens first — the TUI — against a
real daemon and a real model, in a real PTY:

1. `teamagents --state-root … --cwd …` starts the daemon and attaches the TUI;
2. the probe types a prompt and presses Enter;
3. it waits for the model's answer to appear **on screen** (history comes back through the daemon after the
   turn, which is the client half nothing else exercises with a model);
4. it checks the session's own database for the same facts (the input is an entry, the answer is an entry)
   and then quits.

The screen needles are the ones the *renderer* produces, not the raw words: the history pane labels an
entry with its author (`v2ui.rs`), so the answer has to appear as `<instance> <answer>` and the typed input
as `you <input>`. A bare word would also match the composer echo of the prompt, which is on screen before
the turn even starts — that is why the first version of this probe "passed" the answer within a second.

    python3 review/dogfood/tui.py                    # DeepSeek
    python3 review/dogfood/tui.py --provider kimi    # over `responses`

It is a real-model check: it needs `DEEPSEEK_API_KEY` (or `KIMI_API_KEY`), uses the model's native window
(D-36), writes only under `--state-dir`, and is not part of `make check` (`make pty` covers the TUI itself
against a fake daemon; this covers the whole stack with a model).
"""
import argparse
import atexit
import json
import os
import pathlib
import pty
import re
import shutil
import sqlite3
import struct
import subprocess
import sys
import termios
import time

import fcntl

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
BIN = REPO / "engine/target/debug/teamagents"
sys.path.insert(0, str(REPO / "tui" / "scripts"))
from pty_screen import Screen, read_all  # noqa: E402  (the smoke's virtual terminal)

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

PROMPT = "Reply with the single word TUIDONE and nothing else."
ANSWER = "TUIDONE"
# what the history pane paints once the daemon sends the turn's entries back
ANSWER_NEEDLE = f"i-leader {ANSWER}"
INPUT_NEEDLE = "you Reply with the single word"


def entries(root: pathlib.Path) -> list[dict]:
    db = sqlite3.connect(f"file:{root / 'session.sqlite'}?mode=ro", uri=True)
    return [json.loads(row[0]) for row in db.execute(
        "SELECT message_json FROM context_entries WHERE instance_id = 'i-leader' ORDER BY idx")]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-tui)")
    parser.add_argument("--timeout", type=int, default=180, help="seconds to wait for the answer")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-tui-{args.provider}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    state_root = root / "root"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text("skills_paths = []\n\n" + MODELS[args.provider])
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "TERM": "xterm-256color"}
    failures: list[str] = []

    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(str(BIN), [str(BIN), "--state-root", str(state_root), "--cwd", str(workspace)], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 100, 0, 0))
    screen = Screen(100, 40)

    def painted() -> str:
        return "\n".join(screen.lines())

    # a drain step well below the timings being reported: `read_all` only returns when its own window
    # closes, so a second-long step would quantize every measurement below to whole seconds
    def wait_for(needle: str, timeout: float, label: str, step: float = 0.25) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            chunk = read_all(fd, min(step, max(0.05, deadline - time.time()))).decode("utf-8", "replace")
            if chunk:
                screen.feed(chunk)
            if needle in painted():
                return True
        failures.append(f"{label}: {needle!r} never appeared on screen")
        return False

    def pump(seconds: float) -> None:
        """Drain the terminal for `seconds` so the screen reflects what was typed (frames are diffed)."""
        chunk = read_all(fd, seconds).decode("utf-8", "replace")
        if chunk:
            screen.feed(chunk)

    started = time.time()
    # 1. the TUI attached to a real daemon: the status line names the session and the leader
    if not wait_for("i-leader", 60, "startup"):
        print(painted()[-800:])
        os.kill(pid, 9)
        for failure in failures:
            print("FAIL:", failure)
        return 1
    print(f"  the TUI attached to the session ({round(time.time() - started, 1)}s)")

    # 2. type the prompt and send it
    typed_at = time.time()
    os.write(fd, PROMPT.encode())
    # positive control for the needles: while the prompt only sits in the composer, the bare answer word is
    # already on screen (it is part of the instruction) but the labelled answer is not. If the composer echo
    # were absent this run would prove nothing about the screen check, so say so instead of passing quietly.
    if wait_for(ANSWER, 20, "positive control: the typed prompt is echoed on screen", step=0.1):
        print(f"  the composer echoes what was typed ({round(time.time() - typed_at, 1)}s)")
    if ANSWER_NEEDLE in painted():
        failures.append("the labelled answer is on screen before the turn started")
    sent = time.time()
    os.write(fd, b"\r")
    print(f"  sent the prompt from the composer")

    # 3. the answer has to come back through the daemon and land on screen, labelled by its author
    answered = wait_for(ANSWER_NEEDLE, args.timeout, "answer")
    elapsed = round(time.time() - sent, 1)
    if answered:
        print(f"  the answer is on screen as {ANSWER_NEEDLE!r} after {elapsed}s")
        if INPUT_NEEDLE not in painted():
            failures.append(f"the input never entered the history pane ({INPUT_NEEDLE!r})")
        else:
            print(f"  the input is in the history pane as {INPUT_NEEDLE!r}")
    else:
        print(painted()[-1200:])

    # 4. the session agrees: the input and the answer are context entries
    time.sleep(1.0)
    context = entries(state_root)
    roles = [e.get("role") for e in context]
    if not any(e.get("role") == "user" and PROMPT in e.get("content", "") for e in context):
        failures.append("the input is not a user entry in the session")
    if not any(e.get("role") == "assistant" and e.get("content", "").strip() == ANSWER for e in context):
        failures.append(f"the model's answer is not an assistant entry in the session ({roles})")

    # 5. quit the TUI (Ctrl+C) and stop the daemon this run started
    os.write(fd, b"\x03")
    time.sleep(1.0)
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass
    leak_guard.stop_daemons(state_root)
    print(f"  total {round(time.time() - started, 1)}s")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

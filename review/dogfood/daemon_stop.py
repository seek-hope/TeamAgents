#!/usr/bin/env python3
"""The session stop lever, hermetically (D-248).

`teamagents daemon --stop` stops the session that owns a state root, addressed by that root's socket: the socket
*is* the identity, so there is no pid file, no pid lookup and no "is that still the process I meant" question —
the shape `docs/ACCEPTANCE.md`'s known gap asked for, and the lever `docs/USER-GUIDE.md` §1 now offers. This
probe drives the real binary and pins the five shapes the command can meet:

1. **nothing is running** — `no daemon is running`, exit 0: stopping what is not running is an answer, not a
   failure;
2. **the starting shape's flags are refused by name** (`--cwd`, `--model`, `--full-auto`, exit 2), and a refused
   stop starts nothing;
3. **a running session is stopped** — and not just its listener: the socket goes *and the process exits* with
   the designed shutdown's `stopping...` (D-150). The first wiring of the lever answered the client and removed
   the socket while the process sat in its signal wait forever (measured 2026-09-27), which is why this probe
   checks the process and not only the socket;
4. **stopping again** is still an answer (`no daemon is running`, 0);
5. **a socket a crashed daemon left** behind is answered as `nothing is listening` (0) — and the lever never
   removes that socket, because the next client that *starts* a daemon is what replaces it.

It needs no credentials and no network: the profile it configures points at a closed port, and no turn is ever
submitted.

    python3 review/dogfood/daemon_stop.py
    python3 review/dogfood/daemon_stop.py --state-dir /tmp/ta-daemon-stop
"""
import argparse
import atexit
import os
import pathlib
import shutil
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop (D-148)
import leak_guard  # noqa: E402

BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_STOP_PROBE_KEY"
# The profile points at a closed port: the daemon resolves the protocol at boot (DESIGN §7) and no turn is ever
# submitted, so this probe never touches the network and never needs a credential.
CONFIG = f"""# Stop-lever probe (D-248): a closed port, no credentials, no network, no turn.
skills_paths = []

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "stop-probe"
base_url = "http://127.0.0.1:1/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 5
"""


def stop(state_root: pathlib.Path, env: dict, extra: list | None = None) -> tuple[int, str, str]:
    """Run the lever against `state_root` and return `(exit code, stdout, stderr)`."""
    done = subprocess.run([str(BIN), "daemon", "--stop", *(extra or []), "--state-root", str(state_root)],
                          capture_output=True, text=True, env=env, timeout=60)
    return done.returncode, done.stdout, done.stderr


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-daemon-stop)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    root = pathlib.Path(args.state_dir or "/tmp/ta-daemon-stop")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    shutil.rmtree(root, ignore_errors=True)
    workspace, state_root = root / "ws", root / "root"
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "PATH": os.environ.get("PATH", "/usr/bin:/bin"), KEY_VAR: "not-a-key", "HOME": str(root)}
    atexit.register(leak_guard.stop_daemons, state_root)
    socket = state_root / "daemon.sock"
    failures: list[str] = []

    # 1. nothing is running yet: an answer, not a failure
    code, out, err = stop(state_root, env)
    print(f"1. no session: exit={code} {out.strip()!r}")
    if code != 0 or "no daemon is running" not in out:
        failures.append(f"a stop with no session must answer 0 and say so: exit={code} out={out!r} err={err!r}")

    # 2. a flag of the *starting* shape is named rather than ignored, and a refused stop starts nothing
    for flag, value in (("--cwd", ["/tmp"]), ("--model", ["leader_main"]), ("--full-auto", [])):
        code, out, err = stop(state_root, env, extra=[flag, *value])
        named = flag in err
        print(f"2. {flag} refused: exit={code} named={named}")
        if code != 2 or not named:
            failures.append(f"daemon --stop must refuse {flag} by name (exit 2): exit={code} err={err!r}")
        if socket.exists():
            failures.append(f"a refused stop started a session: {socket} exists")

    # 3. a real detached daemon, started the way §1 says and stopped by the lever
    daemon = subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, env=env)
    deadline = time.time() + 30
    while not socket.exists() and time.time() < deadline:
        time.sleep(0.1)
    print(f"3. the session is up: socket={socket.exists()} daemons={len(leak_guard.daemon_pids(state_root))}")
    if not socket.exists():
        failures.append("the daemon never listened, so the lever had nothing to reach")

    code, out, err = stop(state_root, env)
    print(f"3. stopped: exit={code} {out.strip()!r}")
    if code != 0 or "stopped the session" not in out:
        failures.append(f"the lever must stop the running session (0): exit={code} out={out!r} err={err!r}")
    if socket.exists():
        failures.append("the socket must go with the accept loop")
    # the process, not only the listener: the first wiring left the daemon in its signal wait (D-248)
    exited, timed_out = None, False
    try:
        exited = daemon.wait(timeout=30)
    except subprocess.TimeoutExpired:
        timed_out = True
        daemon.kill()
        daemon.wait()
    stderr_text = daemon.stderr.read() if daemon.stderr else ""
    print(f"3. the process ended: exit={exited} timed_out={timed_out} "
          f"stopping_note={'stopping...' in stderr_text}")
    if timed_out:
        failures.append("the daemon process outlived its own stop: the socket went but the session kept running")
    elif exited != 0 or "stopping..." not in stderr_text:
        failures.append(f"the stop must reach the designed shutdown (exit 0 with `stopping...`): "
                        f"exit={exited} stderr={stderr_text[-200:]!r}")
    if leak_guard.daemon_pids(state_root):
        failures.append(f"a daemon is still serving the state root: {leak_guard.daemon_pids(state_root)}")

    # 4. stopping again is still an answer
    code, out, _ = stop(state_root, env)
    print(f"4. stopped twice: exit={code} {out.strip()!r}")
    if code != 0 or "no daemon is running" not in out:
        failures.append(f"a second stop must answer 0 with `no daemon is running`: exit={code} out={out!r}")

    # 5. a socket a crashed daemon left behind: nothing is listening, the lever removes nothing, and the next
    #    client that starts a daemon is what replaces it
    socket.write_text("")
    code, out, _ = stop(state_root, env)
    print(f"5. a stale socket: exit={code} {out.strip()[:70]!r} still_there={socket.exists()}")
    if code != 0 or "nothing is listening" not in out:
        failures.append(f"a stale socket is not a failure: exit={code} out={out!r}")
    if not socket.exists():
        failures.append("the lever removed a socket it did not own: the next client that starts a daemon "
                        "replaces it, and a stop must never delete state it cannot verify")
    socket.unlink()

    left = (len(leak_guard.daemon_pids(state_root)), len(leak_guard.runner_pids(state_root)))
    print(f"6. cleaned up: daemons/runners left = {left}")
    if left != (0, 0):
        failures.append(f"the probe left processes behind: {left}")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

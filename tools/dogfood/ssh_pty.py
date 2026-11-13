#!/usr/bin/env python3
"""Run existing PTY smokes through an isolated loopback SSH service.

This needs ssh, ssh-keygen and sshd. It creates a temporary user-mode server,
one-use host/user keys and a high loopback port; it never edits system SSH
configuration or known_hosts. The remote commands run the existing nested-PTY
checks, so this is real SSH/process/terminal-environment evidence, not a remote
public host, graphical-terminal pixel test or sandbox boundary.

    python3 tools/dogfood/ssh_pty.py --out tools/tmp/ssh-pty-1
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parents[2]


def run(command, timeout=120):
    return subprocess.run(command, text=True, capture_output=True, timeout=timeout)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="new directory for logs and the JSON receipt")
    args = parser.parse_args(argv)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=False)
    for tool in ("ssh", "ssh-keygen", "sshd"):
        if shutil.which(tool) is None:
            raise SystemExit(f"missing required tool: {tool}")

    scratch = Path(tempfile.mkdtemp(prefix="ta-ssh-pty-"))
    server = None
    server_log = None
    report = {
        "schema": 1,
        "transport": "loopback SSH with a temporary user-mode sshd",
        "boundary": "nested PTY checks over local SSH; not a public-host or graphical-pixel matrix",
        "cases": [],
    }
    try:
        host = scratch / "host_key"
        user = scratch / "user_key"
        for path in (host, user):
            done = run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(path)])
            if done.returncode != 0:
                raise RuntimeError(f"ssh-keygen failed: {done.stderr.strip()}")
        shutil.copy2(str(user) + ".pub", scratch / "authorized_keys")
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        config = scratch / "sshd_config"
        config.write_text(
            f"""Port {port}
ListenAddress 127.0.0.1
HostKey {host}
PidFile {scratch / 'sshd.pid'}
AuthorizedKeysFile {scratch / 'authorized_keys'}
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PubkeyAuthentication yes
PermitRootLogin no
AllowTcpForwarding no
X11Forwarding no
PermitTunnel no
UseDNS no
LogLevel VERBOSE
"""
        )
        # StrictModes would reject the world-writable /tmp ancestor. This
        # isolated server accepts only the temporary key above and is removed
        # with it; no user or system authorization file is changed.
        server_log = (out / "sshd.log").open("w")
        server = subprocess.Popen(
            [shutil.which("sshd"), "-D", "-e", "-f", str(config)], stdout=server_log, stderr=server_log
        )
        for _ in range(100):
            if server.poll() is not None:
                raise RuntimeError(f"sshd exited {server.returncode}; see {out / 'sshd.log'}")
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                    break
            except OSError:
                time.sleep(0.05)
        else:
            raise RuntimeError("temporary sshd did not listen")

        for label, term in [("direct", "xterm-256color"), ("screen", "screen-256color")]:
            log = out / f"{label}.log"
            command = [
                "ssh",
                "-tt",
                "-i",
                str(user),
                "-p",
                str(port),
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "LogLevel=ERROR",
                f"{os.environ['USER']}@127.0.0.1",
                f"cd {REPO} && env TA_PTY_TERM={term} python3 tui/scripts/pty_v2_smoke.py && "
                f"env TA_PTY_TERM={term} python3 tui/scripts/pty_graphics_smoke.py",
            ]
            done = run(command)
            log.write_text(done.stdout + done.stderr)
            report["cases"].append({"label": label, "term": term, "exit": done.returncode, "log": log.name})
            if done.returncode != 0:
                (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
                print(f"FAIL: {label}; see {log}")
                return 1
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        return 0
    finally:
        if server is not None:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
        if server_log is not None:
            server_log.close()
        shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

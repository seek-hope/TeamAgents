#!/usr/bin/env python3
"""Install the current-source musl archive, then verify real isolated product paths; never publish."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from workflows import Fixture, ROOT


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    env = {**os.environ, "TMPDIR":"/var/tmp"}
    rehearsal = subprocess.run([sys.executable, "review/release_rehearsal.py", "--keep"], cwd=ROOT,
                               env=env, text=True, capture_output=True, timeout=1200)
    print(rehearsal.stdout, end="", flush=True)
    print(rehearsal.stderr, end="", file=sys.stderr, flush=True)
    assert rehearsal.returncode == 0
    scratch = Path(next(line.removeprefix("kept: ") for line in rehearsal.stdout.splitlines() if line.startswith("kept: ")))
    assert scratch.parent == Path("/var/tmp") and scratch.name.startswith("ta-release-rehearsal-")
    binary, tui = scratch / "bin/teamagents", scratch / "bin/teamagents-tui"
    try:
        manifest = scratch / "dist/SHA256SUMS"
        for line in manifest.read_text().splitlines():
            sha, name = line.split()
            path = scratch / "dist" / name
            assert path.parent == scratch / "dist" and digest(path) == sha
        print(json.dumps({"archive_sha256": {p.name:digest(p) for p in (scratch / "dist").iterdir() if p.is_file()},
                          "installed_binaries": {p.name:digest(p) for p in [binary,tui]},
                          "platform": subprocess.check_output(["uname","-sm"],text=True).strip()}), flush=True)
        with Fixture(binary, "installed-init-doctor", []) as fixture:
            path = fixture.dir / "config/teamagents/config.toml"
            before = path.read_bytes()
            fixture.run("version")
            fixture.run("init")
            fixture.run("doctor")
            assert path.read_bytes() == before
            fixture.data("sessions", "list")
        subprocess.run([sys.executable,"review/maturity-2026-10-11/workflows.py","--bin",str(binary)],cwd=ROOT,check=True)
        subprocess.run([sys.executable,"review/maturity-2026-10-11/terminal.py","--bin",str(binary),"--tui",str(tui)],cwd=ROOT,check=True)
        # Reuse existing real-daemon PTY probes with explicit installed binary injection. Clean environment:
        # no provider credential or user config can enter these no-model checks.
        smoke_env = {"PATH":os.environ["PATH"],"HOME":str(scratch / "home"),"TMPDIR":"/var/tmp",
                     "TERM":"xterm-256color","LANG":"C.UTF-8","TEAMAGENTS_TUI":str(tui)}
        for name in ["tui_reconnect", "tui_sessions"]:
            code = """import importlib.util,pathlib,sys
spec=importlib.util.spec_from_file_location('probe',sys.argv[1]); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
module.BIN=pathlib.Path(sys.argv[2]); module.TUI=pathlib.Path(sys.argv[3]); sys.argv=[sys.argv[1]]
raise SystemExit(module.main())
"""
            subprocess.run([sys.executable,"-c",code,str(ROOT / "review/dogfood" / (name+".py")),str(binary),str(tui)],
                           cwd=ROOT,env=smoke_env,check=True,timeout=90)
        retained = Path(tempfile.mkdtemp(prefix="teamagents-maturity-artifacts-",dir="/var/tmp"))
        for file in (scratch / "dist").iterdir():
            if file.is_file():
                shutil.copy2(file, retained / file.name)
        print(json.dumps({"retained_artifacts":str(retained),"published":False}),flush=True)
        print("PASS: installed archive init/config/doctor, five coding cases, PTY input/search/queue, reconnect and sessions",flush=True)
    finally:
        shutil.rmtree(scratch)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Exhaust the D-451 models and require each control to refute its named property."""
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
JAR = Path.home() / ".local/share/teamagents-verify/tla2tools.jar"


def main():
    subprocess.run(["make", "verify-tools"], cwd=ROOT, check=True)
    count = 0
    for prefix, module in [("inputqueue", "V2InputQueue"), ("displayworker", "V2DisplayWorker")]:
        for cfg in sorted((ROOT / "verification/tla").glob(f"MC_{prefix}*.cfg")):
            with tempfile.TemporaryDirectory(prefix="teamagents-tlc-", dir="/var/tmp") as scratch:
                command = ["java", "-Xmx1g", "-XX:+UseParallelGC", "-cp", str(JAR), "tlc2.TLC",
                           "-config", cfg.name, "-workers", "2", "-metadir", str(Path(scratch) / "states"), module + ".tla"]
                run = subprocess.run(command, cwd=cfg.parent, text=True, capture_output=True, timeout=120)
                output = run.stdout + run.stderr
                print(f"== {cfg.name}: exit {run.returncode} ==", flush=True)
                print(output, flush=True)
                assert "Warning:" not in output, "inconsistent action is not evidence"
                if cfg.name == f"MC_{prefix}.cfg":
                    assert run.returncode == 0 and "No error has been found" in output
                else:
                    props = cfg.read_text().split("INVARIANTS\n", 1)[1].split()
                    expected = [p for p in props if p != "TypeOK"]
                    assert len(expected) == 1
                    assert re.search(r"Invariant " + re.escape(expected[0]) + r" is violated", output), expected
                count += 1
    print(f"PASS: 2 positive configurations and {count - 2} intended refutations")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Record one local verification command with immutable source hashes and real exit status.

No model calls are made by this wrapper. The explicitly supplied command defines the probe.
Use a fresh name for every run; existing receipts are never overwritten.
"""
import argparse
import datetime
import hashlib
import gzip
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or not args.name.replace("-", "").isalnum():
        parser.error("provide a command and an alphanumeric/hyphen name")
    root = Path(__file__).resolve().parents[2]
    out = Path(__file__).resolve().parent / "receipts"
    out.mkdir(exist_ok=True)
    receipt = out / (args.name + ".json")
    stdout = out / (args.name + ".stdout")
    stderr = out / (args.name + ".stderr")
    assert not any(p.exists() for p in (receipt, stdout, stderr)), "use a fresh receipt name"
    # Hash actual compilation/probe inputs, including untracked candidate files, not build products.
    paths = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard"], cwd=root, text=True).splitlines()
    paths = sorted(set(p for p in paths if (p.startswith(("core/", "engine/", "tui/", "verification/tla/", "verification/kani/"))
                                          or p in ("Makefile", "rust-toolchain.toml") or p.endswith(".py"))
                       and not p.startswith(("review/eval/", "review/benchmark/"))))
    hashes = {p: hashlib.sha256((root / p).read_bytes()).hexdigest() for p in paths if (root / p).is_file()}
    execution_hashes = {}
    if args.cwd.resolve() != root:
        for path in sorted((args.cwd / "tui").rglob("*")):
            if path.is_file() and "target" not in path.parts:
                execution_hashes[str(path.relative_to(args.cwd))] = hashlib.sha256(path.read_bytes()).hexdigest()
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    timer = time.monotonic()
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    signal.signal(signal.SIGQUIT, signal.SIG_DFL)
    with stdout.open("x") as sout, stderr.open("x") as serr:
        result = subprocess.run(command, cwd=args.cwd, stdout=sout, stderr=serr)
    record = {"command": command, "cwd": str(args.cwd.resolve()), "started_at": started,
              "exit_code": result.returncode, "seconds": round(time.monotonic() - timer, 3),
              "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
              "source_sha256": hashes, "execution_source_sha256": execution_hashes, "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR"),
              "stdout": str(stdout.relative_to(root)), "stderr": str(stderr.relative_to(root)),
              "stdout_sha256": hashlib.sha256(stdout.read_bytes()).hexdigest(),
              "stderr_sha256": hashlib.sha256(stderr.read_bytes()).hexdigest()}
    receipt.write_text(json.dumps(record, indent=2) + "\n")
    # Preserve exact output bytes (including failing fixtures/whitespace) without treating logs as code.
    for stream in (stdout, stderr):
        with stream.with_suffix(stream.suffix + ".gz").open("xb") as packed:
            packed.write(gzip.compress(stream.read_bytes(), mtime=0))
        stream.unlink()
    print(json.dumps({"receipt": str(receipt), "exit_code": result.returncode, "seconds": record["seconds"]}))
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()

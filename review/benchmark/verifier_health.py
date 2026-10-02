#!/usr/bin/env python3
"""Which trials in a harbor jobs directory had a verifier that never ran?

A scored 0 is only about the agent when the verifier actually evaluated the workspace. On some task images the
verifier's own `test.sh` cannot install its tooling — `apt-get update` fails against a rotted mirror and then
`curl`/`uvx` are missing, so pytest never starts — and the trial is 0 for a reason the agent cannot influence.
Those trials must be named and excluded from any comparison rather than silently counted against the product
(the shape D-382 found twice: five of fifty-nine trials in one sample, six of two hundred and sixty-seven in
the full set).

    python3 review/benchmark/verifier_health.py /tmp/ta-harbor/jobs_full_n3 [/tmp/ta-harbor/jobs_other ...]

Prints one line per jobs directory and one per affected task, and exits 1 when any trial's verifier never ran,
so it can gate a comparison script. Reads only committed trial output; runs nothing.
"""
import glob
import json
import os
import sys

# The ways the verifier's own setup failed in this dataset: the apt index could not be fetched, or the tools
# `test.sh` installs are absent when pytest is invoked. Each marker is a line the harness prints, not a guess
# about the exit code (the verifier's exit code is 1 for a legitimate failing test too).
MARKERS = (
    "Unable to locate package curl",
    "curl: command not found",
    "uvx: command not found",
    "Could not handshake: Error in the certificate verification",
)


def audit(jobs: str) -> list[tuple[str, int, int]]:
    """`(task, trials, trials whose verifier never ran)` for every task under `jobs`."""
    per_task: dict[str, list[bool]] = {}
    for result in glob.glob(os.path.join(jobs, "*", "*", "result.json")):
        trial = os.path.dirname(result)
        with open(result, encoding="utf-8") as handle:
            payload = json.load(handle)
        task = payload.get("task_name", "").split("/")[-1] or os.path.basename(trial).split("__")[0]
        stdout = os.path.join(trial, "verifier", "test-stdout.txt")
        text = ""
        if os.path.exists(stdout):
            with open(stdout, errors="replace", encoding="utf-8") as handle:
                text = handle.read()
        per_task.setdefault(task, []).append(any(marker in text for marker in MARKERS))
    return sorted((task, len(flags), sum(flags)) for task, flags in per_task.items())


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__.strip().splitlines()[0])
        print("usage: verifier_health.py JOBS_DIR [...]")
        return 2
    broken = 0
    for jobs in argv:
        rows = audit(jobs)
        trials = sum(n for _, n, _ in rows)
        blind = sum(b for _, _, b in rows)
        print(f"{jobs}: {trials} trial(s), {blind} whose verifier never ran, {len(rows)} task(s)")
        for task, n, b in rows:
            if b:
                print(f"  EXCLUDE {task}: {b} of {n} trial(s) never evaluated")
        broken += blind
    # exit 1 when something must be excluded, so a comparison cannot ignore it by accident
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

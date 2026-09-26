#!/usr/bin/env python3
"""The acceptance ledger's headline numbers, kept equal to the suites that ran (D-178).

`docs/ACCEPTANCE.md` opens with "`make check` is green (core 100 / engine 220 / tui 33 test targets) and
`make pty` passes" — the precondition every item below rests on, dated by the line above it. Both numbers
rotted the same way D-176's probe counts did: they were true when written, nothing read them, and the suites
grew (engine 220 → 239, tui 33 → 35) while the sentence kept claiming the old state.

    python3 review/test_counts.py              # check (inside `make test`, after the suites)
    python3 review/test_counts.py --write      # update the numbers in the ledger

The counts are derived exactly, not by a text count of `#[test]`: the crates are asked for their test list
(`cargo test -- --list`, which runs no test body and needs the build the gate already has), so a `#[test]` in
prose or a `#[cfg]`-gated test cannot skew the number. The gate runs inside `make test` — the suites are built
there anyway — and *not* in `make hygiene`, whose audits must work on a tree that has not been built.

The *date* on that line is deliberately not the script's business: "checked on <date>" means the ledger was
reviewed, which is a human claim, so `--write` touches the numbers only.
"""
import argparse
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
LEDGER = REPO / "docs" / "ACCEPTANCE.md"
CRATES = ("core", "engine", "tui")
BASELINE = re.compile(r"`make check` is green \(core (\d+) / engine (\d+) / tui (\d+) test targets\)")


def counted() -> dict:
    """`{crate: tests}` from the crates' own test lists (exact, and no test body runs)."""
    counts = {}
    for crate in CRATES:
        try:
            out = subprocess.run(
                ["cargo", "test", "--offline", "--manifest-path", f"{crate}/Cargo.toml", "--", "--list"],
                cwd=REPO, capture_output=True, text=True, check=True).stdout
        except (subprocess.CalledProcessError, FileNotFoundError) as error:
            raise SystemExit(
                f"cannot list {crate}'s tests ({error}); run `make build` first, or `cargo test` by hand"
            ) from error
        counts[crate] = sum(1 for line in out.split("\n") if line.endswith(": test"))
    return counts


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="update the counts in docs/ACCEPTANCE.md")
    parser.add_argument("--ledger", default=str(LEDGER), help="the ledger to read (a copy is the control)")
    args = parser.parse_args(argv)
    ledger = pathlib.Path(args.ledger)
    text = ledger.read_text()
    match = BASELINE.search(text)
    counts = counted()
    if match is None:
        print(
            f"FAIL: {ledger.name} has no '`make check` is green (core N / engine N / tui N test targets)' "
            "baseline line; if the sentence changed on purpose, update this audit with it"
        )
        return 1
    documented = {crate: int(match.group(index + 1)) for index, crate in enumerate(CRATES)}
    if args.write:
        if documented == counts:
            print(f"docs/ACCEPTANCE.md already states the counts: {counts}")
            return 0
        line = f"`make check` is green (core {counts['core']} / engine {counts['engine']} / tui {counts['tui']} test targets)"
        ledger.write_text(text[:match.start()] + line + text[match.end():])
        print(f"docs/ACCEPTANCE.md counts updated: {documented} -> {counts}")
        return 0
    findings = [
        f"{crate}: the ledger says {documented[crate]}, the suite has {counts[crate]}"
        for crate in CRATES
        if documented[crate] != counts[crate]
    ]
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}")
        print("run `python3 review/test_counts.py --write` (and move the ledger's checked-on date if the review "
              "is re-dated)")
        return 1
    print(f"the ledger's counts match the suites: core {counts['core']} / engine {counts['engine']} / "
          f"tui {counts['tui']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

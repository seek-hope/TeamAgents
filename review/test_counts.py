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

**D-221 added the other two places the same numbers are stated.** The ledger is the one held to the suites, but
it is not the only document a reader trusts with them: `.github/release-notes.md` is the release body (the
workflow publishes it with `--notes-file`), and `AGENTS.md` is the file a contributor takes as the baseline to
judge a run against. Measured 2026-09-27: **both were stale, and each stale differently** — the release body
said `core 91 / engine 136 / tui 29` and the instruction file `core 100 / engine 220 / tui 33`, against the
ledger's `core 101 / engine 244 / tui 35`. One fact, three statements, one checked: the script now holds every
document in `STATEMENTS` to the ledger's numbers, so `--write` keeps all three current together.
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
# The documents that state the same three numbers and are held to the ledger's: the published release body and
# the repository's own instruction file. A document that drops the sentence fails too — deleting a statement is
# how the check would otherwise go quiet — and `--write` updates all of them at once.
STATEMENTS = (REPO / ".github" / "release-notes.md", REPO / "AGENTS.md")
COUNTS = re.compile(r"core (\d+) / engine (\d+) / tui (\d+)")


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
    parser.add_argument("--statement", action="append", default=[], metavar="PATH",
                        help="a document that states the counts, instead of the default two (a copy is the "
                             "control)")
    args = parser.parse_args(argv)
    ledger = pathlib.Path(args.ledger)
    statements = [pathlib.Path(p) for p in args.statement] or list(STATEMENTS)
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
        wanted = f"core {counts['core']} / engine {counts['engine']} / tui {counts['tui']}"
        changed = []
        if documented != counts:
            line = (f"`make check` is green (core {counts['core']} / engine {counts['engine']} / "
                    f"tui {counts['tui']} test targets)")
            ledger.write_text(text[:match.start()] + line + text[match.end():])
            changed.append(f"{LEDGER.name}: {documented} -> {counts}")
        for path in statements:
            other = path.read_text()
            if not COUNTS.search(other):
                continue
            updated = COUNTS.sub(lambda _: wanted, other, count=1)
            if updated != other:
                path.write_text(updated)
                changed.append(f"{path.name}: updated to {wanted}")
        print("; ".join(changed) if changed else f"the counts are current everywhere: {wanted}")
        return 0
    findings = [
        f"{crate}: the ledger says {documented[crate]}, the suite has {counts[crate]}"
        for crate in CRATES
        if documented[crate] != counts[crate]
    ]
    wanted = f"core {counts['core']} / engine {counts['engine']} / tui {counts['tui']}"
    for path in statements:
        stated = COUNTS.search(path.read_text())
        name = path.relative_to(REPO) if path.is_relative_to(REPO) else path
        if stated is None:
            findings.append(f"{name} no longer states the baseline counts: one fact with several statements is "
                            "the shape here, and a statement that disappears is a check that goes quiet "
                            "(D-221)")
        elif f"core {stated.group(1)} / engine {stated.group(2)} / tui {stated.group(3)}" != wanted:
            findings.append(f"{name} states core {stated.group(1)} / engine {stated.group(2)} / "
                            f"tui {stated.group(3)}, the ledger {wanted}: the release body and the instruction "
                            "file are read as the baseline too (D-221)")
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

#!/usr/bin/env python3
"""Hold the three documented format versions to the constants that define them.

`database schema N`, `socket protocol N` and `work policy N` are stated in a dozen tracked documents. Nothing
compared those statements with the code, so the schema number drifted: the constant said 14 while twelve
documents still said 13, and the migration that made the change was in the tree the whole time. The same
silence would let the socket protocol or the work policy drift next.

This audit reads the constants and fails when any tracked document states a different number. It also checks
the migration chain, because a bumped constant with a missing migration arm is the same defect one step later:
a store at the previous version would have no step to reach the current one.

    python3 tools/format_version.py            # audit the tracked tree
    python3 tools/format_version.py --self-check

It compares *stated numbers*. It does not prove that a migration preserves data, that an older writer is
refused, or that a client negotiates the version it claims.
"""
import argparse
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
MODELS = "core/src/v2/models.rs"
WIRE = "core/src/v2/wire.rs"
WORK = "core/src/v2/control/work.rs"
STORE = "core/src/v2/store.rs"

SCHEMA_CONST = re.compile(r"pub const V2_SCHEMA_VERSION: i64 = (\d+);")
WIRE_CONST = re.compile(r"pub const VERSION: u64 = (\d+);")
POLICY_CONST = re.compile(r'const POLICY_KEY: &str = "work_policy_v(\d+)";')
# A migration arm is a bare `N =>` inside the version loop; the catch-all arm is `other =>`.
ARM = re.compile(r"^\s+(\d+) =>", re.M)
# A *current* statement, not a migration edge: `schema 10→11` and `schema 13 -> 14` name two versions and are
# history. Excluding the arrow is what keeps a correct historical sentence from reading as drift. `(?!\d)` runs
# first so `\d+` cannot backtrack from "10" to "1" and then find no arrow in "0→11".
SCHEMA_DOC = re.compile(r"schema (\d+)(?!\d)(?!\s*(?:→|->))")
WIRE_DOC = re.compile(r"socket protocol (\d+)(?!\d)")
POLICY_DOC = re.compile(r"work policy (\d+)(?!\d)")


def read(relative: str) -> str:
    return (REPO / relative).read_text(encoding="utf-8")


def constants() -> dict:
    """`{name: version}` from the constants, or a refusal when one cannot be located."""
    found = {}
    for name, pattern, relative in (
        ("schema", SCHEMA_CONST, MODELS),
        ("socket protocol", WIRE_CONST, WIRE),
        ("work policy", POLICY_CONST, WORK),
    ):
        match = pattern.search(read(relative))
        if match is None:
            raise SystemExit(f"{relative} no longer declares the {name} version; update this audit with it")
        found[name] = int(match.group(1))
    return found


def migration_arms() -> set:
    """The numeric versions `store.rs` has a migration step for."""
    return {int(match) for match in ARM.findall(read(STORE))}


def chain_findings(schema: int, arms: set) -> list:
    """The constant must have both its own migration arm and no gap below it."""
    findings = []
    if schema > 1 and schema - 1 not in arms:
        findings.append(f"{STORE} has no migration arm {schema - 1}: a store at schema {schema - 1} has no "
                        f"step to reach {schema}, which {MODELS} now declares")
    missing = sorted(set(range(1, schema)) - arms)
    if missing:
        findings.append(f"{STORE} is missing migration arm(s) {missing}, so a store at an older version cannot "
                        f"walk forward to schema {schema}")
    return findings


def tracked_markdown() -> list:
    out = subprocess.run(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                         cwd=REPO, capture_output=True, text=True, check=True).stdout
    return [name for name in out.split("\0") if name.endswith(".md") and not name.startswith("tools/tmp/")]


def statement_findings(versions: dict, files: list) -> tuple:
    """`(statements_read, findings)`: every stated current version against its constant."""
    read_count, findings = 0, []
    for name in files:
        text = (REPO / name).read_text(encoding="utf-8")
        for line_number, line in enumerate(text.split("\n"), 1):
            for label, pattern in (("schema", SCHEMA_DOC), ("socket protocol", WIRE_DOC),
                                   ("work policy", POLICY_DOC)):
                for match in pattern.finditer(line):
                    read_count += 1
                    stated = int(match.group(1))
                    if stated != versions[label]:
                        findings.append(f"{name}:{line_number} says {label} {stated}, the code declares "
                                        f"{versions[label]}")
    return read_count, findings


def audit() -> int:
    versions = constants()
    files = tracked_markdown()
    statements, findings = statement_findings(versions, files)
    findings += chain_findings(versions["schema"], migration_arms())
    for label, value in versions.items():
        print(f"code declares {label} {value}")
    print(f"{statements} current-version statement(s) read across {len(files)} tracked Markdown file(s)")
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print("every stated format version matches the code, and the migration chain has no gap")
    return 0


def self_check() -> int:
    """The arrow rule and the drift rule must both fire, from fixtures."""
    assert [int(m.group(1)) for m in SCHEMA_DOC.finditer("the database schema 14 is current")] == [14]
    assert SCHEMA_DOC.findall("schema 10→11 are not proved") == [], "a migration edge is history, not drift"
    assert SCHEMA_DOC.findall("the schema 13 -> 14 step") == [], "an ASCII arrow is history too"
    assert SCHEMA_DOC.findall("migrations through schema 14 and cleanup") == ["14"]
    assert WIRE_DOC.findall("use socket protocol 2") == ["2"]
    assert WIRE_DOC.findall("stop protocol-1 and protocol-2 clients") == [], "a hyphenated form is not a claim"
    assert POLICY_DOC.findall("work policy 1 is separate") == ["1"]

    versions = {"schema": 14, "socket protocol": 2, "work policy": 1}
    scratch = pathlib.Path(REPO / "tools" / "tmp" / "format-version-fixture")
    scratch.mkdir(parents=True, exist_ok=True)
    (scratch / "agree.md").write_text("database schema 14, socket protocol 2, work policy 1\n")
    (scratch / "drift.md").write_text("database schema 13, socket protocol 2\n")
    try:
        agreed, agree_findings = statement_findings(versions, ["tools/tmp/format-version-fixture/agree.md"])
        assert agreed == 3 and agree_findings == [], f"an agreeing document must pass: {agree_findings}"
        drifted, drift_findings = statement_findings(versions, ["tools/tmp/format-version-fixture/drift.md"])
        assert drifted == 2 and len(drift_findings) == 1 and "says schema 13" in drift_findings[0], \
            f"drift must be reported: {drift_findings}"
    finally:
        for leftover in scratch.iterdir():
            leftover.unlink()
        scratch.rmdir()

    assert chain_findings(14, set(range(1, 14))) == [], "a complete chain passes"
    assert len(chain_findings(14, set(range(1, 13)))) == 2, "a missing newest arm and a gap both report"
    assert len(chain_findings(14, {1, 2, 5, 7, 8, 9, 10, 11, 12, 13})) == 1, "a middle gap reports once"
    print("format-version controls passed: drift, agreeing documents, arrows, and the migration chain")
    return 0


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args(argv)
    if args.self_check:
        return self_check()
    self_check()  # the controls run with the gate (citations.py's convention), not only on demand
    return audit()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

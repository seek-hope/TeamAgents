#!/usr/bin/env python3
"""Config keys this build accepts but never serves (the detector behind D-75 and D-102).

`deny_unknown_fields` makes the loader *know* every config field, so a key that the rest of the program never
reads is accepted in silence: parsing, the path validator and a `doctor` row are enough to look served. That
class produced three findings in this repository (`[permissions] mode`, `[retention]`, `models.*.codex_profile`
— D-75) and one more after the sweep, `instruction_files` (D-102, which turned out to be *promised* by a doctor
row that said "reach every member's prompt"). This script is that audit as a command:

    python3 review/config_keys.py [--list-known]

It reads the config structs out of `core/src/models.rs`, counts each field's mentions in the crates, and reports
the fields whose only mentions are declaration, parsing/merging/validation (`config.rs`), the doctor surface
(`cli.rs`) and the tests. A name that appears in none of the plumbing beyond those files has no read site.

Limits, deliberately stated: the check is name-based (a field reached through a differently named accessor
would be missed), it cannot tell "read for a report" from "read to act" — that is what the allowlist is for —
and the argv parser counts as plumbing (a *flag* that is only parsed for a removed verb, D-53's
`--history-days`, is therefore reported here as unserved — which is what it is).
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
CONFIG_STRUCTS = ("UserConfig", "ModelProfile", "ToolBinding", "Retention", "Hooks", "GoalLimits", "CheckSpec")
# declaration, parsing/merging/validation, the doctor surface and the argv parser: none of them *use* a key
PLUMBING = {"core/src/models.rs", "engine/src/config.rs", "engine/src/cli.rs", "engine/src/main.rs"}

# field -> why it is allowed to have no read site
KNOWN_UNSERVED = {
    "codex_profile": "D-75: refused at load — an external Codex profile is not part of this release",
    "instruction_files": "D-102: declared, validated and reported as not applied; the gap is in ACCEPTANCE",
    "archived_days": "D-75: reported as not applied; retention is a known gap in ACCEPTANCE",
    "history_days": "D-75: reported as not applied; retention is a known gap in ACCEPTANCE",
}


def config_fields() -> list[str]:
    fields, current = [], None
    for line in (REPO / "core/src/models.rs").read_text().splitlines():
        struct = re.match(r"pub struct (\w+)", line)
        if struct:
            current = struct.group(1)
            continue
        field = re.match(r"\s+pub (\w+):", line)
        if field and current in CONFIG_STRUCTS:
            fields.append(field.group(1))
    return fields


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list-known", action="store_true", help="also list the allowed unserved fields")
    args = parser.parse_args()
    roots = ["core/src", "engine/src", "tui/src"]
    files = [p for root in roots for p in (REPO / root).rglob("*.rs")]
    findings, allowed = [], []
    for field in config_fields():
        reads = []
        for path in files:
            rel = str(path.relative_to(REPO))
            if rel in PLUMBING:
                continue
            for number, line in enumerate(path.read_text().splitlines(), 1):
                if re.search(r"\b" + re.escape(field) + r"\b", line):
                    reads.append(f"{rel}:{number}")
        if reads:
            continue
        (allowed if field in KNOWN_UNSERVED else findings).append(field)
    print(f"{len(config_fields())} config fields scanned; {len(findings)} unserved, {len(allowed)} known")
    for field in findings:
        print(f"  unserved: {field} — accepts a value, no code outside the loader/doctor reads it")
    if args.list_known:
        for field in allowed:
            print(f"  known: {field} ({KNOWN_UNSERVED[field]})")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

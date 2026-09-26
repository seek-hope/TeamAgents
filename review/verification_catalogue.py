#!/usr/bin/env python3
"""The formal-verification material, against the targets that drive it and the report that counts it (D-185).

`verification/tla/` holds ten TLA+ modules and twenty-two configurations; `verification/REPORT.md` states what
runs ("all **11** configurations report `No error has been found`", "all **10** negative controls are
refuted") and every property is mapped to its spec in `verification/README.md`. What no audit looked at is
whether the material and the Makefile still agree: a `.cfg` added without a line in a `verify-model*` target is
verification material nothing runs, a target naming a `.cfg` or `.tla` that was renamed fails only when
somebody spends the four minutes the target costs, and the report's counts can drift from the lists they
describe.

    python3 review/verification_catalogue.py
    python3 review/verification_catalogue.py --makefile PATH --report PATH   # the control, on copies

What it checks, all against the `verify-model*` recipes: every `.cfg` and `.tla` a recipe names exists; every
`MC*.cfg` in the directory is named by some target (nothing unrunnable), and every `V2*.tla` module is the
spec of some target (no model outside the checked set); and the two counts the report states for
`verify-model-all` and `verify-model-counterexamples` equal the sizes of those lists. The counts are compared
with the report's own sentences rather than with a generated file, so a list that grows must edit the report.
`verification/README.md` is the property-by-spec mapping the report points a reader at, so every configuration
and every module on disk must be described there too: a new model enters the checked set with a paragraph
about what it models.

Ceiling: this is a *catalogue* audit — it says nothing about whether TLC would pass a configuration, only that
the material is driven and counted. The states, the times and the property names in the report are measurements
of a run, and re-running the targets is what keeps them honest.
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
TLA = "verification/tla"
KANI_SRC = "verification/kani/src"
TARGET = re.compile(r"^([A-Za-z0-9_.-]+):")
CFG = re.compile(r"\b(MC[A-Za-z0-9_]*\.cfg)\b")
SPEC = re.compile(r"\b(V2[A-Za-z0-9_]*\.tla)\b")


def target_recipes(makefile: str) -> dict[str, str]:
    """`{target: recipe text}` for every target in the Makefile."""
    out, target, lines = {}, None, []
    for line in makefile.split("\n"):
        header = TARGET.match(line)
        if header and not line.startswith("\t"):
            if target:
                out[target] = "\n".join(lines)
            target, lines = header.group(1), []
            continue
        if target and line.startswith("\t"):
            lines.append(line)
    if target:
        out[target] = "\n".join(lines)
    return out


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--makefile", default="Makefile")
    parser.add_argument("--report", default="verification/REPORT.md")
    parser.add_argument("--mapping", default="verification/README.md")
    args = parser.parse_args(argv)
    recipes = target_recipes((REPO / args.makefile).read_text(encoding="utf-8"))
    drivers = {name: body for name, body in recipes.items() if name.startswith("verify-model")}
    report = (REPO / args.report).read_text(encoding="utf-8")
    mapping = (REPO / args.mapping).read_text(encoding="utf-8")
    findings, notes = [], []
    if not drivers:
        findings.append(f"no `verify-model*` target in {args.makefile}: this audit's rule has stopped applying")
    named_cfgs = sorted({cfg for body in drivers.values() for cfg in CFG.findall(body)})
    named_specs = sorted({spec for body in drivers.values() for spec in SPEC.findall(body)})
    for cfg in named_cfgs:
        if not (REPO / TLA / cfg).is_file():
            findings.append(f"{args.makefile} runs a TLC configuration that does not exist: {TLA}/{cfg}")
    for spec in named_specs:
        if not (REPO / TLA / spec).is_file():
            findings.append(f"{args.makefile} names a specification that does not exist: {TLA}/{spec}")
    on_disk_cfgs = sorted(p.name for p in (REPO / TLA).glob("MC*.cfg"))
    orphans = [cfg for cfg in on_disk_cfgs if cfg not in named_cfgs]
    if orphans:
        findings.append(f"{TLA} holds configuration(s) no verify-model target runs: {', '.join(orphans)} — "
                        "verification material nothing checks")
    on_disk_specs = sorted(p.name for p in (REPO / TLA).glob("V2*.tla"))
    unchecked = [spec for spec in on_disk_specs if spec not in named_specs]
    if unchecked:
        findings.append(f"{TLA} holds module(s) no configuration is run against: {', '.join(unchecked)} — a "
                        "model outside the checked set is a claim without a run")
    undescribed = [name for name in on_disk_cfgs + on_disk_specs if name not in mapping]
    if undescribed:
        findings.append(f"{args.mapping} does not describe {', '.join(undescribed)}: it is the mapping the "
                        "report sends a reader to, so a configuration or module the checked set gains must be "
                        "described there")
    for target, phrase in (("verify-model-all", "configurations"), ("verify-model-counterexamples", "controls")):
        if target not in drivers:
            findings.append(f"no `{target}` target: the report's {phrase} count has nothing to compare with")
            continue
        driven = len({cfg for cfg in CFG.findall(drivers[target])})
        stated = re.search(rf"all \*\*(\d+)\*\* negative controls|\*\*all (\d+) negative controls\*\*", report) \
            if phrase == "controls" else re.search(r"all \*\*(\d+)\*\* configurations", report)
        if stated is None:
            findings.append(f"{args.report} no longer states the number of {phrase} `{target}` runs, so this "
                            "audit cannot compare it with the list in the Makefile")
            continue
        value = int(stated.group(1) or stated.group(2))
        if value != driven:
            findings.append(f"{args.report} says **{value}** {phrase}, `{target}` drives {driven}: re-run the "
                            "target and state what it did")
        else:
            notes.append(f"{target}: {driven} {phrase}, as the report says")
    # The Kani bullet has the same shape: the report quotes the run's own count, and the harnesses it counted
    # are `#[kani::proof]` functions in the crate the target builds.
    harnesses = sum(len(re.findall(r"^\s*#\[kani::proof\]", path.read_text(encoding="utf-8"), re.M))
                    for path in (REPO / KANI_SRC).rglob("*.rs"))
    quoted = re.findall(r"Complete - (\d+) successfully verified harnesses, 0 failures, (\d+) total", report)
    if not quoted:
        findings.append(f"{args.report} no longer quotes a green Kani run, so this audit cannot compare the "
                        "harness count with the proofs in the tree")
    for verified, total in quoted:
        if verified != total:
            findings.append(f"{args.report} quotes an internally inconsistent Kani line: {verified} verified "
                            f"but {total} total in `Complete - {verified} successfully verified harnesses, "
                            f"0 failures, {total} total`")
        if int(verified) != harnesses:
            findings.append(f"{args.report} quotes {verified} verified Kani harnesses, but {KANI_SRC} holds "
                            f"{harnesses} `#[kani::proof]` function(s): re-run `make verify-kani` and state "
                            "what it did")
    if quoted and all(int(v) == harnesses for v, _ in quoted):
        notes.append(f"verify-kani: {harnesses} harness(es), as the report says")
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"{len(named_cfgs)} configuration(s) and {len(named_specs)} module(s) are driven by "
          f"{len(drivers)} verify-model target(s); {len(on_disk_cfgs)} configuration(s) on disk, all named")
    for note in notes:
        print(f"note: {note}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

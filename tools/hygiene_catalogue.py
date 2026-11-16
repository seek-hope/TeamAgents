#!/usr/bin/env python3
"""Keep current development tools and their documented entry points aligned.

Every Python script invoked by the hygiene/test recipes must be named in
DEVELOPMENT.md. Every root-level tools/*.py must run from a Make target or have
an explicit HAND_RUN reason; stale or contradictory exemptions fail.
Probe-directory coverage is checked by dogfood/probes.py --self-check.
Use --makefile and --doc for isolated audit fixtures.
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
WALKED = ("hygiene", "test")
TARGET = re.compile(r"^([A-Za-z0-9_.-]+):")
INVOCATION = re.compile(r"python3\s+([^\s;|)'\"]+\.py)")

# Root-level audit scripts no make target runs, each with the reason it stays that way. An entry a target *does*
# run is a finding below, so this list cannot shelter a script that has since been wired up.
HAND_RUN = {
    "tools/host_cleanup.py": "reads the host's own process table (D-189): container PID namespaces can hide "
                             "processes and the probe changes nothing, so there is no gate result to check",
    "tools/install_check.py": "downloads and installs a published release (A36); requires network access and "
                               "explicit artifact/source-bound acceptance",
    "tools/runner_cost.py": "measures process CPU and elapsed time without asserting an acceptance threshold",
}


def recipe_scripts(makefile: str) -> list[tuple[str, str]]:
    """`(target, script)` for every `python3 <script>` a Makefile recipe invokes, in file order."""
    out, target = [], None
    for line in makefile.split("\n"):
        header = TARGET.match(line)
        if header and not line.startswith("\t"):
            target = header.group(1)
            continue
        if target and line.startswith("\t"):
            out.extend((target, script) for script in INVOCATION.findall(line))
    return out


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--makefile", default="Makefile")
    parser.add_argument("--doc", default="docs/DEVELOPMENT.md")
    args = parser.parse_args(argv)
    makefile = (REPO / args.makefile).read_text(encoding="utf-8")
    page = (REPO / args.doc).read_text(encoding="utf-8")
    invoked = recipe_scripts(makefile)
    findings, checked = [], 0
    for walked in WALKED:
        scripts = [script for target, script in invoked if target == walked]
        if not scripts:
            findings.append(f"no `{walked}` target runs a python script in {args.makefile}: this audit walks "
                            "the page through those two targets, so its rule has stopped applying")
            continue
        for script in scripts:
            checked += 1
            if script not in page and pathlib.Path(script).name not in page:
                findings.append(f"{args.doc} does not name `{script}`, which `make {walked}` runs: the page "
                                "walks a contributor through the gate, so a step added without a line here "
                                "leaves the page describing a smaller build than the one that runs (D-184)")
    named = set(re.findall(r"tools/[A-Za-z0-9_./-]+\.py", page))
    run = {script for _, script in invoked}
    # D-194: every audit script is either gated or listed as hand-run with its reason.
    audits = {str(path.relative_to(REPO)) for path in (REPO / "tools").glob("*.py")}
    for audit in sorted(audits):
        if audit in run:
            if audit in HAND_RUN:
                findings.append(f"{audit} is listed in HAND_RUN, but a make target runs it — drop the entry so "
                                "the exemption cannot outlive its reason")
            continue
        if audit in HAND_RUN:
            print(f"note: {audit} is run by hand: {HAND_RUN[audit]}")
        else:
            findings.append(f"{audit} is run by no make target and is not listed in HAND_RUN: an audit nobody "
                            "runs is a rule nobody enforces (D-193 found `tools/config_keys.py` that way)")
    for stale in sorted(set(HAND_RUN) - audits):
        findings.append(f"HAND_RUN lists {stale}, which is not a root-level `tools/*.py` audit anymore")
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"{checked} script(s) invoked by make {'/'.join(WALKED)}: every one is named in {args.doc}")
    by_hand = sorted(named - run)
    if by_hand:
        print(f"note: {len(by_hand)} script(s) the page names are run by hand (no target): "
              + ", ".join(by_hand))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

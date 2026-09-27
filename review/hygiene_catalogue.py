#!/usr/bin/env python3
"""The scripts `make check` runs, against the page that walks through them (the detector behind D-184/D-194).

Every documented surface here has a catalogue audit — the events, the protocol, the tools, the config keys, the
CLI flags, the TUI keys — and the audits themselves were the one surface nobody did. `docs/DEVELOPMENT.md` is
the page a contributor reads to learn what `make check` does; it walked through eleven of hygiene's twenty-two
audits and stopped, so ten (D-130's `dead_code`, D-134's `readme_zh`, D-135/136's `doc_flags`, D-137's
`requirement_trace`, D-145's `eval_manifests`, D-182's `eval_surface`, `exec_report`, `tui_keys`, `env_knobs`,
`project_config_claim`) ran without the page ever naming them (measured 2026-09-27). Nothing noticed, because
a script the page does not mention cannot fail a citation: the page is prose.

    python3 review/hygiene_catalogue.py
    python3 review/hygiene_catalogue.py --makefile PATH --doc PATH   # the control, on copies

The rule is the one the page's own sentences make: for the two targets it walks through (`check`'s `hygiene`
and `test`), every script a recipe invokes with `python3` must be named in `docs/DEVELOPMENT.md`. A walked
target that has disappeared is a finding rather than a silent pass. As a note — never a failure — the audit
prints the `review/*.py` the page names that no target runs: those are the probe and evaluation entry points,
which are meant to be run by hand.

**D-194 added the other half, and it is the half D-193 was found by.** The rule above says every script a
target *runs* is documented; nothing said every audit script is *run*. `review/config_keys.py` — the detector
behind four findings (D-75's three keys and D-102's `instruction_files`) — was run by no target at all, and the
page's own listing of hand-run scripts did not mention it either; only a person reading the directory could see
it. So every root-level `review/*.py` must now be either invoked by some make target or listed in `HAND_RUN`
with its reason, and a `HAND_RUN` entry that a target *does* run is a finding too, so an exemption cannot
outlive its reason. The two deeper levels were already covered where they live: `review/dogfood/*.py` by
`probes.py --self-check`, which fails on a file in neither set nor `NOT_PROBES` (D-155), and `review/eval/**`
is frozen material whose driver is run by hand by design.
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
    "review/host_cleanup.py": "reads the host's own process table (D-189): it reports nothing from a sandbox "
                             "and changes nothing, so there is no gate result to check",
    "review/install_check.py": "downloads the published release and installs it (A36): network and an external "
                               "artifact, run by hand and dated in the acceptance row",
    "review/runner_cost.py": "a one-off cost meter (D-153) whose numbers A12 records; it measures a process, it "
                             "does not assert a product property",
    "review/codex_surface.py": "asks the local `codex` binary what it offers (D-209): the comparison's Codex "
                               "column is a statement about that binary, and CI has no Codex to ask",
    "review/comparison_sources.py": "re-derives the comparison's Pi and Hermes halves from their upstream READMEs, "
                                    "docs index, file tree and docs page (D-209): it needs the network",
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
    named = set(re.findall(r"review/[A-Za-z0-9_./-]+\.py", page))
    run = {script for _, script in invoked}
    # D-194: every audit script is either gated or listed as hand-run with its reason.
    audits = {str(path.relative_to(REPO)) for path in (REPO / "review").glob("*.py")}
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
                            "runs is a rule nobody enforces (D-193 found `review/config_keys.py` that way)")
    for stale in sorted(set(HAND_RUN) - audits):
        findings.append(f"HAND_RUN lists {stale}, which is not a root-level `review/*.py` audit anymore")
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

#!/usr/bin/env python3
"""The formal-verification material, against the targets that drive it and the report that counts it (D-185).

`verification/tla/` holds seventeen TLA+ modules and seventy-three configurations — and **these numbers are
checked against the directory by this script's own rule**, because the sentence that said "forty" was the
kind of count nothing looked at (the module count in this line and the two the report states are all
compared with what the tree holds); `verification/REPORT.md`
states what runs ("all **16** configurations report `No error has been found`", "all **30** negative
controls are refuted") and every property is mapped to its spec in `verification/README.md`. What no audit looked at is
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

**D-212 added the two halves of one question about the checked set.** Every configuration a `verify-model*` recipe
runs must be mapped to its module *explicitly* by that recipe (a `case` arm, a `cfg:spec` pair or a single-spec
target): the small configurations ride a `case` whose default arm would silently run the wrong module, and
`MC_artifact.cfg` was doing exactly that. And every name a module marks between its `invariants --` and
`properties --` comment markers must be listed by some configuration that runs the module, because a marked claim
that nothing lists is a claim nothing checks — `V2Compress`' `RequestClosesOnce` was one (it holds, measured, but
nothing would have noticed if it did not). The convention the second half implies: a claim that composes others is
listed itself and its components beside it (V2Grants' four `TypeOK*`, V2Store's `RefusalIsSilent`), so each marked
name is visibly checked.

**D-220 added the rule that every module has to be *refutable*.** A module that no counterexample configuration
**D-222 added the rule that the mapping has to name every claim.** The report sends a reader to
`verification/README.md` for what a module proves, and ten of the 148 marked claims were absent from it (measured
2026-09-27): all four of `V2Retention`'s substantive claims (the rule itself, and the three state halves),
`V2Grants`' four `TypeOK*` components and `V2Wait`'s `SatisfiedHoldsConditions`/`AnswerImpliesResolved`. Every
one is named in the mapping now, and a claim the mapping never mentions fails this audit — the reader's half of
the same question D-212 asked about configurations.

**D-220 added the rule that every module has to be *refutable*.** A module that no counterexample configuration
runs is a set of claims nothing refutes — the same gap as a claim nothing lists, one level up. V2Artifact,
V2Wait and V2Grants were the last three modules without one (measured 2026-09-27), and V2Wait's own refutation
had existed before the fix: `review/fix-notes-verification-2026-09-24.md` records finding V-W1's counterexample
against `MC_wait_contract.cfg`, "renamed since to `MC_wait.cfg`", so the pre-fix shape became the positive
configuration and the refutation went with it — exactly what had happened to V2Task's V-G1 (D-218). All three
now carry a counterfactual constant and a control, and this audit fails a `V2*.tla` that is the spec of no
`cfg:spec` pair.

**D-219 added the rule that a claim has to be *able* to fail.** A variable no action ever changes is a constant
of the model, so every claim over it is trivially true or trivially false. Six of them were carrying claims:
V2Compress' `lost` and `uncovered` (a summary deleting its originals, coverage lifting), V2Daemon's `pruned`,
`drift` and `shrank` (reclaiming events, rewriting a receipt, rolling the log back) and V2Checks' `rewound` (a
round counter that goes backwards). Every monitor had been written for a bug and never wired to a step that could
perform it, so `NoEntryIsEverLost`, `CoverageNeverLifted`, `NoResyncInThisVersion`, `ReceiptsAreStable`,
`LogMonotone` and `RoundsAreMonotone` could not fail. `--tla DIR` reads a copy of the directory, which is how the
control is run: a copy with one monitor's writer removed reports it by name. The class reached past the monitors:
V2Compress' `RequestClosesOnce` was a disjunction whose two disjuncts covered `RequestState`, so `TypeOK` entailed
it — the transition property it names was not modelled at all.

**D-202 added the provenance the counts rest on.** The report opens with `## 0. Gate status (re-run <date> at
`<commit>`)`, and the counts below it describe the material *as of that commit* — but nothing held the two
against each other. Measured 2026-09-27: the heading named `f521fd4f`, a commit that predates
`verification/tla/MC_retention.cfg` (D-192), while the same section counted "the twelfth … the retention rule":
the stated re-run could not have produced the numbers it reported. So the named commit must exist, and it must
be at or after the newest commit that changed the material a re-run covers — `verification/tla`,
`verification/kani`, and every file the harness crate compiles in with `#[path]` (`lib.rs` pulls in
`core/src/kernel/types.rs`, so the proof's subject is material too).

Ceiling: this is a *catalogue* audit — it says nothing about whether TLC would pass a configuration, only that
the material is driven and counted. The states, the times and the property names in the report are measurements
of a run, and re-running the targets is what keeps them honest. It also does not require the named commit to be
today's `HEAD` (a gate status re-run later at the same commit is fine, and prose edits must not force a re-run),
and it cannot see a model whose abstraction drifted from the code it describes — that is what the
spec-to-code test in `make check` is for.
"""
import argparse
import fnmatch
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
TLA = "verification/tla"
KANI_SRC = "verification/kani/src"
MATERIAL = ("verification/tla", "verification/kani")
# (D-224) the version the Makefile pins and the documents that state it: a reader of either trusts the number
TLA_VERSION = re.compile(r"^TLA_VERSION\s*:=\s*(\S+)", re.M)
STATED_TLC = re.compile(r"TLC v([0-9][0-9.]*)")
VERSION_DOCS = ("docs/DEVELOPMENT.md", "verification/README.md")
TARGET = re.compile(r"^([A-Za-z0-9_.-]+):")
CFG = re.compile(r"\b(MC[A-Za-z0-9_]*\.cfg)\b")
MARKER = re.compile(r"^\s*\\\*[ \t]*-+[ \t]*(invariants|properties)\b", re.M)
DEFN = re.compile(r"^([A-Z][A-Za-z0-9_]*)\s*==", re.M)
SPEC = re.compile(r"\b(V2[A-Za-z0-9_]*\.tla)\b")
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
RERUN = re.compile(r"Gate status \(re-run (\d{4}-\d{2}-\d{2}) at `([0-9a-f]{7,40})`\)")


def git(*args: str) -> tuple[int, str]:
    """`(exit code, stdout)` for a git command in this repository."""
    done = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    return done.returncode, done.stdout.strip()


def material_paths() -> list[str]:
    """What a re-run has to cover: the TLA material, the harness crate, and the sources it compiles.

    The harness crate is `#[path]`-based: it compiles this repository's own file instead of a copy, so the
    proof's subject belongs in the set a stale re-run heading is measured against.
    """
    paths = list(MATERIAL)
    for source in sorted((REPO / KANI_SRC).rglob("*.rs")):
        for relative in PATH_ATTR.findall(source.read_text(encoding="utf-8")):
            paths.append(str((source.parent / relative).resolve().relative_to(REPO)))
    return paths


# Counts are spelled out in the prose, so the rule reads words: "fourteen", "forty-seven". A count above
# ninety-nine would need more than this (the ceiling is stated where the rule runs).
NUMBER_UNITS = {"one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7, "eight": 8,
                "nine": 9, "ten": 10, "eleven": 11, "twelve": 12, "thirteen": 13, "fourteen": 14, "fifteen": 15,
                "sixteen": 16, "seventeen": 17, "eighteen": 18, "nineteen": 19}
NUMBER_TENS = {"twenty": 20, "thirty": 30, "forty": 40, "fifty": 50, "sixty": 60, "seventy": 70, "eighty": 80,
               "ninety": 90}


def number(word: str) -> int:
    """The integer an English count word names, or `-1` when it names none (so a typo fails loudly)."""
    if word in NUMBER_UNITS:
        return NUMBER_UNITS[word]
    if "-" in word:
        tens, _, units = word.partition("-")
        if tens in NUMBER_TENS and units in NUMBER_UNITS:
            return NUMBER_TENS[tens] + NUMBER_UNITS[units]
    return NUMBER_TENS.get(word, -1)


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
    parser.add_argument("--tla", default=TLA,
                        help="the TLA+ directory to read (a copy is the control)")
    args = parser.parse_args(argv)
    tla = args.tla
    makefile_text = (REPO / args.makefile).read_text(encoding="utf-8")
    recipes = target_recipes(makefile_text)
    drivers = {name: body for name, body in recipes.items() if name.startswith("verify-model")}
    report = (REPO / args.report).read_text(encoding="utf-8")
    mapping = (REPO / args.mapping).read_text(encoding="utf-8")
    mapping_text = mapping   # the mapping's *text*: `mapping` is reused below for the cfg → spec table
    findings, notes = [], []
    if not drivers:
        findings.append(f"no `verify-model*` target in {args.makefile}: this audit's rule has stopped applying")
    named_cfgs = sorted({cfg for body in drivers.values() for cfg in CFG.findall(body)})
    # the modules the recipes name, *and* the ones the mapping blocks name: the mapping may live in a
    # variable the recipes expand (D-215), so a recipe that reads it names no `.tla` at all
    named_specs = sorted({spec + ".tla" for _, spec in re.findall(r"([A-Za-z0-9_.*|]+)\)\s*echo\s+(V2[A-Za-z0-9_]*)\.tla",
                                                  makefile_text)}
                        | {spec for body in drivers.values() for spec in SPEC.findall(body)})
    for cfg in named_cfgs:
        if not (REPO / tla / cfg).is_file():
            findings.append(f"{args.makefile} runs a TLC configuration that does not exist: {tla}/{cfg}")
    for spec in named_specs:
        if not (REPO / tla / spec).is_file():
            findings.append(f"{args.makefile} names a specification that does not exist: {tla}/{spec}")
    on_disk_cfgs = sorted(p.name for p in (REPO / tla).glob("MC*.cfg"))
    orphans = [cfg for cfg in on_disk_cfgs if cfg not in named_cfgs]
    if orphans:
        findings.append(f"{tla} holds configuration(s) no verify-model target runs: {', '.join(orphans)} — "
                        "verification material nothing checks")
    on_disk_specs = sorted(p.name for p in (REPO / tla).glob("V2*.tla"))
    unchecked = [spec for spec in on_disk_specs if spec not in named_specs]
    if unchecked:
        findings.append(f"{tla} holds module(s) no configuration is run against: {', '.join(unchecked)} — a "
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
    # (D-202) The re-run heading names the commit the gate status was measured at; a commit older than the
    # material it reports on is a provenance claim the tree contradicts.
    rerun = RERUN.search(report)
    if rerun is None:
        findings.append(f"{args.report} no longer opens with `Gate status (re-run <date> at `<commit>`)`, so its "
                        "provenance cannot be held against the material")
    else:
        date, commit = rerun.groups()
        material = material_paths()
        if git("cat-file", "-e", f"{commit}^{{commit}}")[0] != 0:
            findings.append(f"{args.report} names the re-run commit `{commit}`, which is not a commit in this "
                            "repository")
        else:
            code, newest = git("log", "-1", "--format=%H", "--", *material)
            if code != 0 or not newest:
                findings.append(f"no commit touches {', '.join(material)}, so the re-run heading has nothing to "
                                "be measured against")
            elif git("merge-base", "--is-ancestor", newest, commit)[0] != 0:
                findings.append(
                    f"{args.report} says the gates were re-run at `{commit}` ({date}), but `{newest[:12]}` "
                    f"changed {', '.join(material)} after it: the stated re-run predates the material it reports "
                    "on — re-run the targets and update the heading")
            else:
                notes.append(f"the re-run heading names `{commit}`, at or after the newest change to the "
                             f"verification material (`{newest[:12]}`)")
    # (D-212, D-215) Where a configuration's module comes from: the `case` blocks that map them — one shared block
    # the recipes expand, or a copy inside a recipe — and the explicit `cfg:spec` pairs the counterexample target
    # walks. Every block must name *every* configuration in the directory, and every source must agree about a
    # configuration's module. A configuration that fell through a `*)` default would silently run another module's
    # specification: the simulation target met exactly that while it was being generalized (D-215) and TLC's
    # refusal was reported as a "violation" for as long as no rule read the mapping.
    blocks = []
    for match in re.finditer(r"case \S*cfg in (.*?)\besac\b", makefile_text, re.S):
        arms = []
        for group, spec in re.findall(r"([A-Za-z0-9_.*|]+)\)\s*echo\s+(V2[A-Za-z0-9_]*)\.tla", match.group(1)):
            arms += [(arm, spec) for arm in group.split("|") if arm != "*"]
        blocks.append(arms)
    if not blocks:
        findings.append(f"{args.makefile} maps no configuration to a module: this audit's rule has nothing to read")
    for arms in blocks:
        for cfg in on_disk_cfgs:
            if not any(fnmatch.fnmatchcase(cfg, arm) for arm, _ in arms):
                findings.append(f"{args.makefile}: a mapping block does not name {cfg}, so it would fall through "
                                "the `*)` default and run another module's specification")
    pairs = dict(re.findall(r"(MC[A-Za-z0-9_]*\.cfg):(V2[A-Za-z0-9_]*)\.tla", makefile_text))
    for arms in blocks:
        for arm, spec in arms:
            for cfg in on_disk_cfgs:
                if fnmatch.fnmatchcase(cfg, arm) and cfg in pairs and pairs[cfg] != spec:
                    findings.append(f"{args.makefile}: a mapping block runs {cfg} against {spec}.tla while the "
                                    f"counterexample pairs name {pairs[cfg]}.tla: they have to agree")
    # (D-220) Falsifiability is per module, not per configuration set: a module no counterexample configuration
    # runs is a set of claims nothing refutes. Measured 2026-09-27: V2Artifact, V2Wait and V2Grants were the
    # last three without one — and V2Wait's own refutation (finding V-W1) had existed until its old
    # configuration was renamed into the positive `MC_wait.cfg`, the same way V2Task lost V-G1's (D-218).
    controlled = set(pairs.values())
    for spec in on_disk_specs:
        if spec.removesuffix(".tla") not in controlled:
            findings.append(f"{tla}/{spec} is the spec of no counterexample configuration in {args.makefile}: its "
                            "claims are stated and enumerated exhaustively, but no configuration is shown to "
                            "refute one of them (D-220)")
    mapping = {}
    for arms in blocks:
        for arm, spec in arms:
            for cfg in on_disk_cfgs:
                if fnmatch.fnmatchcase(cfg, arm):
                    mapping.setdefault(cfg, spec)
    for cfg, spec in pairs.items():
        if cfg in on_disk_cfgs:
            mapping.setdefault(cfg, spec)
    for name, body in recipes.items():
        if not name.startswith("verify-model"):
            continue
        for cfg in re.findall(r"-config ([A-Za-z0-9_.]+\.cfg)", body):
            if cfg not in mapping:
                findings.append(f"`{name}` runs {cfg}, which no mapping block names: it would fall through the "
                                "`*)` default and run the wrong module")
    checked = {}
    for cfg in on_disk_cfgs:
        spec = mapping.get(cfg)
        if not spec:
            continue
        names = set()
        for key in ("INVARIANTS", "PROPERTIES"):
            block = re.search(rf"^{key}[ \t]*\n((?:[ \t]+\S[^\n]*\n)+)",
                              (REPO / tla / cfg).read_text(encoding="utf-8"), re.M)
            if block:
                names |= {line.strip() for line in block.group(1).split("\n") if line.strip()}
        checked.setdefault(spec, set()).update(names)
    for spec in on_disk_specs:
        module = spec.removesuffix(".tla")   # the cfgs name modules, the directory lists files
        text = (REPO / tla / spec).read_text(encoding="utf-8")
        marks = list(MARKER.finditer(text))
        if not marks:
            findings.append(f"{tla}/{spec} marks no invariants or properties section, so the audit cannot tell "
                            "what it claims")
            continue
        claimed = set()
        for index, mark in enumerate(marks):
            stop = len(text) if index + 1 >= len(marks) else marks[index + 1].start()
            claimed |= set(DEFN.findall(text[mark.end():stop]))
        missing = sorted(claimed - checked.get(module, set()))
        if missing:
            findings.append(f"{tla}/{spec} marks {', '.join(missing)} as its invariants or properties, and no "
                            "configuration that runs it lists them: a claim nothing checks")
        # (D-222) ... and the mapping has to name every one. The report sends a reader to `verification/README.md`
        # for what a module proves, and ten of the 148 marked claims were absent from it (measured 2026-09-27):
        # all four of V2Retention's substantive claims, V2Grants' four `TypeOK*` components and V2Wait's
        # `SatisfiedHoldsConditions`/`AnswerImpliesResolved`. A name a configuration lists and the mapping never
        # mentions is a claim a reader cannot look up.
        unnamed = sorted(name for name in claimed if name not in mapping_text)
        if unnamed:
            findings.append(f"{tla}/{spec} marks {', '.join(unnamed)}, which {args.mapping} never names: the "
                            "mapping is what the report sends a reader to for what a module proves (D-222)")
    # (D-219) A variable no action ever changes is a constant of the model: every claim over it is either
    # trivially true or trivially false, so nothing checks it. The class was found by the survey that wrote
    # this rule (measured 2026-09-27): six variables across three modules had no writer at all — V2Compress'
    # `lost` and `uncovered`, V2Daemon's `pruned`, `drift` and `shrank`, V2Checks' `rewound` — and the claims
    # built on them (`NoEntryIsEverLost`, `CoverageNeverLifted`, `NoResyncInThisVersion`, `ReceiptsAreStable`,
    # `LogMonotone`, `RoundsAreMonotone`) could not fail. `pruned` was deliberate ("this version never
    # reclaims events") and the others were monitors whose writer nobody had wired; both are fixed the same
    # way, with a counterfactual constant that performs the mistake and a control that refutes the claim.
    # The check reads the declared `VARIABLES` block and treats an assignment as a write unless its text is
    # just the variable itself (`v' = v \cup …` counts, `v' = v` does not).
    declares = re.compile(r"^VARIABLES\s*\n((?:[ \t]+\S[^\n]*\n)+)", re.M)
    assigns = re.compile(r"^\s*[\\/]*\s*([A-Za-z][A-Za-z0-9_]*)' *= *([^\n]*)", re.M)
    for spec in on_disk_specs:
        text = (REPO / tla / spec).read_text(encoding="utf-8")
        block = declares.search(text)
        if block is None:
            findings.append(f"{tla}/{spec} declares no VARIABLES block, so this audit cannot tell whether a "
                            "claim over one of its variables can fail")
            continue
        names = []
        for line in block.group(1).split("\n"):
            name = line.split("\\*")[0].strip().rstrip(",")
            if name:
                names.append(name)
        written = {}
        for name, rhs in assigns.findall(text):
            written.setdefault(name, []).append(rhs.split("\\*")[0].strip())
        dead = [name for name in names
                if not written.get(name) or all(rhs == name for rhs in written[name])]
        if dead:
            findings.append(f"{tla}/{spec} never changes {', '.join(dead)}: a variable no action writes is a "
                            "constant of the model, so every claim over it holds for want of a step that could "
                            "break it (D-219)")
    # (D-224) The version the Makefile pins, against the documents that state it. Both are read by a person who
    # will never check the jar: measured 2026-09-27, the two statements agreed with the pin — and nothing had
    # been comparing them, so a bumped `TLA_VERSION` would have left both telling a reader the old one.
    pinned = TLA_VERSION.search(makefile_text)
    if pinned is None:
        findings.append(f"{args.makefile} states no `TLA_VERSION`, so the documents that name the pinned TLC "
                        "cannot be held to it (D-224)")
    else:
        for doc in VERSION_DOCS:
            stated = STATED_TLC.search((REPO / doc).read_text(errors="replace"))
            if stated is None:
                findings.append(f"{doc} no longer states the pinned TLC version, so this audit cannot hold it "
                                "to the Makefile (D-224)")
            elif stated.group(1) != pinned.group(1):
                findings.append(f"{doc} says TLC v{stated.group(1)}, {args.makefile} pins {pinned.group(1)}: the "
                                "reader of one trusts the number in the other (D-224)")
        # This script's own docstring states counts too, and nothing looked at them: the
    # sentence in it said "forty" while the directory held forty-three (D-208).
    stated = re.search(r"holds ([a-z-]+) TLA\+ modules and ([a-z-]+) configurations", __doc__ or "")
    if stated is None:
        findings.append("this script's docstring no longer states its own module and configuration counts, so the "
                        "rule that checks them has nothing to compare with")
    else:
        modules_stated, configs_stated = number(stated.group(1)), number(stated.group(2))
        on_disk = len(on_disk_cfgs)
        if modules_stated != len(on_disk_specs):
            findings.append(f"this script's docstring says {stated.group(1)!r} ({modules_stated}) module(s), {TLA} "
                            f"holds {len(on_disk_specs)}: the prose and the directory have to agree")
        if configs_stated != on_disk:
            findings.append(f"this script's docstring says {stated.group(2)!r} ({configs_stated}) configurations, "
                            f"{tla} holds {on_disk}")

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

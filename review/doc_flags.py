#!/usr/bin/env python3
"""The product docs may not name a flag the CLI does not serve (D-135)

D-73 made every unserved argument a refusal, so a flag a document shows either exists or the user meets an
error the first time they copy it. Nothing checked the two documents a user reads first. The scan that
prompted this found one such flag: `docs/USER-GUIDE.md` §6 described the earlier-release cleanup as "delete
only with an explicit `--apply`", and `--apply` is not a flag of any entry point (the cleanup was a one-off
migration, recorded in `docs/ACCEPTANCE.md`'s upgrade notes).

**D-214 added the verbs.** The rule above is about flags; the same document already tells the user which *verb* to
run (`teamagents exec`, `teamagents instances terminate --id …`), and a verb this build does not serve is exactly
the same defect — D-52/D-73 removed `validate` and `sessions prune`, and a document showing them would send a user
into a refusal. So a backticked `teamagents <verb>` in these three documents must be a verb the help text names (or
the dispatcher's own `Some("…")` arms serve: the hidden `jobs-runner` entry point is one), and when the mention
carries a second word *and* that verb takes sub-verbs at all, the pair must be one the help text shows
(`instances resume|pause`, `tasks cancel`, …). Measured 2026-09-27: 30 verb mentions and 12 pairs in the three
documents, all served. The same history window applies — a line about an earlier release is a note.

The authority is the CLI's own help text — the `usage()` literal in `engine/src/main.rs`, which is the surface
`teamagents --help` prints. It is read from the source rather than by running the binary, so this check works
in `make hygiene` on a tree that has not been built. Flags that belong to the *toolchain* rather than to this
product are listed below with their reason; everything else in scope must be in the help text.

    python3 review/doc_flags.py [--list-served]

The second half of the check is about the parser rather than the documents: a flag it *accepts* must either be
advertised in that help text, or be named in a refusal message — accepted-and-then-ignored is the defect
D-75's rule forbids for config keys, and it is how the removed `sessions` subcommand's flags
(`--history-days`, `--days`, `--dry-run`) survived: parsed into fields nothing reads, while `sessions` itself
is refused. This half needs no allowlist: the three removed top-level flags (`--plain`, `--resume`, `--team`)
are accepted on purpose so the refusal can name them, and each is named in one.

Scope of the document half: the three documents that tell a *user* which commands to run — `README.md`,
`docs/USER-GUIDE.md` and `docs/INSTALL.md` (D-190 added the last one: it was left out because it records the
flags of earlier releases, but its *current* commands, `teamagents --cwd …` and `exec --json`, went unaudited
with them). A line that marks itself as history — an older version number (`v0.1.1`, `v0.1.2`) or one of the
words `earlier`, `legacy`, `removed`, `pre-v2`, `no longer` — is reported as a note rather than a finding,
because it states what that release had, not what this one serves. The rest of the documents stay out of
scope on purpose: `docs/ACCEPTANCE.md` records the flags this build *removed*, `docs/PRODUCT-COMPARISON.md`
names other products' flags and a proposed one, and `docs/DECISIONS.md`, `docs/DEVELOPMENT.md`, `AGENTS.md`
and `review/*.md` quote the scripts and tools this repository runs.

**D-235 added the exit-code contract.** The headless client's exit codes are a product contract stated in prose
twice — `README.md`'s "Exit codes: `0` settled, …" sentence and `docs/USER-GUIDE.md`'s table beside the `exec`
section — and implemented in `engine/src/v2/exec.rs`'s `End::exit_code`, whose values the code-level test
`exit_codes_follow_the_documented_contract` asserts. So the *code* was pinned by a test and the *documents* were
the unchecked half: measured 2026-09-27, nothing compared them. The rule compares the two sets of *values* (never
the semantics): every code `End::exit_code` can return, plus the usage code 2 that `engine/src/main.rs` exits
with, must be named by a document, and no document may name a code the client cannot return. A missing side is a
finding too, so the rule cannot pass by reading nothing. Control: `--doc` on a copy of the user guide with its
`124` changed to `125` reports both directions at once.

Ceiling: the parser half reads match-arm *patterns*, so a flag accepted through a different shape (a loop over
`["--a", "--b"]`, say) is invisible to it; and neither half checks that a help-advertised flag is actually
honoured at runtime.
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
CLI_SOURCE = REPO / "engine/src/main.rs"
EXEC_SOURCE = REPO / "engine/src/v2/exec.rs"
DOCS = ["README.md", "docs/USER-GUIDE.md", "docs/INSTALL.md"]
# (D-235) The headless exit codes are a product contract in prose (the README's sentence, the user guide's table)
# and in code (`End::exit_code`, with `exit_codes_follow_the_documented_contract` asserting *its* values). Nothing
# compared the two sides: the code-level test pins the table, so the table is the source of truth and the
# documents were the unchecked half (measured 2026-09-27). Only the *values* are compared, never the semantics.
EXIT_ROW = re.compile(r"^\|\s*`(\d+)`\s*\|", re.M)
EXIT_ARM = re.compile(r"End::\w+(?:\s*\|\s*End::\w+)*(?:\s*if[^=]*)?\s*=>\s*(\d+)")
USAGE_EXIT = re.compile(r"process::exit\(2\)")
# A line that says it is about an earlier release states history, not a claim about this build (D-190). Prose
# wraps, so the *window* is the line and the two above it: `docs/INSTALL.md`'s v0.1.2 note names the flags it
# had on its second line.
HISTORY_MARKERS = ("v0.1.1", "v0.1.2", "earlier", "legacy", "removed", "pre-v2", "no longer")
HISTORY_WINDOW = 2

# Flags of the tools these docs tell the user to run, not of this product.
TOOL_FLAGS = {
    "--offline": "cargo/make: reuse the cached dependencies",
    "--locked": "cargo: require Cargo.lock to be current",
    "--manifest-path": "cargo: point at a crate's Cargo.toml",
    "--release": "cargo build: an optimised build",
    "--bin": "cargo build: build only this binary",
    "--bin-dir": "install.sh: where the installer puts the binaries (its own test is engine/tests/install.rs)",
    "--archive": "install.sh: install from a local archive instead of downloading (review/install_check.py)",
    "--strict": "review/dogfood/protocols.py: fail instead of skipping a family with no credential",
}

FLAG = re.compile(r"(?<![\w-])--[a-z][a-z-]*")
# A match arm whose pattern is a bare alternation of string literals, e.g. `"--json"` on its own line (its
# guard and `=>` follow). Distinguishes an arm from a message that merely starts with a quote.
ARM_LINE = re.compile(r'^"[^"]*"(?:\s*\|\s*"[^"]*")*\s*$')
VERB_MENTION = re.compile(r"`teamagents ([a-z][a-z-]*)(?: ([a-z][a-z-]*))?")
# a verb line of the help text, e.g. "  teamagents instances resume|pause --id ID"
VERB_LINE = re.compile(r"\s*teamagents ([a-z][a-z-]*)(.*)$")
# the hidden entry points the dispatcher matches, which are served without being advertised
HIDDEN_VERB = re.compile(r'Some\("([a-z][a-z-]*)"\)')


def served_flags() -> set:
    """The flags in the CLI's own `HELP` text: the surface `teamagents --help` prints."""
    lines = CLI_SOURCE.read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith("const HELP"))
    end = next(i for i in range(start, len(lines)) if lines[i].rstrip().endswith('";'))
    return set(FLAG.findall("\n".join(lines[start:end])))


def served_verbs() -> tuple:
    """`(verbs, pairs)` the CLI serves: what the help text names, plus the dispatcher's own arms.

    The help text is D-135's authority for the flag surface, and it is the authority here too — with one addition,
    because a served verb is also a `match` arm: `jobs-runner` is the runner's hidden entry point and appears in no
    help line.
    """
    lines = CLI_SOURCE.read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith("const HELP"))
    end = next(i for i in range(start, len(lines)) if lines[i].rstrip().endswith('";'))
    verbs, pairs = set(), set()
    for line in lines[start:end]:
        match = VERB_LINE.match(line)
        if not match:
            continue
        verb, rest = match.group(1), match.group(2).split()
        verbs.add(verb)
        # the sub-verbs are the next word unless it is a flag or nothing: `instances resume|pause`, `tasks cancel`
        if rest and not rest[0].startswith("-"):
            for sub in rest[0].strip("[]").split("|"):
                if re.fullmatch(r"[a-z][a-z-]*", sub):
                    pairs.add(f"{verb} {sub}")
    return verbs | set(HIDDEN_VERB.findall(CLI_SOURCE.read_text())), pairs


def accepted_flags() -> set:
    """Flags the parser *accepts*: match-arm patterns, including the ones whose guard wraps onto later lines."""
    accepted = set()
    for line in CLI_SOURCE.read_text().splitlines():
        if "=>" in line:
            accepted |= set(FLAG.findall(line.split("=>")[0]))
        elif ARM_LINE.match(line.strip()):
            accepted |= set(FLAG.findall(line))
    return accepted


def named_flags() -> set:
    """Flags the file names outside an arm pattern: refusal messages, doc comments, the help text."""
    named = set()
    for line in CLI_SOURCE.read_text().splitlines():
        if "=>" not in line and not ARM_LINE.match(line.strip()):
            named |= set(FLAG.findall(line))
    return named


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list-served", action="store_true", help="print the flags the CLI's help lists")
    parser.add_argument("--doc", action="append", default=[], metavar="PATH",
                        help="documents to read instead of the default three (a copy is the control)")
    args = parser.parse_args()
    docs = args.doc or DOCS
    served = served_flags()
    verbs, pairs = served_verbs()
    if args.list_served:
        print("served: " + " ".join(sorted(served)))
    findings, history = [], []
    scanned, scanned_verbs = 0, 0
    # D-235: the exit-code contract, both ways — every code the client can return is named by the documents, and
    # no document names a code it cannot return. A missing side is a finding too, so the rule cannot pass by
    # reading nothing.
    arms = EXIT_ARM.findall(EXEC_SOURCE.read_text(errors="replace"))
    if not arms:
        findings.append("engine/src/v2/exec.rs: no `End::… => N` arm found, so the exit codes cannot be read")
    if not USAGE_EXIT.search(CLI_SOURCE.read_text(errors="replace")):
        findings.append("engine/src/main.rs: no `process::exit(2)` found, so the usage code this audit assumes "
                        "is gone — teach the rule where a usage error exits now")
    implemented = {int(code) for code in arms} | {2}
    documented = set()
    for name in docs:
        text = (REPO / name).read_text(errors="replace")
        for match in EXIT_ROW.finditer(text):
            documented.add(int(match.group(1)))
        for line in text.split("\n"):
            if line.startswith("Exit codes") or line.startswith("Exit code "):
                documented |= {int(code) for code in re.findall(r"`(\d+)`", line)}
    for code in sorted(implemented - documented):
        findings.append(f"the client can exit {code}, and no document names it: the exit code is a contract a "
                        "script reads (docs/USER-GUIDE.md's table, README's sentence — D-235)")
    for code in sorted(documented - implemented):
        findings.append(f"a document names exit code {code}, which the client cannot return "
                        "(End::exit_code in engine/src/v2/exec.rs plus the usage code — D-235)")
    for name in docs:
        lines = (REPO / name).read_text().splitlines()
        for number, line in enumerate(lines, 1):
            window = " ".join(lines[max(0, number - 1 - HISTORY_WINDOW):number]).lower()
            told_as_history = any(marker in window for marker in HISTORY_MARKERS)
            for flag in FLAG.findall(line):
                scanned += 1
                if flag in served or flag in TOOL_FLAGS:
                    continue
                where = f"{name}:{number}: {flag} is not a flag this build serves"
                (history if told_as_history else findings).append(where)
            for verb, sub in VERB_MENTION.findall(line):
                scanned_verbs += 1
                if verb not in verbs:
                    where = f"{name}:{number}: `teamagents {verb}` is not a verb this build serves"
                    (history if told_as_history else findings).append(where)
                elif sub and f"{verb} {sub}" not in pairs and any(p.startswith(verb + " ") for p in pairs):
                    where = (f"{name}:{number}: `teamagents {verb} {sub}` is not a pair this build serves "
                             f"(it shows {', '.join(sorted(p.split(' ', 1)[1] for p in pairs if p.startswith(verb + ' ')))})")
                    (history if told_as_history else findings).append(where)

    accepted, named = accepted_flags(), named_flags()
    silent = sorted(accepted - served - named)
    for flag in silent:
        findings.append(f"engine/src/main.rs: the parser accepts {flag}, and neither the help text nor any "
                        f"refusal message names it")
    print(f"{scanned} flag mentions in {', '.join(DOCS)}; the CLI's help lists {len(served)} flags and "
          f"{len(TOOL_FLAGS)} belong to the toolchain")
    print(f"{scanned_verbs} verb mention(s) in the same documents name {len(verbs)} served verb(s) and "
          f"{len(pairs)} pair(s)")
    print(f"the parser accepts {len(accepted)} flags; "
          f"{len(accepted & served)} of them are advertised, {len(silent)} are accepted silently")
    if history:
        print(f"note: {len(history)} flag mention(s) sit on a line that says it is about an earlier release: "
              + "; ".join(history[:3]) + (" …" if len(history) > 3 else ""))
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

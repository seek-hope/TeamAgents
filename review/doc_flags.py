#!/usr/bin/env python3
"""The product docs may not name a flag the CLI does not serve (D-135)

D-73 made every unserved argument a refusal, so a flag a document shows either exists or the user meets an
error the first time they copy it. Nothing checked the two documents a user reads first. The scan that
prompted this found one such flag: `docs/USER-GUIDE.md` §6 described the earlier-release cleanup as "delete
only with an explicit `--apply`", and `--apply` is not a flag of any entry point (the cleanup was a one-off
migration, recorded in `docs/ACCEPTANCE.md`'s upgrade notes).

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

Scope of the document half: `README.md` and `docs/USER-GUIDE.md`. The other documents talk about flags on
purpose —
`docs/ACCEPTANCE.md` and `docs/INSTALL.md` record the ones this build *removed* (`--plain`, `--resume`,
`--team`, `--verbose`), `docs/PRODUCT-COMPARISON.md` names other products' flags and a proposed one, and
`docs/DECISIONS.md` and `docs/DEVELOPMENT.md` quote the scripts and tools this repository runs.

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
DOCS = ["README.md", "docs/USER-GUIDE.md"]

# Flags of the tools these docs tell the user to run, not of this product.
TOOL_FLAGS = {
    "--offline": "cargo/make: reuse the cached dependencies",
    "--locked": "cargo: require Cargo.lock to be current",
    "--manifest-path": "cargo: point at a crate's Cargo.toml",
    "--release": "cargo build: an optimised build",
    "--bin": "cargo build: build only this binary",
    "--strict": "review/dogfood/protocols.py: fail instead of skipping a family with no credential",
}

FLAG = re.compile(r"(?<![\w-])--[a-z][a-z-]*")
# A match arm whose pattern is a bare alternation of string literals, e.g. `"--json"` on its own line (its
# guard and `=>` follow). Distinguishes an arm from a message that merely starts with a quote.
ARM_LINE = re.compile(r'^"[^"]*"(?:\s*\|\s*"[^"]*")*\s*$')


def served_flags() -> set:
    """The flags in the CLI's own `HELP` text: the surface `teamagents --help` prints."""
    lines = CLI_SOURCE.read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith("const HELP"))
    end = next(i for i in range(start, len(lines)) if lines[i].rstrip().endswith('";'))
    return set(FLAG.findall("\n".join(lines[start:end])))


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
    args = parser.parse_args()
    served = served_flags()
    if args.list_served:
        print("served: " + " ".join(sorted(served)))
    findings = []
    scanned = 0
    for name in DOCS:
        for number, line in enumerate((REPO / name).read_text().splitlines(), 1):
            for flag in FLAG.findall(line):
                scanned += 1
                if flag in served or flag in TOOL_FLAGS:
                    continue
                findings.append(f"{name}:{number}: {flag} is not a flag this build serves")

    accepted, named = accepted_flags(), named_flags()
    silent = sorted(accepted - served - named)
    for flag in silent:
        findings.append(f"engine/src/main.rs: the parser accepts {flag}, and neither the help text nor any "
                        f"refusal message names it")
    print(f"{scanned} flag mentions in {', '.join(DOCS)}; the CLI's help lists {len(served)} flags and "
          f"{len(TOOL_FLAGS)} belong to the toolchain")
    print(f"the parser accepts {len(accepted)} flags; "
          f"{len(accepted & served)} of them are advertised, {len(silent)} are accepted silently")
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

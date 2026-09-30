#!/usr/bin/env python3
"""Citations in the documentation that point at nothing in the tree (the detector behind D-109).

`docs/ACCEPTANCE.md` is the evidence ledger and `review/README.md` is the index of it, so their citations —
`` `v2_driver::end_to_end_shell_then_finish` ``, `` `review/dogfood/crash.py` `` — *are* the re-runnable
commands. A citation that names a test that was renamed, a file that was removed or a path that never existed
makes an acceptance claim unverifiable while still reading as evidence; the same class produced D-53
(documented claims corrected to the code), D-78/D-86 (public items nothing calls) and D-102 (a config key
nothing reads), all found by hand.

    python3 review/citations.py

It reads the tracked markdown (docs, review, README.md, AGENTS.md) and checks three kinds of backticked
citation against the tree:

  * a qualified name `` `a::b` `` must resolve to a test, to any `fn`/`const`/`static`/`struct`/`enum`/`type`/
    `trait`/`mod` name, or to a module file (`a::b` resolves if `b.rs` exists);
  * a repository-looking path (an extension of `.rs/.py/.sh/.toml/.md/.json/.jsonl` under `docs/`, `engine/`,
    `tui/`, `core/`, `review/`, `verification/` or `examples/`) must exist in the tracked tree;
  * a backticked `.rs` basename must name a file that exists anywhere in the tree (in Rust sources too, where
    this is the only rule applied).

A line that says the cited thing is *gone* (a marker word, or a markdown table whose header says so) counts as
a note rather than a finding: `docs/DECISIONS.md` keeps tables of removed items on purpose, and a citation
there is how the removal is recorded. The tree definition is `git ls-files --cached --others --exclude-standard`,
so a file that is about to be added resolves (the check runs on a working tree, not on a commit) while the
ignored scratch areas stay invisible.

Limits, deliberately stated: bare basenames (`report.json`, `notify.sh`, `credentials.toml`, `INPUTS.md`) are
out of scope — they are runtime artifacts and user-written files, and in a prose scan they drown the signal;
upstream citations (`docs/loops.md` of another project, a URL) are out of scope too, because the tree cannot
resolve them — D-109 is the entry that found one of those by hand; and prose that names a file without any
backticks is not seen at all.

**D-200 added the links a reader clicks.** The backticked citations above are paths a reader retypes; an inline
markdown link (`[the guide](USER-GUIDE.md#2-configuration)`) is a path a reader *follows*, and relative targets
were checked by nothing (`readme_zh.py` covers the Chinese README's in-repository links only). Every relative
inline link in a tracked markdown file must resolve against the linking file's directory, after dropping the
fragment; absolute URLs, `mailto:` and pure fragments are out of scope, as is a link whose target sits inside a
code span — the extraction drops code spans and fenced blocks first, because `[](Entitled(i, "shell"))` in a
TLA+ formula is not a link (the first run of the check reported exactly that as broken, which is how the rule got
its shape).

**D-201 added the commands a reader runs.** A `make <target>` in a document is the same kind of citation one step
over from a link: the reader pastes it, and if the target was renamed or removed the shell answers "make: *** No
rule to make target". Nothing watched it — `build_references.py` reads the Makefile's own two lists (D-196) and
its own Limits paragraph delegates documents to this file, which resolved paths and links but not commands.
Measured when it was added: 19 tracked markdown files carried **389** `make <target>` citations (356 inside inline code
spans, 33 as a line of a fenced block); they name 20 distinct targets, 18 of which the Makefile declares, and the
two that are not are both absence records (below). The check is added after the fact, not because something was
broken. The declared list is `build_references.phony_targets`, so the two audits cannot disagree about what the
Makefile offers.

A document that records a target's absence is a note, exactly as for a path: `review/fix-notes-verification-2026-09-24.md`
says `make verify-model-contract` "was deleted", and `docs/DECISIONS.md`'s D-196 entry quotes this audit's own
finding about a made-up `make deploy` help line. A line that mentions `.PHONY` is talking about declarations
rather than telling the reader to run something, so `.PHONY` on the line makes it a note too (a record, the way
"deleted" does).

**D-213 added the model's own names.** `verification/tla/` is cited all over the documents — `MC_compress.cfg`,
`tla/V2Jobs.tla`, and properties by module and name (`V2Control::NoReceiptAcrossEpochs`) — and none of it was
resolved: the path rule's suffixes did not include `.tla`/`.cfg`, the basename rule knew only `.rs`, and the
qualified-name rule required a lowercase start, which no TLA+ module name has. A cited model file must now exist
(by path or by basename, like a `.rs` file), and a `` `V2Name::member` `` citation is resolved *only* when
`verification/tla/V2Name.tla` exists — because the tree has Rust types whose names look the same (`V2Toolkit::new`
is `engine/src/tools.rs`'s own struct, and its doc comments cite it that way), so a capitalised `::` citation is
not evidence of a model citation. This is also what makes a sentence that leans on a *marked* claim a sentence
about something checked: D-212 requires every marked name to be listed by a configuration that runs its module.

Ceiling: only the two shapes a reader can paste are read (an inline code span whose content is a `make` command,
and a fenced-block line that begins with one, an optional `$ ` prompt stripped), so a command in bare prose is not
seen; the `.PHONY` list is the universe, so a documented target that exists as a recipe but is not in `.PHONY` is
reported; a flag between the command and the target (`make -f FILE target`, as the D-111 control writes) hides it;
a `make` command quoted from another project inside a fenced example would be reported (none exists today); and a
citation on a line that mentions `.PHONY` for an unrelated reason is excused.

**D-223 added the sections a reader follows.** The documents cite the design by number — `§4.4`, `§5.3`,
`§12.1`, `§12.3`, `§14` — and no audit resolved a number: a citation to a section that was renumbered or never
existed reads exactly like one that resolves. It was not hypothetical. The earlier plan's numbering survives in
43 places (measured 2026-09-27) across the documents, `AGENTS.md` and twelve Rust sources — `§12.1` (the offered
surface follows the declared bindings), `§12.2` (sandbox, environment and credential hygiene), `§12.3` (workspace
policies) and `§14` (one owner per behaviour) — while the design's §12 is the acceptance matrix and its headings
run 1–10, 12, 13, 16: a reader following `§12.3` in `AGENTS.md`, or `§12.1` in `docs/TOOLS.md`, arrived at the
wrong place. Every one is repointed at the live section (the mapping is in D-223), the three that state a rule
the design does not carry say "the archived plan's" instead, and a `§N.M` citation must name a section of one of
the numbered documents — `docs/DESIGN.md`, `docs/USER-GUIDE.md`, `docs/INSTALL.md`, `verification/REPORT.md` —
unless the line records it as archived or its file is the upstream-comparison note (`EXTERNAL_SECTIONS`, whose
numbers are the compared platform's own).

This docstring states counts of the current tree as well, and nothing looked at them either (D-208's pattern, one
file over): **21** markdown files and **85** Rust files carry **1151** citations, **82** relative links, **697** `make` commands and **1054** `§`-section references, all five recomputed and compared here.
"""

import pathlib
import re
import subprocess
import sys
import urllib.parse

REPO = pathlib.Path(__file__).resolve().parents[1]

sys.path.insert(0, str(REPO / "review"))   # one Makefile parser, shared with the audit that owns D-196
from build_references import phony_targets  # noqa: E402

EXCLUDED = ("review/tmp/", "review/eval/")
PATH_PREFIXES = ("docs/", "engine/", "tui/", "core/", "review/", "verification/", "examples/")
PATH_SUFFIXES = (".rs", ".py", ".sh", ".toml", ".md", ".json", ".jsonl", ".tla", ".cfg")
# an inline markdown link: [label](target), optionally with a title, and never inside a code span or fence
LINK = re.compile(r"\[([^\]]*)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
FENCE = re.compile(r"^\s*(```|~~~)")
CODE_SPAN = re.compile(r"`[^`]*`")
# a `make <target>` a reader can paste: the command word, then a target name, then anything (flags, variables)
MAKE_CMD = re.compile(r"^make\s+([A-Za-z][A-Za-z0-9-]*)")
# a line that mentions `.PHONY` is discussing the Makefile's declarations, not telling the reader to run a command
DECLARATION_TALK = ".phony"
QUALIFIED = re.compile(r"`([a-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+)`")
# a model citation: a module name that has a `verification/tla/<name>.tla`, and a member of it
TLA_CITE = re.compile(r"`(V2[A-Za-z0-9_]*)::([A-Za-z_][A-Za-z0-9_]*)`")
PATH_RE = re.compile(r"`([A-Za-z0-9_./-]+(?:" + "|".join(re.escape(s) for s in PATH_SUFFIXES) + r"))`")
RS_BASENAME = re.compile(r"`([A-Za-z0-9_-]+\.(?:rs|tla|cfg))`")
ITEM_RE = re.compile(r"\b(?:fn|const|static|struct|enum|type|trait|mod)\s+([A-Za-z_][A-Za-z0-9_]*)")
TEST_RE = re.compile(r"#\[(?:tokio::)?test\]\s*(?:#\[[^\]]*\]\s*)*fn\s+([A-Za-z_][A-Za-z0-9_]*)")
# the numbered documents a `§N.M` citation can point at, and the headings that define those numbers (D-223)
SECTION = re.compile(r"§(\d+(?:\.\d+)*)")
SECTION_DOCS = ("docs/DESIGN.md", "docs/USER-GUIDE.md", "docs/INSTALL.md", "verification/REPORT.md")
HEADING = re.compile(r"^#{2,4}\s+(\d+(?:\.\d+)*)\.?\s", re.M)
STATED_COUNTS = re.compile(r"\*\*(\d+)\*\* markdown files and \*\*(\d+)\*\* Rust files carry "
                           r"\*\*(\d+)\*\* citations, \*\*(\d+)\*\* relative links, "
                           r"\*\*(\d+)\*\* `make` commands and \*\*(\d+)\*\* `§`-section references")
# a file whose `§`-numbers are deliberately not this repository's, with the reason it stays that way
EXTERNAL_SECTIONS = {
    "review/dsec-kernel-reference-2026-09-24.md":
        "it states what an external sandbox platform's own sections contain, so its `§`-numbers are that "
        "document's",
}
# a citation that says the thing is gone is a record of a removal, not a broken reference
GONE_MARKERS = ("deleted", "removed", "renamed", "gone", "dropped", "no longer exists", "obsolete", "pre-v2", "legacy",
                "archived",
                "does not exist", "do not exist", "doesn't exist", "never existed", "nonexistent",
                "no such file", "404")


def tracked():
    out = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard"],
                         cwd=REPO, capture_output=True, text=True, check=True).stdout
    return [path for path in out.split("\n") if path]


def tla_members() -> dict:
    """`{module: its text}` for every `verification/tla/V2*.tla`: what a `V2Module::member` citation resolves in."""
    out = {}
    for path in (REPO / "verification" / "tla").glob("V2*.tla"):
        out[path.stem] = path.read_text(errors="replace")
    return out


def universe(files):
    """Every name a qualified citation may resolve to, and every file basename."""
    items, tests, basenames = set(), set(), {}
    for name in files:
        basenames.setdefault(pathlib.Path(name).name, name)
        if not name.endswith(".rs"):
            continue
        text = (REPO / name).read_text(errors="replace")
        items |= {match.group(1) for match in ITEM_RE.finditer(text)}
        tests |= {match.group(1) for match in TEST_RE.finditer(text)}
    return items, tests, basenames


def table_header(line_number, lines):
    """The header row of the markdown table the line belongs to, if that block has one.

    Only the *first* row (the header, recognised by the `|---|` separator under it) is returned: a long table
    has no business lending a word from an unrelated row to a citation far below it — `docs/ACCEPTANCE.md`'s
    A-matrix is one table, and a "removed" in row A12 would otherwise excuse every citation under it.
    """
    if line_number > 0 and not lines[line_number - 1].lstrip().startswith("|"):
        return ""
    index = line_number - 1
    while index >= 0 and lines[index].lstrip().startswith("|"):
        index -= 1
    header, separator = lines[index + 1], lines[index + 2] if index + 2 < len(lines) else ""
    return header if separator.lstrip().startswith("|") and set(separator.strip()) <= set("|-: ") else ""


def sections() -> set:
    """Every section number the numbered documents of this repository define."""
    out = set()
    for doc in SECTION_DOCS:
        out |= set(HEADING.findall((REPO / doc).read_text(errors="replace")))
    return out


def scan(name, lines, items, tests, basenames, paths, modules, numbers):
    checked, findings, notes, section_refs = 0, [], [], 0
    for number, line in enumerate(lines, start=1):
        context = (line + " " + table_header(number - 1, lines)).lower()
        gone = any(marker in context for marker in GONE_MARKERS)
        for match in SECTION.finditer(line):
            section_refs += 1
            if match.group(1) in numbers or name in EXTERNAL_SECTIONS:
                continue
            checked += 1
            (notes if gone else findings).append(
                f"{name}:{number}: §{match.group(1)} is not a section of any numbered document here "
                f"({', '.join(SECTION_DOCS)})")
        for match in QUALIFIED.finditer(line):
            checked += 1
            chain = match.group(1)
            last = chain.split("::")[-1]
            if last in tests or last in items or last + ".rs" in basenames:
                continue
            (notes if gone else findings).append(f"{name}:{number}: `{chain}` names nothing in the tree")
        for match in TLA_CITE.finditer(line):
            module, member = match.groups()
            if module not in modules:
                continue          # a Rust type that looks like a module name, not a model citation
            checked += 1
            # a definition, with or without parameters (`FailRequest(i) ==` is an action)
            if not re.search(rf"^{re.escape(member)}\s*(?:\([^)]*\))?\s*==", modules[module], re.M):
                (notes if gone else findings).append(
                    f"{name}:{number}: `{module}::{member}` is not defined in verification/tla/{module}.tla")
        for match in PATH_RE.finditer(line):
            path = match.group(1)
            if not path.startswith(PATH_PREFIXES) or path in paths:
                continue
            checked += 1
            (notes if gone else findings).append(f"{name}:{number}: `{path}` does not exist")
        for match in RS_BASENAME.finditer(line):
            basename = match.group(1)
            checked += 1
            if basename not in basenames:
                (notes if gone else findings).append(f"{name}:{number}: `{basename}` does not exist")
    return checked, findings, notes, section_refs


def prose_without_code(text: str) -> list[tuple[int, str]]:
    """`(line number, text)` for a markdown file with fenced blocks and inline code spans blanked.

    A formula inside a code span can look like a link (`[](Entitled(i, "shell"))` in `V2Authority`'s prose) and a
    table shown in a fence is an example, not this document's; both are dropped before the links are read.
    """
    out, fenced = [], False
    for number, line in enumerate(text.split("\n"), 1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        if fenced:
            continue
        out.append((number, CODE_SPAN.sub("`…`", line)))
    return out


def broken_links(name: str, text: str) -> tuple[int, list]:
    """`(relative links read, findings)` for `name`: a target that does not exist against its own directory."""
    out, read = [], 0
    base = pathlib.Path(name).parent
    for number, line in prose_without_code(text):
        for label, target in LINK.findall(line):
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            path = urllib.parse.unquote(target.split("#", 1)[0])
            if not path:
                continue
            read += 1
            if not (base / path).exists():
                out.append(f"{name}:{number}: `[{label[:40]}]({target})` names nothing in the tree")
    return read, out


def make_citations(name: str, lines: list, declared: set) -> tuple[int, list, list, set]:
    """`(commands read, findings, notes)` for the `make <target>` commands `name` tells a reader to run.

    Only the two shapes a reader pastes count: an inline code span whose whole content is a `make` command, and a
    fenced-block line that begins with one (an optional `$ ` prompt is stripped). A line that records the target as
    gone, or that is discussing the Makefile's declarations (a table header marking a removal counts, as in
    `scan`), is a note rather than a finding.
    """
    read, findings, notes, targets = 0, [], [], set()
    fenced = False
    for number, line in enumerate(lines, start=1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        commands = []
        if fenced:
            source = line.strip()
            if source.startswith("$ "):
                source = source[2:].strip()
            match = MAKE_CMD.match(source)
            if match:
                commands.append(match.group(1))
        else:
            for span in CODE_SPAN.finditer(line):
                match = MAKE_CMD.match(span.group(0).strip("`").strip())
                if match:
                    commands.append(match.group(1))
        if not commands:
            continue
        context = (line + " " + table_header(number - 1, lines)).lower()
        absent = DECLARATION_TALK in context or any(marker in context for marker in GONE_MARKERS)
        for target in commands:
            read += 1
            targets.add(target)
            if target in declared:
                continue
            message = f"{name}:{number}: `make {target}` names no `.PHONY` target"
            (notes if absent else findings).append(message)
    return read, findings, notes, targets


def main():
    files = tracked()
    items, tests, basenames = universe(files)
    paths = set(files)
    declared = set(phony_targets((REPO / "Makefile").read_text(errors="replace")))
    modules = tla_members()
    numbers = sections()
    cited_targets = set()
    checked, links, made, refs, markdown, rust = 0, 0, 0, 0, 0, 0
    findings, notes = [], []
    for stale in sorted(set(EXTERNAL_SECTIONS) - set(files)):
        findings.append(f"EXTERNAL_SECTIONS lists {stale}, which is not a tracked file anymore: an exemption "
                        "cannot outlive its reason")
    for name in files:
        if not name.endswith((".md", ".rs")) or name.startswith(EXCLUDED):
            continue
        text = (REPO / name).read_text(errors="replace")
        seen, found, noted, seen_refs = scan(name, text.split("\n"), items, tests, basenames, paths, modules,
                                            numbers)
        checked += seen
        refs += seen_refs
        findings += found
        notes += noted
        if name.endswith(".md"):
            markdown += 1
        else:
            rust += 1
        if name.endswith(".md"):
            read, broken = broken_links(name, text)
            links += read
            findings += broken
            commands, missing, records, cited = make_citations(name, text.split("\n"), declared)
            made += commands
            cited_targets |= cited
            findings += missing
            notes += records
    # (D-223) the counts this docstring states are compared with the tree, the way verification_catalogue's are:
    # a number in the prose that describes an audit must be one the audit recomputes (D-208).
    stated = STATED_COUNTS.search(__doc__ or "")
    if stated is None:
        findings.append("this script's docstring no longer states its counts in the compared form "
                        "(`**N** markdown files and **N** Rust files carry **N** citations, ...`), so the rule "
                        "that checks them has nothing to read")
    else:
        want = [markdown, rust, checked, links, made, refs]
        labels = ["markdown files", "Rust files", "citations", "relative links", "`make` commands",
                  "`§`-section references"]
        for label, said, real in zip(labels, (int(x) for x in stated.groups()), want):
            if said != real:
                findings.append(f"this script's docstring says {said} {label}, the tree has {real}: a count in the "
                                "prose that describes an audit has to be one the audit recomputes (D-223)")
    # (D-234) The index page states the same counts in prose, and nothing read them: its row said "79 relative
    # links resolve today" and "418 commands, 22 distinct targets, 19 declared" while the audit had moved to 81
    # and 462 (measured 2026-09-27). The page is what a reviewer trusts about the audits, so its two sentences are
    # held to what this run computes — the same rule the docstring above follows (D-208/D-223), one file over.
    page = (REPO / "review" / "README.md").read_text(errors="replace")
    declared_cited = len({target for target in cited_targets if target in declared})
    for pattern, what, real in (
        (r"(\d+) relative links resolve today", "relative links", (links,)),
        (r"(\d+) commands, (\d+) distinct targets, (\d+) declared", "make citations",
         (made, len(cited_targets), declared_cited)),
    ):
        stated = re.search(pattern, page)
        if stated is None:
            findings.append(f"review/README.md no longer states the {what} count in the form this audit reads, "
                            "so the page and the run cannot be compared")
        elif tuple(int(group) for group in stated.groups()) != real:
            said = ", ".join(str(group) for group in stated.groups())
            findings.append(f"review/README.md says {what} = {said}, this run has "
                            f"{', '.join(str(value) for value in real)}: the page's sentences have to be numbers "
                            "the audit recomputes (D-234)")
    for note in notes:
        print(f"note: {note}")
    for finding in findings:
        print(finding)
    print(f"\n{checked} citations checked against {len(basenames)} files, {len(tests)} tests and "
          f"{len(items)} items, plus {links} relative link(s), {made} `make` command(s) over "
          f"{len(cited_targets)} distinct target(s) and {refs} `§`-section reference(s): {len(findings)} "
          f"unexplained, {len(notes)} recorded as removed")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Public items nothing *calls* (the audit behind D-78, D-86 and D-130).

`clippy` cannot help here: `dead_code` only fires for private items, so a `pub fn`/`pub struct` that no
caller anywhere in the tree mentions is silent — and this repository has three times found real leftovers
that way (removed writers, a worktree helper reading a layout that no longer exists, the retired v1 tool
executors). This script is that audit, mechanically and re-runnably:

    python3 review/dead_code.py [--list-known]

It scans the three crates' `src` for public item definitions (`fn`, `struct`, `enum`, `const`, `static`,
`type`, `trait`) and counts every *mention* of each name, splitting the tree by whether a mention is a
**call**:

- a call is a mention in the product's own code: `core/src`, `engine/src`, `tui/src` outside a
  `#[cfg(test)] mod`;
- nothing else counts. Integration tests and `*_tests.rs`, `engine/examples` and `engine/benches` are how the
  product is *used*, not code the product runs; and Rust comments are prose — both a `///` doc comment and an
  ordinary `//` one (D-443: the `//` half was a blind spot, and `driver::cancel_turn` sat inside it, kept by a
  comment in the supervisor). Prose under `docs/**` and
  `review/**/*.md` was already excluded (D-86: the word "official" in DESIGN masked `Anthropic::official`);
  D-130 applied the same rule to a test and to a `///` line, because neither "a test drives it" nor "a doc
  mentions it" is a call — `workspace::merge_branch` and `driver::cancel_turn` hid behind exactly those.
- a name mentioned in the scripts and `Makefile` that drive the CLI *is* called, just from outside Rust.

A definition that is itself `#[cfg(test)]`-gated is test-only by construction and is not scanned.

Three buckets, each with an allowlist that records the decision keeping the name, so a new finding stands out
and the exit code stays meaningful. The allowlists are checked too (D-443), because an exemption is a claim
nobody re-measures: every reason must name its authority — a binding decision (`D-<n>`) or the literal
`mechanical:` for a structural one (a DESIGN data contract, a test-support hook) — and an entry doing no work
is itself a finding, since a stale entry hides the next finding of that name. `merge_branch` was exactly that
case when the rule landed: D-252 had wired the merge surface the entry still called an open item.

| Bucket     | Meaning                                          | Allowlist          |
|------------|--------------------------------------------------|--------------------|
| `uncalled` | no mention anywhere                              | `KNOWN_UNCALLED`   |
| `test_only`| only tests, examples or benches mention it       | `KNOWN_TEST_ONLY`  |
| `doc_only` | only a doc comment mentions it                   | `KNOWN_DOC_ONLY`   |
"""
import argparse
import collections
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
CODE_ROOTS = ["core/src", "engine/src", "tui/src"]            # the product's own code
NON_CODE_ROOTS = ["core/tests", "engine/tests", "tui/tests", "engine/examples", "engine/benches"]
USER_FILES = ["Makefile", "install.sh", "tui/scripts/pty_v2_smoke.py", "tui/scripts/pty_screen.py",
              "review/install_check.py", "review/dogfood/input_latency.py", "review/dogfood/tui.py"]

# name -> why it may be mentioned nowhere at all
KNOWN_UNCALLED = {
    "wait_idle": "D-63: the parked substrate for interrupt-and-redirect; documented with a ponytail note",
    # DESIGN §4.1 names these row shapes as the minimal data contract; the code reads them through SQL
    # (store.rs owns the schema), so the typed form documents the contract instead of being called. Their
    # goals/artifacts siblings (`Goal`, `Artifact`) sit in KNOWN_DOC_ONLY for the same reason.
    "Envelope": "mechanical: DESIGN §4.1 minimal data contract (Envelope / ContextEntry), read through SQL",
    "ModelRequestRecord": "mechanical: DESIGN §4.1 minimal data contract (ModelRequest / Attempt), read through "
                          "SQL",
    "Attempt": "mechanical: DESIGN §4.1 minimal data contract (ModelRequest / Attempt), read through SQL",
}

# name -> why tests/examples/benches may mention it and no product code needs to
KNOWN_TEST_ONLY = {
    "run_reference": "mechanical: the eval group-A direct-drive reference loop (reference.rs module doc: "
                     "evaluation only, no production recovery promise); the comparison baseline its tests drive",
    "inject": "mechanical: jobs::client's own doc calls it a test hook (only honored by runners spawned with "
              "the hooks env)",
    "with_control": "mechanical: driver test-support hook: runs one closure on the storage worker so a test can "
                    "inject a real storage-level condition (the doc names the page cap for A31)",
    "with_stream_stall": "mechanical: provider test-support hook: makes a stream stall so the retry path can be "
                         "driven",
    "load_user_config": "D-244: the user-only loader, kept as the reference the merge path is compared against "
                        "(`the_two_loaders_agree_on_a_user_only_config`) — the product loads through "
                        "`load_user_config_for`, so only tests call this one",
}

# name -> why only a doc comment mentions it
KNOWN_DOC_ONLY = {
    "Goal": "mechanical: DESIGN §4.1 minimal data contract (the goal row, §4.2/§8), read through SQL",
    "Artifact": "mechanical: DESIGN §4.1 minimal data contract (the artifact row, §9.1), read through SQL",
    "Operation": "mechanical: DESIGN §4.1 minimal data contract (the operation row, §6.1), read through SQL",
    "cancel_turn": "D-63: the parked substrate for interrupt-and-redirect (D-78 kept it with a ponytail note "
                   "on `wait_idle`); the driver's own doc is its only other mention",
}

DEFINITION = re.compile(
    r"^\s*pub(?:\(crate\))? (?:async )?(?:unsafe )?(?:extern \"[^\"]+\" )?(fn|struct|enum|const|static|type|trait) (\w+)")
CFG_TEST = "#[cfg(test)]"
TEST_MOD = re.compile(r"^mod \w+")
CLOSE = re.compile(r"^}")
DOC_LINE = re.compile(r"^\s*(///|//!|/\*\*|\*)")
# An ordinary `//` comment is prose too (D-443): the audit used to count it as a call, which is why
# `driver::cancel_turn` looked called — one comment in the supervisor mentions it — while its exemption sat
# silently inert. Excluding the whole comment family changed no other classification in the tree (measured).
COMMENT = re.compile(r"^\s*//")
ATTRIBUTE = re.compile(r"^\s*#\[")
AUTHORITY = re.compile(r"D-\d+|mechanical:")


def all_entries() -> dict:
    """Every allowlist name with its reason, refusing a name in two buckets: one authority per name (D-443)."""
    merged: dict = {}
    for table in (KNOWN_UNCALLED, KNOWN_TEST_ONLY, KNOWN_DOC_ONLY):
        for name, reason in table.items():
            if name in merged:
                raise SystemExit(f"the allowlists name {name} in two buckets: one reason per name")
            merged[name] = reason
    return merged


def rust_files(roots: list) -> list:
    return [p for root in roots for p in pathlib.Path(REPO / root).rglob("*.rs")]


def test_spans(lines: list) -> list:
    """`(start, end)` 0-based inclusive spans of `#[cfg(test)] mod X { … }` (rustfmt closes at column 0)."""
    spans = []
    for i in range(len(lines) - 1):
        if lines[i].strip() == CFG_TEST and TEST_MOD.match(lines[i + 1] or ""):
            j = i + 1
            while j < len(lines) and not CLOSE.match(lines[j]):
                j += 1
            spans.append((i, j))
    return spans


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list-known", action="store_true", help="also list every allowed name with its reason")
    args = parser.parse_args()

    prod_files = [(p, p.read_text()) for p in rust_files(CODE_ROOTS)]
    other_files = [(p, p.read_text()) for p in rust_files(NON_CODE_ROOTS)]

    definitions: dict = collections.defaultdict(list)
    skipped = 0
    for path, text in prod_files:
        lines = text.splitlines()
        in_test = test_spans(lines)
        inside = lambda n: any(a < n <= b for a, b in in_test)  # noqa: E731
        for number, line in enumerate(lines, 1):
            match = DEFINITION.match(line)
            if not match or inside(number):
                continue
            # a definition whose own attribute block carries #[cfg(test)] is test-only by construction
            j = number - 2
            gated = False
            while j >= 0 and (DOC_LINE.match(lines[j]) or ATTRIBUTE.match(lines[j])):
                if lines[j].strip() == CFG_TEST:
                    gated = True
                    break
                j -= 1
            if gated:
                skipped += 1
                continue
            definitions[match.group(2)].append(f"{path.relative_to(REPO)}:{number}")

    # one token pass over each line (`\bName\b` and a word token agree for identifiers), so the scan costs
    # one regex per line instead of one per name — this is the audit `make hygiene` runs.
    tokens = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
    prod_paths = {p for p, _ in prod_files}
    prod, test, doc = collections.Counter(), collections.Counter(), collections.Counter()
    for path, text in prod_files + other_files:
        is_test_file = path not in prod_paths
        lines = text.splitlines()
        spans = [] if is_test_file else test_spans(lines)
        for number, line in enumerate(lines, 1):
            found = collections.Counter(tokens.findall(line))
            if not found:
                continue
            in_test = is_test_file or any(a < number <= b for a, b in spans)
            is_doc = bool(DOC_LINE.match(line)) or bool(COMMENT.match(line))
            for word, count in found.items():
                if word not in definitions:
                    continue
                if in_test:
                    test[word] += count
                elif is_doc:
                    doc[word] += count
                else:
                    prod[word] += count
    drivers = "\n".join((REPO / name).read_text(errors="ignore")
                        for name in USER_FILES if (REPO / name).is_file())

    # The bucket follows the mention that would otherwise be mistaken for a call: a test first (D-130's
    # headline case), then a doc comment, then nothing at all. A name is allowed only by its own bucket's
    # table, so an item that changes shape is re-decided instead of staying quietly allowed.
    buckets = [
        ("reached only from tests/examples/benches", "test_only", KNOWN_TEST_ONLY),
        ("reached only from a doc comment", "doc_only", KNOWN_DOC_ONLY),
        ("uncalled (no mention anywhere)", "uncalled", KNOWN_UNCALLED),
    ]
    findings: dict = {key: [] for _label, key, _table in buckets}
    allowed: set = set()
    for name, where in sorted(definitions.items()):
        if prod[name] - len(where) > 0 or re.search(r"\b" + re.escape(name) + r"\b", drivers):
            continue
        key = "test_only" if test[name] > 0 else "doc_only" if doc[name] > 0 else "uncalled"
        table = next(table for _label, k, table in buckets if k == key)
        if name in table:
            allowed.add(name)
        else:
            findings[key].append((name, where))

    # The allowlists are checked as well, because an exemption is a claim (D-443): every reason must name its
    # authority, and an entry doing no work is itself a finding. `merge_branch` was in exactly that position
    # when the rule landed — D-252 wired the merge surface the entry still called an open item.
    entries = all_entries()
    unjustified = sorted(name for name, reason in entries.items() if not AUTHORITY.search(reason))
    stale = sorted(set(entries) - allowed)

    total = sum(len(rows) for rows in findings.values())
    print(f"{len(definitions)} public items scanned ({skipped} skipped as cfg(test)-gated); "
          f"{total} uncalled, {len(allowed)} known and allowed")
    for label, key, _table in buckets:
        for name, where in findings[key]:
            print(f"  {label}: {name} at {where[0]}")
    for name in stale:
        print(f"  stale exemption: {name} is no longer a finding — remove it from the allowlist")
    for name in unjustified:
        print(f"  exemption without an authority: {name} must cite a `D-<n>` or state `mechanical:`")
    if args.list_known:
        for table, label in ((KNOWN_UNCALLED, "uncalled"), (KNOWN_TEST_ONLY, "test-only"),
                             (KNOWN_DOC_ONLY, "doc-only")):
            for name, reason in table.items():
                print(f"  allowed ({label}): {name} — {reason}")
    return 1 if total or stale or unjustified else 0


if __name__ == "__main__":
    sys.exit(main())

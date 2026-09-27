#!/usr/bin/env python3
"""The CLI's `--json` reports and the documentation, held together (the detector behind D-154).

Seven verbs print one JSON object instead of text — `exec`, `authority`, `approvals`, `instances`, `tasks`,
`runners`, `artifacts` — and
that object is a *scripting* surface: a CI job, a wrapper script or the evaluation harness reads fields out of
it, so a field that is renamed or dropped breaks callers while every test in the tree stays green. The
protocol, the events, the tools and the config each had a catalogue audit (`protocol_catalogue.py`,
`event_catalogue.py`, `tool_catalogue.py`, `config_reference.py`); this surface had none, and the report's own
fields were partly undocumented (`docs/USER-GUIDE.md` §1.1 named four of `exec`'s sixteen).

    python3 review/exec_report.py

It reads the field names out of the source — every `json!({ … })` literal in the six report modules that
carries `session_id`, which is what makes a literal a *printed report* rather than a protocol request or an
event payload — and compares the union with the fields `docs/USER-GUIDE.md` §1.2 catalogues, in both
directions:

* a field the reports can carry and the catalogue does not name is a finding (undocumented contract);
* a field the catalogue names and no report carries is a finding too (a stale name is how a rename hides).

`docs/PROTOCOL.md` keeps the fields of the *rows* inside those reports (`instances[]`, `tasks[]`,
`approvals[]`, `grants[]`), so this audit deliberately stops at the top level.
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
SOURCES = ("engine/src/v2/exec.rs", "engine/src/v2/authority.rs", "engine/src/v2/approvals.rs",
           "engine/src/v2/intervene.rs", "engine/src/v2/runners.rs", "engine/src/v2/artifacts.rs")
GUIDE = REPO / "docs/USER-GUIDE.md"
SECTION = "### 1.2 The JSON reports"


def literals(text: str):
    """Every `json!({ … })` in `text` as `(line, {top-level keys})`, nested keys excluded."""
    for match in re.finditer(r"json!\(\s*\{", text):
        start = match.end() - 1
        depth, index = 0, start
        while index < len(text):
            if text[index] in "[{":
                depth += 1
            elif text[index] in "]}":
                depth -= 1
                if depth == 0:
                    break
            index += 1
        body = text[start:index + 1]
        keys, depth = [], 0
        for token in re.finditer(r'"(?:[^"\\]|\\.)*"|[{}\[\]]', body):
            value = token.group(0)
            if value in "[{":
                depth += 1
            elif value in "]}":
                depth -= 1
            elif depth == 1 and value.startswith('"') and body[token.end():].lstrip().startswith(":"):
                keys.append(value[1:-1])
        yield text[:match.start()].count("\n") + 1, keys


def reported() -> dict:
    """`field -> [file:line, …]` for every field a printed report carries."""
    found: dict[str, list[str]] = {}
    for name in SOURCES:
        path = REPO / name
        for line, keys in literals(path.read_text()):
            if "session_id" not in keys:      # not a report: a request, an event payload, a check verdict
                continue
            for key in keys:
                found.setdefault(key, []).append(f"{name}:{line}")
    return found


def catalogued() -> set:
    """The field names §1.2's *table* documents: second cell of every row between header and blank line.

    The prose that follows the table names values (`end`'s words, the nested `verification` fields) and verbs
    on purpose, so the audit reads the table only — a catalogue that cannot be told from prose is not one.
    """
    text = GUIDE.read_text()
    start = text.find(SECTION)
    if start < 0:
        return set()
    end = text.find("\n## ", start)          # the section ends at the next top-level heading
    section = text[start:end if end > 0 else len(text)]
    rows = re.findall(r"^\|([^|]+)\|([^|]+)\|\s*$", section, re.M)
    names = set()
    for verb, fields in rows:
        if verb.strip().replace("-", "").strip() == "" or "field" in verb.lower() or "verb" in verb.lower():
            continue          # the separator row and the header
        names |= set(re.findall(r"`([a-z_][a-z0-9_]*)`", fields))
    return names


def main() -> int:
    fields = reported()
    documented = catalogued()
    if not fields or not documented:
        print(f"FAIL: nothing to compare (reports: {len(fields)} fields, catalogue: {len(documented)} names)")
        return 1
    findings = []
    for name in sorted(set(fields) - documented):
        findings.append(f"the reports carry `{name}` ({fields[name][0]}) and §1.2 does not name it")
    for name in sorted(documented - set(fields)):
        findings.append(f"§1.2 names `{name}` and no report carries it any more")
    for finding in findings:
        print(finding)
    print(f"{len(fields)} report fields, {len(documented)} catalogued names: {len(findings)} unexplained")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

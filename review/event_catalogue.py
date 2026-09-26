#!/usr/bin/env python3
"""Keep `docs/EVENTS.md` equal to the events the control plane actually emits (the detector behind D-125).

The event log is a product surface: the daemon's `events` request is how a client reconnects from a watermark,
`exec` reads it for its report, probes assert on it, and an operator can read `session.sqlite` directly. DESIGN.md
describes *that* it exists and what it carries (a per-session sequence, a scope, a payload) but never listed the
kinds — 45 of them, a third of which nothing in this tree reads. The catalogue is `docs/EVENTS.md`, and this script
keeps it equal to the code:

    python3 review/event_catalogue.py            # check (inside `make hygiene`)
    python3 review/event_catalogue.py --write    # regenerate the table in place

The table between the generated markers lists, for every `event(...)` call site in `core/src`, the kind, the
scope expression, the payload keys and the files outside `core/src` that mention the kind (its in-tree readers;
"observability-only" is an honest column value, not a defect). A kind added to the code but missing from the
document, a payload key that changed, or a documented kind that no longer exists all fail the check.

Limits: the arguments are split by commas and the payload keys are read from a `json!({…})` literal, so a payload
built at runtime is invisible; the readers column is a text search over `tui/`, `engine/`, `review/` and `docs/`,
so a reader that matches a kind dynamically (a prefix or a list) may not be credited.
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
DOC = REPO / "docs" / "EVENTS.md"
BEGIN = "<!-- generated: begin -->"
END = "<!-- generated: end -->"
# Who *consumes* a kind: the TUI, the engine, the test suites and the probes. Prose (docs/, review/*.md) is
# deliberately not a reader — a decision entry naming a kind documents it, and counting that would make
# the column flip whenever the log is written about (D-125).
READER_ROOTS = ["tui", "engine/src", "engine/tests", "core/tests"]
CALL = re.compile(r'\bevent\(')


def brace_block(text, start):
    depth, index = 0, start
    while index < len(text):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[start:index + 1]
        index += 1
    return text[start:]


def split_args(text, start):
    """The top-level arguments of the call whose first argument starts at `start`."""
    args, depth, index, current = [], 0, start, start
    while index < len(text):
        character = text[index]
        if character in "([{":
            depth += 1
        elif character in ")]}":
            if depth == 0:
                args.append(text[current:index])
                return args
            depth -= 1
        elif character == "," and depth == 0:
            args.append(text[current:index])
            current = index + 1
        index += 1
    return args


def payload_keys(expression):
    marker = expression.find("json!(")
    if marker == -1:
        return None
    block = brace_block(expression, expression.find("{", marker))
    body, keys, depth, index = block[1:-1], [], 0, 0
    while index < len(body):
        character = body[index]
        if character in "{[(":
            depth += 1
        elif character in "}])":
            depth -= 1
        elif character == '"' and depth == 0:
            match = re.match(r'"([A-Za-z_][A-Za-z0-9_]*)":', body[index:])
            if match:
                keys.append(match.group(1))
                index += match.end()
                continue
            closing = body.find('"', index + 1)
            index = closing + 1 if closing != -1 else index + 1
            continue
        index += 1
    return keys


def emitted():
    """kind -> {"scope": str, "keys": [str], "sites": [file:line]}."""
    events = {}
    for path in sorted((REPO / "core/src").rglob("*.rs")):
        text = path.read_text(errors="replace")
        for match in CALL.finditer(text):
            args = split_args(text, match.end())
            if len(args) < 5:
                continue
            kind = re.fullmatch(r'\s*"([a-z_]+)"\s*', args[2])
            if kind is None:
                continue
            entry = events.setdefault(kind.group(1), {"scope": [], "keys": set(), "sites": []})
            entry["scope"].append(args[3].strip())
            keys = payload_keys(args[4])
            if keys is not None:
                entry["keys"].update(keys)
            entry["sites"].append(f"{path.relative_to(REPO)}:{text.count(chr(10), 0, match.start()) + 1}")
    return events


def reader_texts():
    """Every file outside `core/src` that could read a kind by name, read once (the check scans per kind)."""
    texts = {}
    for root in READER_ROOTS + ["review"]:
        for path in sorted((REPO / root).rglob("*")):
            if not path.is_file() or path == DOC or "tmp" in path.parts:
                continue
            if path.name == "event_catalogue.py":
                continue
            if path.suffix not in (".rs", ".py") and not (str(path).startswith("review") and path.suffix == ".py"):
                continue
            try:
                texts[str(path.relative_to(REPO))] = path.read_text(errors="replace")
            except OSError:
                continue
    return texts


def readers(kind, texts):
    return [name for name, text in texts.items()
            if f'"{kind}"' in text or f"'{kind}'" in text or f"`{kind}`" in text]


def table(events, texts):
    lines = ["| Event | Scope | Payload | Emitted at | Read by (outside core/src) |", "|---|---|---|---|---|"]
    for kind in sorted(events):
        info = events[kind]
        scopes = sorted(set(info["scope"]))
        keys = ", ".join(f"`{key}`" for key in sorted(info["keys"])) or "—"
        sites = ", ".join(f"`{site}`" for site in sorted(info["sites"]))
        seen = readers(kind, texts)
        who = ", ".join(f"`{one}`" for one in seen) if seen else "observability only"
        lines.append(f"| `{kind}` | {' / '.join(f'`{s}`' for s in scopes)} | {keys} | {sites} | {who} |")
    return "\n".join(lines)


def main(argv):
    events = emitted()
    texts = reader_texts()
    generated = table(events, texts)
    if "--write" in argv:
        text = DOC.read_text()
        start, end = text.index(BEGIN), text.index(END)
        DOC.write_text(text[:start] + BEGIN + "\n\n" + generated + "\n\n" + text[end:])
        print(f"docs/EVENTS.md regenerated: {len(events)} kinds")
        return 0
    text = DOC.read_text()
    documented = set(re.findall(r"^\| `([a-z_]+)` \|", text, re.M))
    findings = []
    for kind in sorted(set(events) - documented):
        findings.append(f"{kind}: emitted by {events[kind]['sites'][0]} but not in docs/EVENTS.md")
    for kind in sorted(documented - set(events)):
        findings.append(f"{kind}: documented but no `event(…, \"{kind}\", …)` call site exists")
    if BEGIN in text and END in text:
        start, end = text.index(BEGIN) + len(BEGIN), text.index(END)
        if text[start:end].strip() != generated:
            findings.append("the generated table does not match the code: run `python3 review/event_catalogue.py --write`")
    else:
        findings.append("docs/EVENTS.md has no generated table markers")
    for finding in findings:
        print(finding)
    if findings:
        return 1
    unread = [kind for kind in sorted(events) if not readers(kind, texts)]
    print(f"{len(events)} event kinds documented and in sync; {len(unread)} with no reader in this tree "
          f"({', '.join(unread) if unread else 'none'})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

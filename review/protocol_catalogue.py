#!/usr/bin/env python3
"""Keep `docs/PROTOCOL.md` equal to the daemon's dispatcher (the detector behind D-126).

The Unix-socket protocol is the API: the TUI, `exec`, the CLI verbs and every probe speak it, and DESIGN.md
describes it as "one JSON-lines protocol (`/v1`)" without listing a single method. `docs/PROTOCOL.md` is the
catalogue, generated from the two dispatch points in the code:

* the read methods the daemon answers from the session database (`read_method` in `engine/src/v2/daemon.rs`,
  cross-checked against the whitelist in `handle` that decides which methods are read-only), and
* the commands the control plane executes (`fn dispatch` in `core/src/v2/control.rs`), with the parameters each
  handler reads and whether the handler takes an identity (i.e. whether authorization applies).

and one more table from the same file: **every field the protocol carries in one JSON object**, with the function
or read arm that builds it. The arrays a client reads (`instances[]`, `tasks[]`, `approvals[]`, `grants[]`) and a
checkpoint's `goal` object are rows of the daemon's own views; the guide's §1.2 calls them part of the scripting
contract and points *here* for their fields, and until this table existed a row field could be added to the
protocol without any document changing — `reason` on the instance rows arrived exactly that way (D-165).

    python3 review/protocol_catalogue.py            # check (inside `make hygiene`)
    python3 review/protocol_catalogue.py --write     # regenerate the tables in place

A method added to either dispatcher without a line in the document, a documented method that no longer exists, a
changed parameter list or a stale table fails the check.

The same holds for the field table, generated from the `json!({ … })` literals in `engine/src/v2/daemon.rs`: a
field added, removed or renamed in any protocol object changes it.

Limits: handler parameters are the ones the handler *itself* reads (`params["…"]` in its own body), so a field a
helper reads is not listed (that is a documentation gap, not a wrong entry); the tables are generated from
`match` arms, so a method built at runtime would be missing entirely, and a protocol object built at runtime is
invisible to the field table for the same reason.
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
DOC = REPO / "docs" / "PROTOCOL.md"
BEGIN = "<!-- generated: begin -->"
END = "<!-- generated: end -->"
DAEMON = REPO / "engine" / "src" / "v2" / "daemon.rs"
CONTROL = REPO / "core" / "src" / "v2" / "control.rs"
# the two markers a literal can sit under: a read arm of the dispatcher, or the function that builds it
MARKERS = ((re.compile(r'^\s*"([a-z_]+)" => \{', re.M), True),
           (re.compile(r'^(?:pub(?:\(crate\))? )?(?:async )?fn ([a-z_]+)', re.M), False))
JSON_OBJECT = re.compile(r"json!\(\s*\{")
# the heading of the generated field table: the method scan stops there, and `tables()` writes it
FIELDS_HEADING = "### Every field the protocol carries in one object, and where it is built"


def enclosing(text: str, position: int) -> str:
    """The function or read arm a literal sits in: the nearest preceding marker, as a doc reference."""
    found = (-1, "", False)
    for pattern, is_arm in MARKERS:
        for match in pattern.finditer(text, 0, position):
            if match.start() > found[0]:
                found = (match.start(), match.group(1), is_arm)
    _, name, is_arm = found
    return f"`{name}`" + (" arm" if is_arm else "")


def literal_fields(text: str):
    """[(where, [fields])] for every `json!({ … })` in the protocol file, in source order.

    The keys are the literal's own top level; a nested `json!` (the goal object inside a snapshot) is a literal
    of its own and is picked up by the same scan, so no key is counted twice.
    """
    out = []
    for match in JSON_OBJECT.finditer(text):
        start = text.rindex("{", match.start(), match.end())
        keys, depth, index = [], 0, start
        while index < len(text):
            character = text[index]
            if character == "{":
                depth += 1
            elif character == "}":
                depth -= 1
                if depth == 0:
                    break
            elif character == '"' and depth == 1:
                token = re.match(r'"(?:[^"\\]|\\.)*"', text[index:])
                if token and text[index + token.end():].lstrip().startswith(":"):
                    keys.append(token.group(0)[1:-1])
                index += token.end() - 1 if token else 0
            index += 1
        out.append((enclosing(text, match.start()), keys))
    return out


def read_methods():
    """[(name, params the read arm reads, reply keys, site)] from `read_method`."""
    text = DAEMON.read_text(errors="replace")
    start = text.index("fn read_method(")
    body = text[start:]
    arms = list(re.finditer(r'^\s{8}"([a-z_]+)" => \{$', body, re.M))
    whitelist_match = re.search(r'match method \{\s*\n\s*((?:"[a-z_]+"\s*\|\s*)*"[a-z_]+")\s*=>', text)
    whitelist = set(re.findall(r'"([a-z_]+)"', whitelist_match.group(1))) if whitelist_match else set()
    out = []
    for index, arm in enumerate(arms):
        end = arms[index + 1].start() if index + 1 < len(arms) else len(body)
        chunk = body[arm.start():end]
        params = sorted(set(re.findall(r'params\.get\("([A-Za-z_][A-Za-z0-9_]*)"\)', chunk)))
        replies = sorted(set(re.findall(r'"([A-Za-z_][A-Za-z0-9_]*)":', chunk)))
        line = text.count("\n", 0, start + arm.start()) + 1
        out.append((arm.group(1), params, replies, f"`engine/src/v2/daemon.rs:{line}`", arm.group(1) in whitelist))
    return out


def commands():
    """[(name, params the handler reads, identity, site)] from `fn dispatch`."""
    text = CONTROL.read_text(errors="replace")
    start = text.index("fn dispatch(")
    body = text[start:]
    arms = list(re.finditer(r'^\s{8}"([a-z_]+)" => (?:([a-z_]+))?\(', body, re.M))
    out = []
    for arm in arms:
        name, handler = arm.group(1), arm.group(2)
        chunk = body[arm.start():arm.start() + 200]
        identity = bool(handler) and "identity" in chunk
        params = []
        if handler:
            functions = re.search(rf'^fn {handler}\((.*?)^\}}', text, re.M | re.S)
            if functions:
                params = sorted(set(re.findall(r'params\["([A-Za-z_][A-Za-z0-9_]*)"\]', functions.group(1)))
                                | set(re.findall(r'params\.get\("([A-Za-z_][A-Za-z0-9_]*)"\)', functions.group(1))))
        line = text.count("\n", 0, start + arm.start()) + 1
        out.append((name, params, identity, f"`core/src/v2/control.rs:{line}`"))
    return out


def tables():
    reads = read_methods()
    lines = ["### Read methods (answered from the session database)", "",
             "| Method | Parameters | Reply keys | Answered at |", "|---|---|---|---|"]
    for name, params, replies, site, is_read_only in reads:
        lines.append(f"| `{name}` | {', '.join(f'`{p}`' for p in params) or '—'} | "
                     f"{', '.join(f'`{r}`' for r in replies) or '—'} | {site} |")
    if not all(entry[4] for entry in reads):
        missing = [entry[0] for entry in reads if not entry[4]]
        lines.append("")
        lines.append(f"> these arms are not in the read-only whitelist: {missing}")
    lines += ["", "### Commands (executed by the control plane)", "",
              "| Method | Parameters the handler reads | Identity checked | Handled at |", "|---|---|---|---|"]
    for name, params, identity, site in commands():
        lines.append(f"| `{name}` | {', '.join(f'`{p}`' for p in params) or '—'} | "
                     f"{'yes' if identity else 'no'} | {site} |")
    lines += ["", "### Every field the protocol carries in one object, and where it is built", "",
              "The rows a client reads (`instances[]`, `tasks[]`, `approvals[]`, `grants[]`, `entries[]`), a",
              "checkpoint's `goal` object and the greeting/reply envelopes are all built as `json!({ … })`",
              "literals in `engine/src/v2/daemon.rs`; this table is that list. A row is one literal.", "",
              "| Built at | Fields |", "|---|---|"]
    for where, fields in literal_fields(DAEMON.read_text(errors="replace")):
        lines.append(f"| {where} | {', '.join(f'`{field}`' for field in fields) or '—'} |")
    return "\n".join(lines)


def main(argv):
    generated = tables()
    if "--write" in argv:
        text = DOC.read_text()
        start, end = text.index(BEGIN), text.index(END)
        DOC.write_text(text[:start] + BEGIN + "\n\n" + generated + "\n\n" + text[end:])
        print(f"docs/PROTOCOL.md regenerated: {len(read_methods())} read methods, {len(commands())} commands")
        return 0
    text = DOC.read_text()
    # only the two method tables name methods; the field table's first column names functions and read arms
    methods_region = text.split(FIELDS_HEADING, 1)[0]
    documented = (set(re.findall(r"^\| `([a-z_]+)` \|", methods_region, re.M)))
    findings = []
    for name, _, _, _, _ in read_methods():
        if name not in documented:
            findings.append(f"{name}: a read method with no row in docs/PROTOCOL.md")
    for name, _, _, _ in commands():
        if name not in documented:
            findings.append(f"{name}: a command with no row in docs/PROTOCOL.md")
    known = {entry[0] for entry in read_methods()} | {entry[0] for entry in commands()}
    for name in sorted(documented - known):
        findings.append(f"{name}: documented but no dispatcher arm exists")
    if BEGIN in text and END in text:
        start, end = text.index(BEGIN) + len(BEGIN), text.index(END)
        if text[start:end].strip() != generated:
            findings.append("the generated tables do not match the code: run `python3 review/protocol_catalogue.py --write`")
    else:
        findings.append("docs/PROTOCOL.md has no generated table markers")
    for finding in findings:
        print(finding)
    if findings:
        return 1
    print(f"{len(read_methods())} read methods and {len(commands())} commands documented and in sync")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

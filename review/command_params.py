#!/usr/bin/env python3
"""Command payload fields nothing reads (the detector behind D-123 and D-124).

Every control command is a `method` plus a `json!({…})` payload, and the handler in `core/src/v2/control.rs`
reads the fields it needs. A field the caller fills in and the command layer never reads is dropped silently —
and D-123 was exactly that: the driver sent `error_class` on every `record_attempt` (four call sites), the
column existed, and nothing ever wrote it, so the one field that explains a retry was missing from every
attempt row. It was found by a probe that needed it, not by anything that checks.

    python3 review/command_params.py

It reads every `command(…, "method", json!({…}))` call in `engine/src`, takes the *top-level* keys of the
payload, and compares them with the keys the command layer reads (`params["…"]` / `params.get("…")` anywhere
in `control.rs`, so a field read by a helper counts). A field nobody reads is a finding, and it runs inside
`make hygiene`.

Limits: the payload is parsed by brace/key matching, not by a Rust parser — a key built at runtime (a variable
or a spread) is invisible to it, and a field the *client* sends over the protocol (the TUI or `exec`, not the
engine) is out of scope, because those callers are not in this scan. Both limits make it quieter, not wrong:
every finding it does report is a field the engine takes the trouble to fill.
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
SENDERS = "engine/src"
CONTROL = "core/src/v2/control.rs"
CALL = re.compile(r'command\(\s*\n?\s*[^,]+,\s*"([a-z_]+)"\s*,\s*json!\(')
READ = re.compile(r'params\["([A-Za-z_][A-Za-z0-9_]*)"\]|params\.get\("([A-Za-z_][A-Za-z0-9_]*)"\)')


def brace_block(text, start):
    """The `{…}` block that starts at `start`, braces balanced."""
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


def top_level_keys(block):
    """`"key":` names at the block's own depth (nested objects and arrays are skipped)."""
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


def main():
    senders = {}
    for path in sorted(pathlib.Path(REPO / SENDERS).rglob("*.rs")):
        text = path.read_text(errors="replace")
        for match in CALL.finditer(text):
            method = match.group(1)
            block = brace_block(text, text.find("{", match.end() - 1))
            for key in top_level_keys(block):
                senders.setdefault(method, {}).setdefault(key, set()).add(
                    f"{path.relative_to(REPO)}:{text.count(chr(10), 0, match.start()) + 1}")
    read = {one or two for one, two in READ.findall((REPO / CONTROL).read_text(errors="replace"))}
    findings = []
    for method in sorted(senders):
        unread = sorted(key for key in senders[method] if key not in read)
        if unread:
            findings.append((method, unread, senders[method]))
    for method, unread, keys in findings:
        print(f"{method}: the command layer reads no {unread}")
        for key in unread:
            print(f"    {key} sent from {', '.join(sorted(keys[key]))}")
    if findings:
        return 1
    total = sum(len(keys) for keys in senders.values())
    print(f"{len(senders)} commands, {total} payload fields, every one read by {CONTROL}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

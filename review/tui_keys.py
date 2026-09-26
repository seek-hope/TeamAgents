#!/usr/bin/env python3
"""The TUI's keys: what the interface advertises and documents against what it handles (D-157).

Every other documented surface had a catalogue audit — CLI flags (`doc_flags.py`), config keys
(`config_reference.py`), events, tools, protocol methods, the `--json` reports (D-154) — except the terminal
keys, and they are the surface a user is *inside*. `README.md` even makes the claim this audit checks: "TUI keys
(they match the hint line at the bottom …)". A hint that advertises a key nothing handles is the D-130/D-133
shape: a promise with no behaviour behind it, and the only witness was a human pressing the key.

    python3 review/tui_keys.py

Three lists are compared:

* the keys the **hint lines** print (`footer_hint`, the confirmation line, the panel hints) — every one must be
  handled by `tui/src/v2app.rs`;
* the keys `README.md` documents in its TUI paragraph — every one must be handled too;
* `USER-GUIDE.md`'s key mentions (the approvals box, the panels) — same rule.

Only that direction is a finding: a handler no surface advertises (Home/End, Backspace/Delete, plain typing) is
normal and not drift. A key is proven handled by finding it in the handler file, so the audit states its
ceiling: it proves *a handler exists*, not that it is reachable in the view that advertises it (the panel probes
do that for the ones that matter — `tui_panels.py`, `approval.py`).
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
APP = REPO / "tui/src/v2app.rs"
DOCS = (REPO / "README.md", REPO / "docs/USER-GUIDE.md")

# canonical name -> what proves a handler (a regex over the handler file)
HANDLED = {
    "Enter": r"KeyCode::Enter",
    "Esc": r"KeyCode::Esc",
    "Tab": r"KeyCode::Tab",
    "Up": r"KeyCode::Up",
    "Down": r"KeyCode::Down",
    "PageUp": r"KeyCode::PageUp",
    "PageDown": r"KeyCode::PageDown",
    "Left": r"KeyCode::Left",
    "Right": r"KeyCode::Right",
    "Home": r"KeyCode::Home",
    "End": r"KeyCode::End",
    "Backspace": r"KeyCode::Backspace",
    "Delete": r"KeyCode::Delete",
}
# A chord is proved by a *window*: the handlers are nested (`if modifiers.contains(CONTROL) { match code {
# KeyCode::Char('n') => … } }`), so requiring both on one line would report a handler that exists (measured
# 2026-09-27 — the first version of this audit did exactly that for Ctrl+A/Ctrl+N). The window must also name
# `KeyCode`, so a plain `Char('n')` in a typing path next to an unrelated `CONTROL` does not count.
def chord(letter: str) -> str:
    # 1500 characters is measured, not guessed: the chord arms sit next to each other in one `match` under one
    # `KeyModifiers::CONTROL` guard, ~800 chars apart in this file (2026-09-27).
    return (
        rf"(?s)(?=.{{0,1500}}KeyModifiers::CONTROL.{{0,1500}}Char\('{letter}'\)"
        rf"|.{{0,1500}}Char\('{letter}'\).{{0,1500}}KeyModifiers::CONTROL)"
    )


for letter in "acdjnpqrtwy":
    HANDLED[f"Ctrl+{letter.upper()}"] = chord(letter)
# the `Ctrl+←/→` word jumps and the shift-enter newline are written as guards, not as chords
HANDLED["Ctrl+Left"] = r"(?s)(?=.{0,400}CONTROL)(?=.{0,400}KeyCode::Left)"
HANDLED["Ctrl+Right"] = r"(?s)(?=.{0,400}CONTROL)(?=.{0,400}KeyCode::Right)"
HANDLED["Shift+Enter"] = r"(?s)(?=.{0,400}SHIFT)(?=.{0,400}KeyCode::Enter)"

# what the interface prints and what the documents name, as canonical keys
HINT_WORDS = {
    "enter": "Enter", "esc": "Esc", "tab": "Tab", "up": "Up", "down": "Down", "up/down": "Up",
    "pageup": "PageUp", "pagedown": "PageDown", "ctrl+j": "Ctrl+J", "ctrl+n": "Ctrl+N", "ctrl+a": "Ctrl+A",
    "ctrl+w": "Ctrl+W", "ctrl+c": "Ctrl+C", "ctrl+d": "Ctrl+D", "ctrl+left": "Ctrl+Left",
    "ctrl+right": "Ctrl+Right", "shift+enter": "Shift+Enter",
}
PANEL_KEYS = {"p", "r", "t", "c", "a", "d", "y", "n"}


def arm(letter: str) -> str:
    """A *command arm* for a single-letter key: `KeyCode::Char('d') => …` (an alternation is fine).

    A bare mention is not proof: the quit chord writes `KeyCode::Char('c') | KeyCode::Char('d')` inside a
    `matches!`, which satisfied the first version of this audit for the approvals panel's `d` even after its
    arm was renamed (measured 2026-09-27 while running the controls).
    """
    return rf"Char\('{letter}'\)[^\n]{{0,60}}=>"


CHORD = re.compile(r"ctrl\+(shift\+)?([a-z]|←|→|left|right)", re.I)
ARROWS = {"←": "Left", "→": "Right", "left": "Left", "right": "Right"}


def chords_in(text: str) -> set:
    """Every chord a piece of text names, including ones this audit has no rule for.

    Generic on purpose: the first version only recognised the chords in its own table, so a hint advertising
    `Ctrl+Z` — a key nothing handles — was invisible to it (measured 2026-09-27 while running this audit's own
    controls). An unknown chord is now *named* by the audit, which either knows how to prove it or says it does
    not.
    """
    found = set()
    for shifted, letter in CHORD.findall(text):
        found.add(f"Ctrl+{ARROWS.get(letter.lower(), letter.upper())}")
    if re.search(r"shift\+enter", text, re.I):
        found.add("Shift+Enter")
    return found


def hint_keys(text: str) -> set:
    """The keys the hint strings advertise."""
    found = set()
    for literal in re.findall(r'"([^"\\]*(?:\\.[^"\\]*)*)"', text):
        lowered = literal.lower()
        if "·" not in literal and "confirm" not in lowered and "cancel" not in lowered:
            continue          # a hint string reads "A · B · C" (or the confirmation line)
        for word, key in HINT_WORDS.items():
            if word.startswith("ctrl") or word.startswith("shift"):
                continue                      # chords come from `chords_in`, which is generic
            if re.search(rf"(?<![a-z+]){re.escape(word)}(?![a-z])", lowered):
                found.add(key)
        found |= chords_in(literal)
        for function in re.findall(r"\bF\d{1,2}\b", literal):
            found.add(function)          # README: "deliberately no function keys" — one in a hint is drift
        for bare in re.findall(r"(?:^|[ ·])([a-z])(?: |·$)", literal):
            if bare in PANEL_KEYS:
                found.add(f"Key {bare}")
    return found


def documented_keys() -> dict:
    """`key -> [file:line]` for every key the documents name in their key paragraphs."""
    found: dict[str, list[str]] = {}
    for path in DOCS:
        for number, line in enumerate(path.read_text().splitlines(), 1):
            lowered = line.lower()
            if not any(word in lowered for word in ("ctrl+", "`enter`", "`tab`", "`esc`", "tui keys")):
                continue
            for word, key in HINT_WORDS.items():
                if word.startswith("ctrl") or word.startswith("shift"):
                    continue                  # chords come from `chords_in`, which is generic
                if re.search(rf"(?<![a-z+]){re.escape(word)}(?![a-z])", lowered):
                    found.setdefault(key, []).append(f"{path.name}:{number}")
            for key in chords_in(line):
                found.setdefault(key, []).append(f"{path.name}:{number}")
            for function in re.findall(r"\bF\d{1,2}\b", line):
                found.setdefault(function, []).append(f"{path.name}:{number}")
            for bare in re.findall(r"`([a-z])`", line):
                if bare in PANEL_KEYS:
                    found.setdefault(f"Key {bare}", []).append(f"{path.name}:{number}")
    return found


def main() -> int:
    source = APP.read_text()
    findings = []
    advertised = hint_keys(source)
    for key in sorted(advertised - {"Key p", "Key r", "Key t", "Key c", "Key a", "Key d", "Key y", "Key n"}):
        if key not in HANDLED:
            findings.append(f"the hints advertise {key!r} and this audit does not know how to prove it")
        elif not re.search(HANDLED[key], source):
            findings.append(f"the hints advertise {key!r} and {APP.name} has no handler for it")
    for bare in sorted(key for key in advertised if key.startswith("Key ")):
        letter = bare[-1]
        if not re.search(arm(letter), source):
            findings.append(f"the hints advertise the {letter!r} key and {APP.name} has no handler for it")
    for key, where in sorted(documented_keys().items()):
        pattern = HANDLED.get(key) if not key.startswith("Key ") else arm(key[-1])
        if pattern is None:
            findings.append(f"{where[0]} documents {key!r} and this audit does not know how to prove it")
        elif not re.search(pattern, source):
            findings.append(f"{where[0]} documents {key!r} and {APP.name} has no handler for it")
    for finding in findings:
        print(finding)
    print(f"{len(advertised)} advertised and {len(documented_keys())} documented key(s) checked against "
          f"{APP.name}: {len(findings)} unexplained")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

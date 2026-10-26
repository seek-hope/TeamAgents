#!/usr/bin/env python3
"""Keep language-independent parts of the two READMEs aligned (D-134).

The Chinese README is a translation, not an independent command reference. Compare the heading-level
skeleton, in-repository links, TeamAgents verbs and flags, Ctrl chords and supported slash commands.

    python3 review/readme_zh.py
    python3 review/readme_zh.py --self-check

This checks structure and interface vocabulary, not translation quality or whether a shared link is current.
"""
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
ENGLISH = REPO / "README.md"
CHINESE = REPO / "README.zh-CN.md"

HEADING = re.compile(r"^(#+)\s")
LINK = re.compile(r"\]\(([^)\s]+)\)")
VERB = re.compile(r"\bteamagents ([a-z][a-z-]*)")
FLAG = re.compile(r"(?<![\w-])--[a-z][a-z-]*")
CHORD = re.compile(r"Ctrl\+(?:[A-Za-z]+|←|→)")
LOCAL_COMMAND = re.compile(r"(?<![\w/])/(?:help|sessions|interrupt|quit|model|attach|detach|queue|edit)\b")

# Each README links to the other translation.
LINK_EXCEPTIONS = {"README.md", "README.zh-CN.md"}


def skeleton(text: str) -> list:
    return [match.group(1) for line in text.splitlines() if (match := HEADING.match(line))]


def links(text: str) -> list:
    return sorted(target for target in LINK.findall(text)
                  if not target.startswith("http") and target not in LINK_EXCEPTIONS)


def surface(text: str) -> tuple:
    return (sorted(set(VERB.findall(text))), sorted(set(FLAG.findall(text))),
            sorted(set(CHORD.findall(text))), sorted(set(LOCAL_COMMAND.findall(text))))


def compare(english: str, chinese: str) -> tuple[list[str], tuple[int, ...]]:
    findings = []
    en_skeleton, zh_skeleton = skeleton(english), skeleton(chinese)
    if en_skeleton != zh_skeleton:
        findings.append(
            f"the heading skeleton differs: README.md has {len(en_skeleton)} headings "
            f"({''.join(en_skeleton)}), README.zh-CN.md has {len(zh_skeleton)} ({''.join(zh_skeleton)})"
        )

    en_links, zh_links = links(english), links(chinese)
    for missing in sorted(set(en_links) - set(zh_links)):
        findings.append(f"README.zh-CN.md does not link {missing}, which README.md does")
    for extra in sorted(set(zh_links) - set(en_links)):
        findings.append(f"README.zh-CN.md links {extra}, which README.md does not")

    en_surface, zh_surface = surface(english), surface(chinese)
    labels = ("teamagents verb", "flag", "Ctrl chord", "slash command")
    for label, en_items, zh_items in zip(labels, en_surface, zh_surface):
        for missing in sorted(set(en_items) - set(zh_items)):
            findings.append(f"README.zh-CN.md is missing {label} {missing}, which README.md shows")
        for extra in sorted(set(zh_items) - set(en_items)):
            findings.append(f"README.zh-CN.md shows {label} {extra}, which README.md does not")

    counts = (len(en_skeleton), len(en_links), *(len(items) for items in en_surface))
    return findings, counts


def main() -> int:
    if "--self-check" in sys.argv[1:]:
        findings, _ = compare("# Test\nCtrl+Q /quit /model\n", "# Test\n")
        for token in ("Ctrl+Q", "/quit", "/model"):
            assert any(token in finding for finding in findings), (token, findings)
        assert not compare("# Test\nCtrl+Q /quit\n", "# Test\nCtrl+Q /quit\n")[0]
        print("readme_zh self-check: heading/link/interface comparison and missing-control detection passed")
        return 0

    findings, counts = compare(ENGLISH.read_text(), CHINESE.read_text())
    headings, link_count, verbs, flags, chords, commands = counts
    print(f"README.md ↔ README.zh-CN.md: {headings} headings, {link_count} in-repo links, "
          f"{verbs} verbs / {flags} flags, {chords} Ctrl chords and {commands} slash commands on each side")
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

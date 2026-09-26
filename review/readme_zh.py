#!/usr/bin/env python3
"""The Chinese README is a mirror of the English one, and nothing said so (D-134)

`README.zh-CN.md` is the repository's single non-English document (AGENTS.md), so it is a *translation* of
`README.md` and every part of it that does not need translating — the section skeleton, the link targets, the
CLI surface it shows — has to stay equal to the English file. Nothing checked that, and two things drifted:

* the documentation table lost a row (`docs/PRODUCT-COMPARISON.md` was added to the English table by
  `35dc328` and never to the Chinese one), and
* the file carried **two sections under the same heading** (the quick-start heading, used twice), the earlier
  one a shortened copy of the later one.

Both are invisible to `citations.py` (the paths it lost still resolve) and to `language-check` (the Chinese
file is the documented exception). This script asserts the three invariants that are language-independent:

1. **the heading skeleton** — the sequence of heading levels must be identical, so a section added, removed or
   re-levelled on one side is a finding (the section *text* is translated, so it is not compared);
2. **the link targets** — every in-repository link must appear on both sides;
3. **the CLI surface** — the set of `teamagents <verb>` invocations and `--flag` tokens must be equal, because
   neither commands nor flags are translated.

    python3 review/readme_zh.py

Ceiling: this compares structure and surface, not meaning. A translated paragraph that drifts from its English
original while keeping the same section and links is not caught; neither is a link that is correct in both
files but stale in both.
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

# The one intentional asymmetry: each README links to the other ("read this in Chinese/English").
LINK_EXCEPTIONS = {"README.md", "README.zh-CN.md"}


def skeleton(text: str) -> list:
    return [match.group(1) for line in text.splitlines() if (match := HEADING.match(line))]


def links(text: str) -> list:
    return sorted(target for target in LINK.findall(text)
                  if not target.startswith("http") and target not in LINK_EXCEPTIONS)


def surface(text: str) -> tuple:
    return sorted(set(VERB.findall(text))), sorted(set(FLAG.findall(text)))


def main() -> int:
    english, chinese = ENGLISH.read_text(), CHINESE.read_text()
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

    en_verbs, en_flags = surface(english)
    zh_verbs, zh_flags = surface(chinese)
    for missing in sorted(set(en_verbs) - set(zh_verbs)):
        findings.append(f"README.zh-CN.md never shows `teamagents {missing}`, which README.md does")
    for missing in sorted(set(en_flags) - set(zh_flags)):
        findings.append(f"README.zh-CN.md never shows {missing}, which README.md does")
    for extra in sorted(set(zh_verbs) - set(en_verbs)):
        findings.append(f"README.zh-CN.md shows `teamagents {extra}`, which README.md does not")
    for extra in sorted(set(zh_flags) - set(en_flags)):
        findings.append(f"README.zh-CN.md shows {extra}, which README.md does not")

    print(f"README.md ↔ README.zh-CN.md: {len(en_skeleton)} headings, {len(en_links)} in-repo links and "
          f"{len(en_verbs)} verbs / {len(en_flags)} flags on each side")
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

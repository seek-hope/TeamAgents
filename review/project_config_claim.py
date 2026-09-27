#!/usr/bin/env python3
"""One fact, checked against every document that states it: is the project config read? (D-133)

The product reads the **user** config only. `config::load_user_config_for`, the loader that merges a
repository-local `<cwd>/.teamagents/config.toml` under the trust rules, is implemented and unit-tested, but
nothing outside its own tests calls it — the daemon, TUI and `exec` all go through `load_user_config` with
`user_config_path()`. Three documents said the merge was live anyway (the config reference, the MCP bullet of
the user guide and one acceptance row), and they drifted for the honest reason that the loader *exists*: a
reader who sees the function, its tests and D-74's design note has every reason to believe it runs.

So the fact is computed here from the code — does `load_user_config_for` have a production caller (a mention
in `core|engine|tui/src` outside a `#[cfg(test)] mod`)? — and the documents have to agree with it:

* while it has none, each negative sentence below must be **present**, and each wording that claims the merge
  is live must be **absent**;
* wiring the loader flips both halves, so the script fails until the sentences are rewritten and this list is
  updated with them. That is the point: the claim is one fact with several statements, not several opinions.

    python3 review/project_config_claim.py

**D-236 added the sibling fact in the same files.** The *user* config's path is stated in five places — the
README, the user guide, the config reference, the install guide and `install.sh`, which **writes** the very file
the product reads — and the code builds it as `xdg_config_home().join(APP).join("config.toml")`. The audit now
derives `<APP>/config.toml` from that function (and reports if its shape changes, so the rule cannot quietly stop
reading), requires every one of those five statements to name it, and requires the line that names it to say which
directory it sits in (`XDG_CONFIG_HOME` or the `~/.config` default). A rename would otherwise leave the installer
writing a config nothing reads. Controls: `--doc` on a README copy with the path renamed reports the missing path,
and one with the directory half dropped reports that half.

Ceiling: the negative half is a fixed sentence list, and the positive half is a blacklist of the three
wordings that were wrong, so a *new* wording that claims the merge is live without matching one of them is
not caught — review has to keep this list in step with the prose.
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
CODE_ROOTS = ["core/src", "engine/src", "tui/src"]
LOADER = "load_user_config_for"
CFG_TEST = "#[cfg(test)]"
TEST_MOD = re.compile(r"^mod \w+")
CLOSE = re.compile(r"^}")

# (document, sentence that must be present while nothing reads the project config, where it lives)
UNWIRED_CLAIMS = [
    ("docs/CONFIG.md", "no entry point calls it yet", "the config reference's trust rule"),
    ("docs/USER-GUIDE.md", "not read by the current entry points", "the user guide's §2"),
    ("docs/ACCEPTANCE.md", "The project config is not read.", "the known-gap entry"),
    ("docs/PRODUCT-COMPARISON.md", "not read yet", "the comparison's config row"),
]

# Wording that asserts the merge is live. All three were in the tree before D-133 and are false while nothing
# calls the loader; they must be gone, and this is what notices if one comes back.
WIRED_ONLY_PHRASES = [
    ("docs/CONFIG.md", "project file *is* read"),
    ("docs/USER-GUIDE.md", "a cloned project's tools load only with"),
    ("docs/ACCEPTANCE.md", "or a trusted project config"),
]

# (D-236) The *user* config's path is the sibling fact, stated in the same files and compared by nothing: the
# code builds `xdg_config_home().join(APP).join("config.toml")`, and five places say where that is — two
# documents (twice each in the guides), the generated reference and the installer, which *writes* the very file
# the product reads. A rename of the directory or the file would leave the installer writing a config nothing
# reads, which is the sort of disagreement no gate looked at (measured 2026-09-27).
UPATH_SOURCE = REPO / "engine/src/config.rs"
UPATH_FN = re.compile(r"pub fn user_config_path\(\)[^{]*\{(.*?)\n\}", re.S)
UPATH_APP = re.compile(r'pub const APP: &str = "([^"]+)"')
UPATH_FILES = [
    ("README.md", "the README's install section"),
    ("docs/USER-GUIDE.md", "the user guide's configuration section"),
    ("docs/CONFIG.md", "the config reference's first line"),
    ("docs/INSTALL.md", "the install guide's first-configuration section"),
    ("install.sh", "the installer, which writes the default config"),
]


def production_calls() -> int:
    """Call sites of the loader in the product's own code, its own definition subtracted.

    A *call*, not a mention (D-130): an import or a doc comment referring to the loader does not read any
    config, so only `load_user_config_for(` sites outside a test module count, and the `pub fn` definition is
    not one of them.
    """
    total = 0
    for root in CODE_ROOTS:
        for path in (REPO / root).rglob("*.rs"):
            lines = path.read_text().splitlines()
            spans = []
            for i in range(len(lines) - 1):
                if lines[i].strip() == CFG_TEST and TEST_MOD.match(lines[i + 1] or ""):
                    j = i + 1
                    while j < len(lines) and not CLOSE.match(lines[j]):
                        j += 1
                    spans.append((i, j))
            for number, line in enumerate(lines, 1):
                if any(a < number <= b for a, b in spans):
                    continue
                if line.lstrip().startswith(("///", "//!", "//")):
                    continue
                if line.strip().startswith(("pub fn " + LOADER, "fn " + LOADER)):
                    continue
                total += len(re.findall(r"\b" + LOADER + r"\s*\(", line))
    return total


def normalize(text: str) -> str:
    """Markdown emphasis and line wrapping must not hide a claim: compare on words."""
    return re.sub(r"\s+", " ", text.replace("*", " ")).strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--doc", action="append", default=[], metavar="PATH=FILE",
                        help="read one of the documents from FILE instead (a copy is the control)")
    args = parser.parse_args()
    overrides = dict(item.split("=", 1) for item in args.doc)
    calls = production_calls()
    wired = calls > 0
    state = "is" if wired else "is not"
    print(f"the project-config loader ({LOADER}) {state} called by the product's own code "
          f"({calls} production call site(s))")
    findings = []
    for path, sentence, where in UNWIRED_CLAIMS:
        present = normalize(sentence) in normalize((REPO / path).read_text())
        if present == wired:  # must be present while unwired, absent once wired
            verb = "still present although the loader is now called" if wired else "missing"
            findings.append(f"{path}: the statement that the project config is not read is {verb} — {where}")
    for path, phrase in WIRED_ONLY_PHRASES:
        if not wired and normalize(phrase) in normalize((REPO / path).read_text()):
            findings.append(f"{path}: {phrase!r} claims the merge is live, but no entry point calls the loader")
    # D-236: the user config's path, derived from the code, against every place that states it
    source = UPATH_SOURCE.read_text()
    app, body = UPATH_APP.search(source), UPATH_FN.search(source)
    if app is None or body is None or 'join("config.toml")' not in body.group(1) or "join(APP)" not in body.group(1):
        findings.append("engine/src/config.rs: `user_config_path()` no longer builds `<xdg_config_home>/<APP>/"
                        "config.toml`, so this audit cannot derive the path it holds the documents to")
    else:
        tail = f"{app.group(1)}/config.toml"
        for path, where in UPATH_FILES:
            text = pathlib.Path(overrides[path]).read_text() if path in overrides else (REPO / path).read_text()
            stated = [line for line in text.split("\n") if tail in line]
            if not stated:
                findings.append(f"{path} ({where}) does not name the user config path {tail!r}, which is what the "
                                "code builds: the path is one fact with several statements")
            elif not any("XDG_CONFIG_HOME" in line or "~/.config" in line for line in stated):
                findings.append(f"{path}: the line naming {tail!r} does not say which directory it sits in — "
                                "`XDG_CONFIG_HOME` (or the `~/.config` default) is the other half of the path")
    for finding in findings:
        print("FAIL:", finding)
    if wired and not findings:
        print("  next: the loader is wired now, so the four negative sentences and this script's list must be "
              "rewritten together")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

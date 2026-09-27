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

* while it has none, each sentence of `UNWIRED_CLAIMS` must be **present** and each sentence of `WIRED_CLAIMS`
  **absent**;
* the lists flip with the fact, so the script fails until the sentences are rewritten together. That is the
  point: the claim is one fact with several statements, not several opinions.

    python3 review/project_config_claim.py

**D-244 flipped them**, and the flip is what the design was for: the loader became the product's path (the
daemon, `doctor` and the client all load through it) under one gate — `[permissions] trust_project = true` in the
user's own config, with `[permissions]` itself, `hooks`, `checks`, `retention` and `limits` never coming from a
repository. The four negative sentences are now this audit's blacklist (`WIRED_ONLY_PHRASES` was folded into
`WIRED_CLAIMS`: each document must state the live behaviour in words the list carries), and the wiring was
measured over a real session rather than inferred from the call graph —
`review/dogfood/project_config.py` reads the offered-surface witness twice, refused and trusted, and a third time
for a project that tries to grant itself the flag.

**D-236 added the sibling fact in the same files.** The *user* config's path is stated in five places — the
README, the user guide, the config reference, the install guide and `install.sh`, which **writes** the very file
the product reads — and the code builds it as `xdg_config_home().join(APP).join("config.toml")`. The audit now
derives `<APP>/config.toml` from that function (and reports if its shape changes, so the rule cannot quietly stop
reading), requires every one of those five statements to name it, and requires the line that names it to say which
directory it sits in (`XDG_CONFIG_HOME` or the `~/.config` default). A rename would otherwise leave the installer
writing a config nothing reads. Controls: `--doc` on a README copy with the path renamed reports the missing path,
and one with the directory half dropped reports that half.

**D-238 added the other path the product builds.** D-236's sibling one directory over: the *state root*. The code
builds `<xdg_state_home>/<APP>/v2` (`engine/src/lib.rs::v2_root()` joins a version segment onto `state_dir()`,
which joins `APP` onto `xdg_state_home()`), and four places tell a user where a session's state lives — the
README's `--state-root` row, the user guide's state-root section (twice), the Chinese README's translated row,
and the install guide's uninstall note, which names the app directory one level up. The audit derives the tail
and the directory half (`XDG_STATE_HOME`, or the `~/.local/state` default) from those functions, *reports* when
either stops reading, and requires each statement to name the tail with one of the two directory names on the
same line. **Controls**: `--doc README.md=<copy with `v2` renamed>` reports the missing path, one with
`XDG_STATE_HOME` swapped for `XDG_CONFIG_HOME` reports the half, `--doc docs/INSTALL.md=<copy without the state
line>` reports the app directory, and `--doc engine/src/lib.rs=<copy without the join>` reports that the path
cannot be derived.

Ceiling: the negative half is a fixed sentence list and the positive half a blacklist of the three wordings
that were wrong, so a *new* wording that claims the merge is live without matching one of them is not caught —
review has to keep that list in step with the prose. The two paths (D-236, D-238) compare the tail verbatim and
the directory half by the presence of one of two names on the line, so a document may state a path with a
different home variable and pass; and their file lists are fixed, so a *new* document that states either path is
not swept in (the Chinese README is in the state-root list because a path is not translated, and the acceptance
ledger's dated migration sentence is deliberately not held).
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

# (document, sentence that must be present while nothing reads the project config, where it lives). Since D-244
# this is the blacklist: the loader *is* wired, so each of these must be gone, and this list is what notices if one
# comes back.
UNWIRED_CLAIMS = [
    ("docs/CONFIG.md", "no entry point calls it yet", "the config reference's trust rule"),
    ("docs/USER-GUIDE.md", "not read by the current entry points", "the user guide's §2"),
    ("docs/ACCEPTANCE.md", "The project config is not read.", "the known-gap entry"),
    ("docs/PRODUCT-COMPARISON.md", "not read yet", "the comparison's config row"),
]

# (D-244) …and the statements that replace them, one per document, required while the merge is live. They say the
# same fact with the parts a reader needs: that it is read, the one gate that makes it contribute, and what a
# repository can never set. `WIRED_ONLY_PHRASES` (the three wordings that were wrong while nothing read the file)
# is folded into this list: the positive half is no longer a blacklist but a statement.
WIRED_CLAIMS = [
    ("docs/CONFIG.md", "is read by the product now", "the config reference's trust rule"),
    ("docs/USER-GUIDE.md", "it contributes nothing until you opt in", "the user guide's §2"),
    ("docs/ACCEPTANCE.md", "The project config is read, since D-244", "the known-gap entry"),
    ("docs/PRODUCT-COMPARISON.md", "reads repository-local configuration", "the comparison's config row"),
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

# (D-238) The other path this product builds, in the same spirit: the *state root* is `<state home>/<APP>/<v2>`,
# `engine/src/lib.rs::v2_root()` joining a version segment onto `state_dir()` (which joins `APP` onto
# `xdg_state_home()`), and the README, the user guide, the Chinese README and the install guide's uninstall note
# all tell a user where a session's state lives. Measured 2026-09-27: nothing compared them, so renaming the
# version segment (or the app directory) would leave every document pointing at a directory no run touches —
# `doctor`, `install.sh` and the docs would all agree on the wrong place.
SROOT_SOURCE = REPO / "engine/src/lib.rs"
SROOT_FN = re.compile(r"fn v2_root\(\)[^{]*\{(.*?)\n\}", re.S)
SROOT_JOIN = re.compile(r'join\("([^"]+)"\)')
SROOT_XDG = re.compile(r"pub fn xdg_state_home\(\)[^{]*\{(.*?)\n\}", re.S)
SROOT_ENV = re.compile(r'env_path\("([^"]+)"\)')
# (file, whether it states the whole root or only the app directory the state sits in, where it lives)
SROOT_FILES = [
    ("README.md", True, "the --state-root row"),
    ("docs/USER-GUIDE.md", True, "the state-root section"),
    ("README.zh-CN.md", True, "the translated --state-root row"),
    ("docs/INSTALL.md", False, "the uninstall note, which names the directory the state sits in"),
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


def read(path: str, overrides: dict[str, str]) -> str:
    """A tracked file, or the copy `--doc` names for it (the control)."""
    return pathlib.Path(overrides[path]).read_text() if path in overrides else (REPO / path).read_text()


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
    state = "read" if wired else "not read"
    for path, sentence, where in (WIRED_CLAIMS if wired else UNWIRED_CLAIMS):
        if normalize(sentence) not in normalize((REPO / path).read_text()):
            findings.append(f"{path}: the statement that the project config is {state} is missing — {where}")
    for path, sentence, where in (UNWIRED_CLAIMS if wired else WIRED_CLAIMS):
        if normalize(sentence) in normalize((REPO / path).read_text()):
            findings.append(f"{path}: {sentence!r} states the {'unwired' if wired else 'wired'} side of the fact, "
                            f"but the loader is {'called' if wired else 'not called'} — {where}")
    # D-236: the user config's path, derived from the code, against every place that states it
    source = read(str(UPATH_SOURCE.relative_to(REPO)), overrides)
    app, body = UPATH_APP.search(source), UPATH_FN.search(source)
    if app is None or body is None or 'join("config.toml")' not in body.group(1) or "join(APP)" not in body.group(1):
        findings.append("engine/src/config.rs: `user_config_path()` no longer builds `<xdg_config_home>/<APP>/"
                        "config.toml`, so this audit cannot derive the path it holds the documents to")
    else:
        tail = f"{app.group(1)}/config.toml"
        for path, where in UPATH_FILES:
            stated = [line for line in read(path, overrides).split("\n") if tail in line]
            if not stated:
                findings.append(f"{path} ({where}) does not name the user config path {tail!r}, which is what the "
                                "code builds: the path is one fact with several statements")
            elif not any("XDG_CONFIG_HOME" in line or "~/.config" in line for line in stated):
                findings.append(f"{path}: the line naming {tail!r} does not say which directory it sits in — "
                                "`XDG_CONFIG_HOME` (or the `~/.config` default) is the other half of the path")
    # D-238: the state root, derived from `v2_root()`/`state_dir()`/`xdg_state_home()`, against every statement
    v2fn = SROOT_FN.search(read(str(SROOT_SOURCE.relative_to(REPO)), overrides))
    xdg = SROOT_XDG.search(source)
    segment = SROOT_JOIN.search(v2fn.group(1)) if v2fn else None
    env = SROOT_ENV.search(xdg.group(1)) if xdg else None
    parts = SROOT_JOIN.findall(xdg.group(1)) if xdg else []
    default = "~/" + "/".join(parts) if parts else None
    if app is None or segment is None or env is None or default is None:
        findings.append("engine/src/lib.rs: `v2_root()` no longer reads as `state_dir().join(\"…\")` (or "
                        "`xdg_state_home()`/`APP` changed shape), so this audit cannot derive the state root's "
                        "path — teach it the new shape")
    else:
        for path, whole, where in SROOT_FILES:
            want = f"{app.group(1)}/{segment.group(1)}" if whole else f"/{app.group(1)}"
            label = f"state root {want!r}" if whole else f"state directory {want!r}"
            stated = [line for line in read(path, overrides).split("\n") if want in line]
            if not stated:
                findings.append(f"{path} ({where}) does not name the {label}, which is what `v2_root()` builds: "
                                "the path is one fact with several statements")
            elif not any(env.group(1) in line or default in line for line in stated):
                findings.append(f"{path}: the line naming {want!r} does not say where it sits — "
                                f"`{env.group(1)}` (or the `{default}` default) is the other half of the path")
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

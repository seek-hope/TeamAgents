#!/usr/bin/env python3
"""Config keys this build accepts but never serves (the detector behind D-75 and D-102).

`deny_unknown_fields` makes the loader *know* every config field, so a key that the rest of the program never
reads is accepted in silence: parsing, the path validator and a `doctor` row are enough to look served. That
class produced three findings in this repository (`[permissions] mode`, `[retention]`, `models.*.codex_profile`
— D-75) and one more after the sweep, `instruction_files` (D-102, which turned out to be *promised* by a doctor
row that said "reach every member's prompt"). This script is that audit as a command:

    python3 review/config_keys.py [--list-known]

It reads the config structs out of `core/src/models.rs`, counts each field's mentions in the crates, and reports
the fields whose only mentions are declaration, parsing/merging/validation (`config.rs`), the doctor surface
(`cli.rs`) and the tests. A name that appears in none of the plumbing beyond those files has no read site.

Limits, deliberately stated: the check is name-based (a field reached through a differently named accessor
would be missed), it cannot tell "read for a report" from "read to act" — that is what the allowlist is for —
and the argv parser counts as plumbing (a *flag* that is only parsed for a removed verb, D-53's
`--history-days`, is therefore reported here as unserved — which is what it is).
"""
import argparse
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
CONFIG_STRUCTS = ("UserConfig", "ModelProfile", "ToolBinding", "Retention", "Hooks", "GoalLimits", "CheckSpec")
# declaration, parsing/merging/validation, the doctor surface and the argv parser: none of them *use* a key
PLUMBING = {"core/src/models.rs", "engine/src/config.rs", "engine/src/cli.rs", "engine/src/main.rs"}

# Keys of the hand-read `[permissions]` table (`engine/src/config.rs::project_permissions` reads them itself and
# refuses every other name, D-161). They are config keys a user may write, so the examples and the documents may
# name them; when that table grows, this set grows with it.
HAND_READ = {"permissions", "mode", "trust_project_tools"}

# Keys the loader *refuses*: naming one in an example or a user-facing document sends a user into an error, so
# that is a finding even though the key is declared (D-75's `codex_profile` is the shape).
REFUSED_AT_LOAD = {
    "codex_profile": "D-75: an external Codex profile is refused at load; configure the member directly",
}

# field -> why it is allowed to have no read site
KNOWN_UNSERVED = {
    "codex_profile": "D-75: refused at load — an external Codex profile is not part of this release",
    "instruction_files": "D-102: declared, validated and reported as not applied; the gap is in ACCEPTANCE",
    "archived_days": "D-75: reported as not applied; retention is a known gap in ACCEPTANCE",
    "history_days": "D-75: reported as not applied; retention is a known gap in ACCEPTANCE",
    "retention": "D-75: the [retention] table is parsed and reported as not applied; retention is a known gap",
    "deadline_minutes": "applied by the loader: config.rs turns it into each goal's absolute deadline",
}


def uses_field(text: str, field: str) -> bool:
    """A *use* of the field (`.field` or `["field"]`), not a bare name.

    A bare-name search counts another struct's field declaration, a local variable or a doc comment as a reader;
    D-128 tightened this after `docs/CONFIG.md` showed `[models.<name>] provider` being "read" by the kernel's own
    unrelated `provider` fields. `retention` and `deadline_minutes` are the two keys that lose every reader under
    the stricter form — correctly: the loader is where they are applied (see KNOWN_UNSERVED).
    """
    return re.search(rf'\.{re.escape(field)}\b|\["{re.escape(field)}"\]', text) is not None


def config_fields() -> list[str]:
    fields, current = [], None
    for line in (REPO / "core/src/models.rs").read_text().splitlines():
        struct = re.match(r"pub struct (\w+)", line)
        if struct:
            current = struct.group(1)
            continue
        field = re.match(r"\s+pub (\w+):", line)
        if field and current in CONFIG_STRUCTS:
            fields.append(field.group(1))
    return fields


def named_keys(text: str) -> set[str]:
    """The config keys and section names a TOML-ish text shows, commented lines included.

    One name per line — the first key — because the value may be an inline table whose inner names
    (`generation_options = { reasoning_effort = "max" }`) are the provider's own options, not config keys. A
    section contributes its head (`[models.leader_main]` → `models`, `[[checks]]` → `checks`), since everything
    after the first dot is a user-chosen name.
    """
    found = set()
    for line in text.splitlines():
        section = re.match(r"\s*(?:#\s*)?\[\[?([a-z_][a-z0-9_.]*)\]?\]", line)
        if section:
            found.add(section.group(1).split(".")[0])
            continue
        key = re.match(r"\s*(?:#\s*)?([a-z_][a-z0-9_]*)\s*=", line)
        if key:
            found.add(key.group(1))
    return found


DOCS = ["README.md", "docs/USER-GUIDE.md", "docs/INSTALL.md"]
TOML_BLOCK = re.compile(r"```toml\n(.*?)```", re.S)


def keys_named_to_a_user() -> dict[str, set[str]]:
    """`{file: names}` for the surfaces that hand a user config text to copy: the shipped examples and the
    `toml` blocks of the three user-facing documents.

    A gate for the *other* direction from the rest of this audit: the fields above are what the code accepts, and
    this is what the documentation promises is accepted. A key that was renamed or removed reaches a user as a
    load error the first time they paste the snippet (D-193).
    """
    out = {}
    for path in sorted((REPO / "examples").glob("*.toml")):
        out[str(path.relative_to(REPO))] = named_keys(path.read_text())
    for name in DOCS:
        path = REPO / name
        names: set[str] = set()
        for block in TOML_BLOCK.findall(path.read_text()):
            names |= named_keys(block)
        if names:
            out[name] = names
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list-known", action="store_true", help="also list the allowed unserved fields")
    args = parser.parse_args()
    roots = ["core/src", "engine/src", "tui/src"]
    files = [p for root in roots for p in (REPO / root).rglob("*.rs")]
    findings, allowed = [], []
    for field in config_fields():
        reads = []
        for path in files:
            rel = str(path.relative_to(REPO))
            if rel in PLUMBING:
                continue
            if uses_field(path.read_text(errors="replace"), field):
                reads.append(rel)
        if reads:
            continue
        (allowed if field in KNOWN_UNSERVED else findings).append(field)
    print(f"{len(config_fields())} config fields scanned; {len(findings)} unserved, {len(allowed)} known")
    for field in findings:
        print(f"  unserved: {field} — accepts a value, no code outside the loader/doctor reads it")
    known = set(config_fields()) | HAND_READ
    named = keys_named_to_a_user()
    checked = 0
    for name, names in named.items():
        for key in sorted(names):
            checked += 1
            if key in REFUSED_AT_LOAD:
                print(f"  {name}: names `{key}`, which the loader refuses ({REFUSED_AT_LOAD[key]})")
                findings.append(key)
            elif key not in known:
                print(f"  {name}: names `{key}`, which is not a config key this build accepts — a user "
                      "copying it would meet a load error")
                findings.append(key)
    print(f"{checked} key mention(s) across {len(named)} user-facing surface(s) "
          f"({', '.join(sorted(named))})")
    if args.list_known:
        for field in allowed:
            print(f"  known: {field} ({KNOWN_UNSERVED[field]})")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

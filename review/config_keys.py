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

**D-237 added the shipped default profile's key.** That key is one fact in three layers: the code prefers it by
name (`default_model_key` asks the catalog for `contains_key("…")`), `init` writes the config that
`engine/src/config.rs` includes as `INITIAL_CONFIG` (`examples/config.minimal.toml`), and the documents tell a
user which key it is (the README's `--model` row, the user guide's configuration example, the install guide's
first-configuration section, and the full `examples/config.toml` the guides point at). Measured 2026-09-27:
nothing compared the three, so renaming the profile in the template would leave the code preferring a key that
ships nowhere and four statements naming a key the shipped config does not declare. The audit now reads the
preferred key out of `default_model_key`, parses the template's `[models.*]` keys, checks that `INITIAL_CONFIG`
really is included from that template (so the rule cannot quietly stop reading), and requires the key to be in
the template *and* in every statement. **Controls**: `--doc examples/config.minimal.toml=<copy without the key>`
reports the template half; `--doc README.md=<copy with the key renamed>` reports the statement half.

**D-240 added the fields the name-based search over-counts.** A same-named field of *another* struct reads as a
reader of the config key, which is how `[models.*].max_retries` stayed invisible: the key is accepted, validated
and never applied — `engine/src/cli.rs` passes the session's own `max_retries: 2`, and the mentions the search
found belong to `ReferenceConfig`/`DriverConfig`. Such a field is recorded in `MASKED` with the receivers those
foreign mentions use, reported in its own bucket (the first number of the summary line stays the unserved count),
and *checked*: a mention with any other receiver — the natural way to wire the key — is a finding, so the entry
cannot outlive the gap it records. `--doc PATH=FILE` reads a copy of any crate file, which is the control
(`--doc engine/src/v2/driver.rs=<copy where the driver takes the profile's value>` reports the new receiver). (D-247 wired that key — the supervisor reads the instance's own catalog
entry — and the check reported the new receiver exactly as advertised; `MASKED` is empty since.)

Ceiling: only the *key* is compared — the model name, the context window and the reasoning effort the same
sentences carry are prose; the statement list is fixed, so a *new* document that names the key is not swept in
(`docs/TOOLS.md` names it too and is left out deliberately: it is generated, and its writer's catalogue already
reads the tool that says it); the comparison is name-based, so a preference reached through a differently named
accessor in `default_model_key` would be missed; and the masked bucket is curated too — the search cannot tell a
same-named foreign field from a reader by itself, so a *new* key of that shape is reported as served until it is
recorded here.
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
HAND_READ = {"permissions", "mode", "trust_project", "sandbox", "sandbox_image"}

# Keys the loader *refuses*: naming one in an example or a user-facing document sends a user into an error, so
# that is a finding even though the key is declared (D-75's `codex_profile` is the shape).
REFUSED_AT_LOAD = {
    "codex_profile": "D-75: an external Codex profile is refused at load; configure the member directly",
}

# field -> why it is allowed to have no read site
KNOWN_UNSERVED = {
    "codex_profile": "D-75: refused at load — an external Codex profile is not part of this release",
    "archived_days": "D-245: accepted and not applied — one session per state root (A33) means no archived set",
    "deadline_minutes": "applied by the loader: config.rs turns it into each goal's absolute deadline",
}

# (D-240) A field the name-based search *over*-counts: every use it finds outside the plumbing is a same-named
# field of *another* struct, so the field looks served while nothing reads the config key. Each entry records the
# receivers those foreign uses have — a mention with any other receiver (the natural way to wire the key) is a
# finding, so the entry cannot outlive the gap it records.
#
# The table is **empty now**, and that is D-240's own outcome: `[models.*].max_retries` was its one entry (`the
# driver's budget is the session's own constant`), D-247 wired the key per instance — the supervisor reads
# `entry.max_retries` — and this check is what noticed: it reported the new receiver and said "either the key was
# wired (then it is no longer unserved and this entry must go) or a new same-named field appeared". The mechanism
# stays for the next field of that shape.
MASKED = {}


def unserved_reason(field: str):
    """The recorded reason a field has no read site of its own, or None when it is not one of the unread keys."""
    if field in KNOWN_UNSERVED:
        return KNOWN_UNSERVED[field]
    return MASKED[field][1] if field in MASKED else None


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

# (D-237) The shipped default profile's key, one fact in three layers (see the docstring).
# D-244 gave the function the session's cwd (the catalog is the merged one now), so the shape check takes any
# parameter list: what it holds is still one `contains_key("…")` test and one `return Some("…")`.
CODE_FN = re.compile(r"fn default_model_key\([^)]*\)[^{]*\{(.*?)\n\}", re.S)
CODE_PREF = re.compile(r'contains_key\("([^"]+)"\)')
CODE_RETURN = re.compile(r'return Some\("([^"]+)"\.to_string\(\)\)')
INITIAL_SOURCE = REPO / "engine/src/config.rs"
INITIAL_INCLUDE = 'include_str!("../../examples/config.minimal.toml")'
TEMPLATE = REPO / "examples/config.minimal.toml"
DEFAULT_STATEMENTS = [
    ("README.md", "the --model row"),
    ("docs/USER-GUIDE.md", "the configuration example"),
    ("docs/INSTALL.md", "the first-configuration section"),
    ("examples/config.toml", "the full example the guides point at"),
]


def shipped_profiles(path: pathlib.Path = TEMPLATE) -> set:
    """The `[models.<key>]` keys the template declares — the config `init` writes."""
    import tomllib
    with path.open("rb") as handle:
        data = tomllib.load(handle)
    return set(data.get("models", {}))


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
    parser.add_argument("--doc", action="append", default=[], metavar="PATH=FILE",
                        help="read one of the inputs (a statement, the template, the code's preference or the "
                             "include) from FILE instead (a copy is the control)")
    args = parser.parse_args()
    overrides = dict(item.split("=", 1) for item in args.doc)
    roots = ["core/src", "engine/src", "tui/src"]
    files = [p for root in roots for p in (REPO / root).rglob("*.rs")]
    findings, allowed, drift, masked, masked_drift = [], [], [], [], []

    def read(rel: str) -> str:
        """A tracked text input, or the copy `--doc` names for it (the control)."""
        return (pathlib.Path(overrides[rel]) if rel in overrides else (REPO / rel)).read_text(errors="replace")

    # D-237: the default profile's key, from the code's preference to the template and the documents
    code = read("engine/src/main.rs")
    initial = read(str(INITIAL_SOURCE.relative_to(REPO)))
    body = CODE_FN.search(code)
    preferred = CODE_PREF.search(body.group(1)) if body else None
    returned = CODE_RETURN.search(body.group(1)) if body else None
    template_rel = str(TEMPLATE.relative_to(REPO))
    template = pathlib.Path(overrides[template_rel]) if template_rel in overrides else TEMPLATE
    shipped = shipped_profiles(template)
    if INITIAL_INCLUDE not in initial:
        drift.append(f"{INITIAL_SOURCE.relative_to(REPO)}: `INITIAL_CONFIG` is no longer included from "
                     f"{template_rel}, so this audit is holding the documents to a config nothing ships")
    if body is None:
        drift.append("engine/src/main.rs: `default_model_key()` is gone, so the default profile's key cannot be "
                     "read — teach this audit where the preference moved")
    elif preferred is None or returned is None:
        drift.append("engine/src/main.rs: `default_model_key()` no longer reads as one `contains_key(\"…\")` test "
                     "and one `return Some(\"…\")`, so the preferred key cannot be read — teach this audit the new "
                     "shape")
    else:
        key = preferred.group(1)
        if returned.group(1) != key:
            drift.append(f"engine/src/main.rs: `default_model_key()` tests `contains_key({key!r})` but returns "
                         f"{returned.group(1)!r}: the preference and the returned key disagree")
        if key not in shipped:
            drift.append(f"the code prefers the model key {key!r} and {template_rel} declares {sorted(shipped)}: "
                         "`init` writes a config the client does not prefer")
        for path, where in DEFAULT_STATEMENTS:
            if key not in read(path):
                drift.append(f"{path} ({where}) does not name {key!r}, the key the code prefers and the template "
                             "ships: the default profile is one fact with several statements")
    for field in config_fields():
        reads, receivers = [], set()
        for path in files:
            rel = str(path.relative_to(REPO))
            if rel in PLUMBING:
                continue
            text = read(rel)
            if uses_field(text, field):
                reads.append(rel)
                receivers |= set(re.findall(rf"([A-Za-z_][\w.]*)\.{re.escape(field)}\b", text))
        if field in MASKED:
            # The uses above are another struct's same-named field; the key itself has no reader (D-240).
            expected, _ = MASKED[field]
            if receivers != expected:
                masked_drift.append(f"{field}: now mentioned through {sorted(receivers - expected) or receivers}; it "
                                    f"was recorded as read only through {sorted(expected)} — either the key was wired "
                                    "(then it is no longer unserved and this entry must go) or a new same-named field "
                                    "appeared (D-240)")
            masked.append(field)
            continue
        if reads:
            continue
        (allowed if field in KNOWN_UNSERVED else findings).append(field)
    print(f"{len(config_fields())} config fields scanned; {len(findings)} unserved, {len(allowed)} known")
    for field in findings:
        print(f"  unserved: {field} — accepts a value, no code outside the loader/doctor reads it")
    for field in masked:
        print(f"  masked: {field} — {MASKED[field][1]}")
    for finding in masked_drift:
        print(f"  masked: {finding}")
    for finding in drift:
        print(f"  default profile: {finding} (D-237)")
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
    return 1 if findings or drift or masked_drift else 0


if __name__ == "__main__":
    sys.exit(main())

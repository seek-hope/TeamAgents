#!/usr/bin/env python3
"""Generate `docs/CONFIG.md`: every key this build reads out of the config, and who reads it (D-128).

The config file is the first thing a user edits, and its reference was scattered: part of it in `USER-GUIDE.md`
§2, part of it in `INSTALL.md`, the rest discoverable only in `core/src/models.rs`. This document is generated
from the structs (`review/config_keys.py`'s extraction, reused so both audit and reference share one definition of
"served") and lists, per TOML table, each key with its type, its default, the files outside the loader that read
it, and the field's own doc comment as its meaning.

    python3 review/config_reference.py            # check (inside `make hygiene`)
    python3 review/config_reference.py --write    # regenerate the document

A field added, renamed or retyped, a new config struct, or a changed doc comment fails the check until the
document is regenerated. Keys the audit knows are accepted but unserved (D-75/D-102) appear with that reason
instead of a reader, so the document never implies a key does something it does not.

Limits: "read by" is a name search over the crates outside the loader/doctor/argv plumbing — a field reached
through a differently named accessor shows no reader (the same limit `config_keys.py` states), and only the TOML
tables this build ships are listed (a table that exists only in an older release is not).

**D-239 made the Absent column read `#[serde(default = "fn")]`.** Measured 2026-09-27: three keys override the
rustc default with a named function — `protocol` → `"openai"`, `timeout` → `120`, `max_retries` → `5` — and the
column printed the *type* default for them (`empty`, `0`, `0`), in the one column whose header says it is the
value the field holds when the key is missing. The audit now resolves each named default from its zero-argument
function's literal and **fails** when one cannot be read, so a new key of that shape cannot quietly ship the
wrong value; `--models PATH` reads a copy of the structs file (the control). Ceiling: only a function whose body
is a string or integer literal is resolved — anything computed (an env var, a join, a length) is reported rather
than guessed, and the column is still the *absent* value, not the effective one.
"""
import argparse
import importlib.util
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
DOC = REPO / "docs" / "CONFIG.md"
BEGIN = "<!-- generated: begin -->"
END = "<!-- generated: end -->"
MODELS = REPO / "core/src/models.rs"

spec = importlib.util.spec_from_file_location("config_keys", REPO / "review" / "config_keys.py")
config_keys = importlib.util.module_from_spec(spec)
spec.loader.exec_module(config_keys)

# struct -> the TOML table its fields live in
TABLES = {
    "UserConfig": "top level",
    "ModelProfile": "[models.<name>]",
    "ToolBinding": "[tools.<name>]",
    "Retention": "[retention]",
    "Hooks": "[hooks]",
    "GoalLimits": "[limits]",
    "CheckSpec": "[[checks]]",
}
# What the field holds when the key is absent. This is the *Rust* default, not necessarily the effective one: an
# `Option` key is usually read with `unwrap_or(…)`, so the value that applies is the reader's (for example
# `tool_timeout_s` is unset here and 120 seconds where it is used). The column is labelled `Absent` for that
# reason, and the introduction says so.
DEFAULTS = {"Option": "unset (the reader applies its own)", "Vec": "empty", "HashMap": "empty", "bool": "false",
            "String": "empty", "u64": "0", "u32": "0", "i64": "0", "f64": "0", "usize": "0"}

# (D-239) `#[serde(default = "fn")]` overrides the rustc default: when the key is missing, serde calls that
# function, so the Absent column must show its value and not the type's. The three keys that carry one
# (`protocol`, `timeout`, `max_retries`) printed `empty`/`0`/`0` while the loader filled `"openai"`/`120`/`5`.
SERDE_DEFAULT = re.compile(r'#\[serde\([^\]]*\bdefault = "([^"]+)"')
ZERO_ARG_FN = re.compile(r"fn (\w+)\(\) -> \w+ \{([^}]*)\}")


def named_defaults() -> dict:
    """`{fn name: the literal it returns}` for the zero-argument defaults in the structs' file.

    Only a plain literal is resolved (`"openai".into()`, `120`): a default that computes its value is *reported*
    by the caller instead of guessed, because a wrong number in this column is exactly what D-239 fixed.
    """
    out = {}
    for match in ZERO_ARG_FN.finditer(MODELS.read_text()):
        body = match.group(2).strip()
        text = re.fullmatch(r'"([^"]*)"\s*\.into\(\)', body)
        out[match.group(1)] = f'"{text.group(1)}"' if text else (body if re.fullmatch(r"-?\d+", body) else None)
    return out


def default_for(kind: str, named: str = None) -> str:
    if named is not None:
        return named
    base = re.match(r"([A-Za-z_][A-Za-z0-9_]*)", kind.strip().lstrip("&")).group(1)
    if base in DEFAULTS:
        return DEFAULTS[base]
    return f"the {base} default"


def fields():
    """[(struct, key, type, doc comment, named serde default fn or None)] in declaration order."""
    out, current, comment, attributes = [], None, [], []
    for line in MODELS.read_text().splitlines():
        struct = re.match(r"pub struct (\w+)", line)
        if struct:
            current, comment, attributes = struct.group(1), [], []
            continue
        trimmed = line.strip()
        if trimmed.startswith("///"):
            comment.append(trimmed[3:].strip())
            continue
        if trimmed.startswith("#["):
            attributes.append(trimmed)
            continue
        field = re.match(r"\s+pub (\w+):\s*(.+),", line)
        if field and current in TABLES:
            named = next((SERDE_DEFAULT.search(one).group(1) for one in attributes if SERDE_DEFAULT.search(one)),
                         None)
            out.append((current, field.group(1), field.group(2).strip(), " ".join(comment).strip(), named))
        comment, attributes = [], []
    return out


def readers(key: str) -> list:
    found = []
    for root in ["core/src", "engine/src", "tui/src"]:
        for path in sorted((REPO / root).rglob("*.rs")):
            rel = str(path.relative_to(REPO))
            if rel in config_keys.PLUMBING or rel == "core/src/models.rs":
                continue
            if config_keys.uses_field(path.read_text(errors="replace"), key):
                found.append(rel)
    return found


def section():
    """(the generated table, [findings for named serde defaults that cannot be read])."""
    lines = []
    current = None
    unresolved = []
    named = named_defaults()
    for struct, key, kind, comment, fn in fields():
        if struct != current:
            current = struct
            lines += ["", f"### `{TABLES[struct]}`", "",
                      "| Key | Type | Absent | Read by | Meaning |", "|---|---|---|---|---|"]
        if fn is not None and named.get(fn) is None:
            unresolved.append(f"{key}: `#[serde(default = \"{fn}\")]` is not a literal this audit can read — the "
                              "Absent column would state the type default while the loader fills the function's "
                              "value (D-239); teach it that function's shape")
        who = readers(key)
        known = config_keys.KNOWN_UNSERVED.get(key)
        if who:
            shown = ", ".join(f"`{one}`" for one in who[:2])
            read_by = shown + (f" … ({len(who)} files)" if len(who) > 2 else "")
        else:
            read_by = f"nothing: {known}" if known else "**nothing**"
        meaning = comment.replace("|", "\\|") or "—"
        absent = default_for(kind, named.get(fn))
        lines.append(f"| `{key}` | `{kind}` | {absent} | {read_by} | {meaning} |")
    return "\n".join(lines).lstrip("\n"), unresolved


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate the document")
    parser.add_argument("--models", metavar="PATH", help="read the config structs from a copy (the control)")
    args = parser.parse_args(argv[1:])
    global MODELS
    if args.models:
        MODELS = pathlib.Path(args.models)
    generated, unresolved = section()
    if args.write:
        if unresolved:
            print("refusing to regenerate docs/CONFIG.md:")
            for finding in unresolved:
                print(f"  {finding}")
            return 1
        text = DOC.read_text()
        start, end = text.index(BEGIN), text.index(END)
        DOC.write_text(text[:start] + BEGIN + "\n\n" + generated + "\n\n" + text[end:])
        print(f"docs/CONFIG.md regenerated: {len(fields())} keys")
        return 0
    text = DOC.read_text()
    findings = list(unresolved)
    if BEGIN not in text or END not in text:
        findings.append("docs/CONFIG.md has no generated markers")
    else:
        start, end = text.index(BEGIN) + len(BEGIN), text.index(END)
        if text[start:end].strip() != generated:
            findings.append("the generated section does not match the config structs: "
                            "run `python3 review/config_reference.py --write`")
    documented = set(re.findall(r"^\| `([a-zA-Z_]+)` \|", text, re.M))
    for _, key, _, _, _ in fields():
        if key not in documented:
            findings.append(f"{key}: a config key with no row in docs/CONFIG.md")
    for finding in findings:
        print(finding)
    if findings:
        return 1
    print(f"{len(fields())} config keys documented and in sync")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

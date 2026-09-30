#!/usr/bin/env python3
"""Generate `docs/TOOLS.md`: the tools a model may be offered, from the schemas that define them (D-127).

The tool surface is the capability surface of a coding agent, and this build assembles it from three schema
functions: `builtin_tool_schemas()` (every instance), the profile's tools — `basic_tool_schemas(web, skills)`,
which a session starts from as `reference::session_tool_schemas` (the web half only for the kinds the config
declares, §5.2/D-79) — and `collaboration_tool_schemas(actions)` (the team tools, per grant). None of that was documented anywhere: a user
writing instructions saw the tools only after a run, and the JSON schemas in the code were the only description.

    python3 review/tool_catalogue.py            # check (inside `make hygiene`)
    python3 review/tool_catalogue.py --write    # regenerate the document

The document is generated from those functions — name, model-facing description and parameters (with the required
ones marked) — so a renamed tool, a new parameter or a reworded description fails the check until `--write` runs.

Limits: only the static schemas are listed. MCP service tools are discovered from the server at session start
(`<service>_<tool>`, D-74) and the one `codemode` tool that reaches them (D-374) is built from the bound set at
boot, so neither can be known here; the *dispatch* side (which tool name reaches which executor)
is covered by the tools' own tests and `review/command_params.py`'s cousin checks, not by this document.
"""
import json
import pathlib
import re
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import verification_catalogue   # `number`: one implementation of spelling a count, shared by two audits

REPO = pathlib.Path(__file__).resolve().parents[1]
DOC = REPO / "docs" / "TOOLS.md"
# the documents that state how many tools the product offers, in words, and the phrase they state it with
COUNT_DOCS = ("docs/ACCEPTANCE.md",)
SPELLED_COUNT = re.compile(r"the ([a-z]+) tools")
BEGIN = "<!-- generated: begin -->"
END = "<!-- generated: end -->"
TYPES = REPO / "core" / "src" / "kernel" / "types.rs"
REFERENCE = REPO / "engine" / "src" / "reference.rs"


def brace_block(text, start):
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


def constants(text):
    return dict(re.findall(r'pub const ([A-Z_]+): &str = "([a-z_]+)";', text))


def parse_schema(block, consts):
    """A `json!({…})` block as a dict: constants substituted, comments dropped."""
    body = re.sub(r'^\s*//.*$', '', block, flags=re.M)
    for name, value in consts.items():
        body = re.sub(rf'\b{name}\b', f'"{value}"', body)
    body = re.sub(r',(\s*[}\]])', r'\1', body)  # trailing commas
    try:
        return json.loads(body)
    except json.JSONDecodeError:
        return None


def tools():
    """[(name, description, [(param, type, required, description)], source)] in offer order."""
    out = []
    types_text = TYPES.read_text(errors="replace")
    consts = constants(types_text)
    for function, label in (("fn builtin_tool_schemas()", "`kernel::builtin_tool_schemas()` (every instance)"),
                            ("fn collaboration_tool_schemas(", "`kernel::collaboration_tool_schemas(actions)` (per grant)")):
        start = types_text.index(function)
        end = types_text.index("\n}\n", start)
        body = types_text[start:end]
        for match in re.finditer(r'json!\(', body):
            block = brace_block(body, body.find("{", match.end()))
            schema = parse_schema(block, consts)
            if schema:
                out.append(entry(schema, label))
    reference = REFERENCE.read_text(errors="replace")
    start = reference.index("pub fn basic_tool_schemas(")
    end = reference.index("\n}\n", start)
    body = reference[start:end]
    for match in re.finditer(r'wrap\("([a-z_]+)",\s*"((?:[^"\\]|\\.)*)",\s*json!\(', body):
        block = brace_block(body, body.find("{", match.end()))
        parameters = parse_schema(block, {})
        if parameters:
            out.append((match.group(1), match.group(2).encode().decode("unicode_escape"),
                        [(name, spec.get("type", "object"), name in parameters.get("required", []), spec.get("description", ""))
                         for name, spec in parameters.get("properties", {}).items()],
                        "`reference::session_tool_schemas` (the profile's tools; the web half only for the "
                        "kinds the config declares — §5.2/D-79/D-168)"))
    return out


def entry(schema, label):
    function = schema["function"]
    parameters = function.get("parameters", {})
    required = parameters.get("required", [])
    return (function["name"], function["description"],
            [(name, spec.get("type", "object"), name in required, spec.get("description", ""))
             for name, spec in parameters.get("properties", {}).items()], label)


def section():
    lines = []
    for name, description, parameters, label in tools():
        lines.append(f"### `{name}`")
        lines.append("")
        lines.append(f"*Offered by {label}.*")
        lines.append("")
        lines.append(description)
        lines.append("")
        if parameters:
            lines.append("| Parameter | Type | Required | Meaning |")
            lines.append("|---|---|---|---|")
            for parameter, kind, required, meaning in parameters:
                lines.append(f"| `{parameter}` | `{kind}` | {'yes' if required else 'no'} | "
                             f"{meaning.replace('|', '\\|') or '—'} |")
        else:
            lines.append("_No parameters._")
        lines.append("")
    return "\n".join(lines).rstrip()


def main(argv):
    generated = section()
    if "--write" in argv:
        text = DOC.read_text()
        start, end = text.index(BEGIN), text.index(END)
        DOC.write_text(text[:start] + BEGIN + "\n\n" + generated + "\n\n" + text[end:])
        print(f"docs/TOOLS.md regenerated: {len(tools())} tools")
        return 0
    text = DOC.read_text()
    findings = []
    names = [tool[0] for tool in tools()]
    # (D-224) The documents that state how many tools there are, held to the schemas this catalogue derives —
    # `docs/ACCEPTANCE.md` says it twice ("the seventeen tools"), and nothing compared either with the list.
    # The count word is read with `verification_catalogue.number`, so the two audits spell numbers the same way.
    for doc in COUNT_DOCS:
        for word in sorted(set(SPELLED_COUNT.findall((REPO / doc).read_text(errors="replace")))):
            if verification_catalogue.number(word) < 0:
                continue        # "the team tools" is not a count; only a spelled number is compared
            if verification_catalogue.number(word) != len(names):
                findings.append(f"{doc} says there are {word} tools, the schemas define {len(names)}: the count "
                                "in the prose has to be one this catalogue recomputes (D-224)")
    if BEGIN not in text or END not in text:
        findings.append("docs/TOOLS.md has no generated markers")
    else:
        start, end = text.index(BEGIN) + len(BEGIN), text.index(END)
        if text[start:end].strip() != generated:
            findings.append("the generated section does not match the schemas: run `python3 review/tool_catalogue.py --write`")
    for finding in findings:
        print(finding)
    if findings:
        return 1
    print(f"{len(names)} tools documented and in sync: {', '.join(names)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

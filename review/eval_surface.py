#!/usr/bin/env python3
"""The model-visible surface the r2-p6 manifests claim to freeze, checked against the tree and the records (D-182).

`review/eval/r2-p6/` is pre-registered evidence. `manifest*.json` pin each task's prompt, fixture and checks
(`review/eval_manifests.py` checks those) and the analysis rule — but the *harness's* half of the surface was
only described as frozen: the file said "A/B keep identical instructions, tools, options and window (the
manifest freezes them)", while nothing pinned the instruction text, the offered tool names or the request
options. A one-character edit to group C's collaboration paragraph would have changed that group's treatment
silently, with the recorded batches and the report still reading as one experiment.

The manifests now carry a `surface` block (the two instruction templates, the offered tool names, the request
options) and every trial records what it ran under (D-182). This audit holds the three together:

* **source** — the text is decoded out of the harness (`engine/examples/eval_groups_abc.rs`, or
  `engine/examples/rebuild_p6.rs` before its rename) with Rust's own string-literal rules, and *every* version
  in that file's history must decode to the pins, so no recorded batch can have run a different text;
* **records** — every trial that reports its surface must report the pinned digest for its group's kind, the
  pinned tool names and the pinned options; a batch recorded before D-182 is reported as such;
* **evidence** — where a trial's committed `session.sqlite` still holds the profile the product persisted
  (a checkpoint does not always include it), the instruction text, the effort and the window are read back
  out of it and compared with the pins.

    python3 review/eval_surface.py                 # the audit (part of `make check` through `make hygiene`)
    python3 review/eval_surface.py --print         # the decoded templates and their digests
    python3 review/eval_surface.py --self-check    # the decoder's controls, on synthetic sources

Limits: the tool *names* are pinned against the trials' records rather than extracted from
`engine/src/reference.rs` where the schemas are built, so a schema change reaches this audit when the next
batch is recorded; the schemas themselves, like the rest of the product's prompt assembly, are pinned per
batch by the recorded commit. The instruction text is compared with `{workspace}` standing for the trial's
own workspace path, because the prompt a trial ran is the substituted one.
"""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
EVAL = REPO / "review/eval/r2-p6"
RUNS = EVAL / "runs"
HARNESS = "engine/examples/eval_groups_abc.rs"
REFERENCE = "engine/src/reference.rs"
MANIFESTS = ["manifest.json", "manifest-r2.json", "manifest-r3.json", "manifest-r4.json",
             "manifest-r5-recon.json", "manifest-r5-calib12.json", "manifest-r6.json"]  # D-257/D-259/D-261
# The anchors the two templates are found by. The text itself carries the platform line that dates it.
AGENT_ANCHOR = "You are a careful coding agent"
TEAM_ANCHOR = "You may build a team"
# D-254's directive arm: the same collaboration surface, a directive instruction shape instead of a
# permissive one. Its template is pinned like the others; revisions before D-254 simply do not carry it.
DIRECTIVE_ANCHOR = "Build a team for this task"
# The pre-registered treatment: the collaboration paragraph belongs to group C alone.
KIND_OF_GROUP = {"A": "agent", "B": "agent", "C": "team", "D": "team-directive"}
# Which pin key holds each kind's template and digest (D-254 added the third kind).
DIGEST_OF_KIND = {"agent": "agent_instructions_sha256", "team": "team_instructions_sha256",
                  "team-directive": "team_directive_instructions_sha256"}
TEMPLATE_OF_KIND = {"agent": "agent_instructions", "team": "team_instructions",
                    "team-directive": "team_directive_instructions"}


class SurfaceError(Exception):
    """The surface cannot be read: the harness moved, or the text was rewritten past recognition."""


def rust_literals(source: str):
    """The decoded value of every double-quoted Rust string literal in `source`, in file order.

    Rust's rules, not a regex: `\\n` and friends, the `\\` continuation that drops the newline *and the
    following indentation*, and comments and raw strings left alone. The decoder is checked against the
    harness itself in `--self-check`, and the digest it produces is compared with the digest the built harness
    prints (`--print-surface`), which is how a wrong model of the compiler's rules would show up.
    """
    out, index, length = [], 0, len(source)
    while index < length:
        if source.startswith("//", index):
            end = source.find("\n", index)
            index = length if end < 0 else end + 1
            continue
        if source.startswith("/*", index):
            end = source.find("*/", index)
            index = length if end < 0 else end + 2
            continue
        if source.startswith('r"', index) or re.match(r'r(#+)"', source[index:] or ""):
            raise SurfaceError("a raw string literal: the decoder does not model those, and the harness has none")
        if source[index] != '"':
            index += 1
            continue
        index += 1
        text = []
        while index < length:
            char = source[index]
            if char == '"':
                index += 1
                break
            if char != "\\":
                text.append(char)
                index += 1
                continue
            escape = source[index + 1] if index + 1 < length else ""
            simple = {"n": "\n", "t": "\t", "r": "\r", "0": "\0", '"': '"', "'": "'", "\\": "\\"}
            if escape in simple:
                text.append(simple[escape])
                index += 2
            elif escape == "\n":
                index += 2
                while index < length and source[index] in " \t":
                    index += 1
            elif escape == "u":
                match = re.match(r"\\u\{([0-9a-fA-F]+)\}", source[index:])
                if not match:
                    raise SurfaceError(f"an escape the decoder does not model: {source[index:index + 12]!r}")
                text.append(chr(int(match.group(1), 16)))
                index += match.end()
            else:
                raise SurfaceError(f"an escape the decoder does not model: {source[index:index + 4]!r}")
        out.append("".join(text))
    return out


def surface_of(source: str) -> tuple[str, str, str]:
    """`(agent template, team template, directive template)` decoded from one revision of the harness.

    The directive template (D-254) is `""` for every revision before that arm existed, which is what lets the
    history rule below accept the older revisions while a *new* batch that records the directive kind must match
    the current text.
    """
    literals = rust_literals(source)
    agent = "".join(literal for literal in literals if AGENT_ANCHOR in literal)
    if not agent:
        raise SurfaceError(f"no string literal holds {AGENT_ANCHOR!r}: the harness's instructions moved")
    team_literal = "".join(literal for literal in literals if TEAM_ANCHOR in literal)
    if not team_literal:
        raise SurfaceError(f"no string literal holds {TEAM_ANCHOR!r}: the collaboration paragraph moved")
    cut = team_literal.rfind("\n", 0, team_literal.index(TEAM_ANCHOR))
    if cut < 0:
        raise SurfaceError("the collaboration paragraph does not start on its own line")
    directive = ""
    directive_literal = "".join(literal for literal in literals if DIRECTIVE_ANCHOR in literal)
    if directive_literal:
        directive_cut = directive_literal.rfind("\n", 0, directive_literal.index(DIRECTIVE_ANCHOR))
        if directive_cut < 0:
            raise SurfaceError("the directive paragraph does not start on its own line")
        directive = agent + directive_literal[directive_cut:]
    return agent, agent + team_literal[cut:], directive


def tools_of(source: str) -> list[str]:
    """The tool names `basic_tool_schemas(web=false, skills=false)` offers, in order.

    The harness passes `web=false, skills=false`, so the eval's surface is the part of the function before the
    `if web` block. If that shape moves, this refuses instead of reporting a shorter list as the truth.
    """
    _, _, body = source.partition("pub fn basic_tool_schemas")
    if not body:
        raise SurfaceError(f"no `basic_tool_schemas` in {REFERENCE}: the audit's model of the tool surface moved")
    body = body.split("if web", 1)[0]
    names = re.findall(r'wrap\("([a-z_0-9]+)"', body)
    if not names:
        raise SurfaceError(f"`basic_tool_schemas` in {REFERENCE} wraps no tool by name: cannot read the surface")
    return names


def digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def text_pattern(template: str) -> re.Pattern:
    """`template` as a pattern over a *trial's* text, where the workspace path has been substituted."""
    before, after = template.split("{workspace}")
    return re.compile(re.escape(before) + "[^\n\"]*" + re.escape(after))


def git(*arguments: str) -> str:
    done = subprocess.run(["git", "-C", str(REPO), *arguments], capture_output=True, text=True)
    if done.returncode != 0:
        raise SurfaceError(f"git {' '.join(arguments)}: {done.stderr.strip()}")
    return done.stdout


def harness_at(revision: str, path: str = HARNESS) -> str:
    return git("show", f"{revision}:{path}")


def harness_history() -> list[tuple[str, str]]:
    """`(commit, path)` for every revision of the harness, newest first — renames followed."""
    listed = git("log", "--follow", "--format=%H", "--name-only", "--", HARNESS).split("\n")
    out, commit = [], None
    for line in listed:
        line = line.strip()
        if not line:
            continue
        if re.fullmatch(r"[0-9a-f]{40}", line):
            commit = line
        elif commit:
            out.append((commit, line))
    return out


def pins() -> dict:
    loaded = []
    for name in MANIFESTS:
        path = EVAL / name
        if not path.is_file():
            raise SurfaceError(f"{name} is missing")
        loaded.append((name, json.loads(path.read_text(encoding="utf-8"))))
    # D-254: a manifest may pin the *trial's own config* (the experiment's, not the machine's); where it does,
    # the file must exist and match. A machine's config that grew an MCP service changes the treatment silently —
    # measured by the first round-4 smoke trial, whose spawned member parked on it.
    for name, manifest in loaded:
        pinned = manifest.get("config")
        if not pinned:
            continue
        path = EVAL / pinned["path"]
        if not path.is_file():
            raise SurfaceError(f"{name} pins the trial config {pinned['path']}, which is missing")
        real = hashlib.sha256(path.read_bytes()).hexdigest()
        if real != pinned["sha256"]:
            raise SurfaceError(f"{pinned['path']} is {real[:12]}, {name} pins {pinned['sha256'][:12]}: the "
                               "experiment's config changed")
    blocks = [(name, manifest.get("surface")) for name, manifest in loaded]
    missing = [name for name, block in blocks if not block]
    if missing:
        raise SurfaceError(f"no `surface` block in {', '.join(missing)}: the harness's half was never pinned")
    # D-254 made the pin a *set* of instruction kinds rather than one block: every manifest must agree on every
    # key it states (a contradiction is still an error), and a manifest that adds a key — the directive arm's
    # template — extends the pin instead of invalidating the batches recorded before it.
    merged: dict = {}
    for name, block in blocks:
        for key, value in block.items():
            if key == "note":
                continue   # each manifest's prose about its own round; the *pins* are what must agree
            if key in merged and merged[key] != value:
                raise SurfaceError(f"{name}'s surface {key!r} differs from the pin the other manifests state; "
                                   "they describe one harness")
            merged[key] = value
    # The templates' *text* is decoded from the harness by the caller (D-182); the manifests state the digests.
    for key in ("agent_instructions_sha256", "team_instructions_sha256", "tools", "reasoning_effort"):
        if key not in merged:
            raise SurfaceError(f"the manifests state no {key!r}: the surface is not pinned")
    return merged


def profile_blob(data: bytes) -> list[dict]:
    """Every `{"model":…,"instructions":…,"options":…,"context_window":…}` the product persisted in `data`."""
    pattern = re.compile(
        rb'\{"model":"[^"]*","instructions":"(?:[^"\\]|\\.)*","options":\{[^}]*\},"context_window":\d+\}'
    )
    out = []
    for match in pattern.finditer(data):
        try:
            out.append(json.loads(match.group(0)))
        except json.JSONDecodeError:
            continue
    return out


def check_batch(batch: pathlib.Path, pin: dict, findings: list, notes: list) -> dict:
    """Check one recorded batch. Returns its coverage counts for the summary."""
    summary = {"trials": 0, "self_reported": 0, "from_state": 0, "profiles": 0, "state": 0, "legacy": 0}
    header_path = batch / "run-header.json"
    if not header_path.is_file():
        findings.append(f"{batch.name}: no run-header.json, so the batch's commit and analysis rule are unknown")
        return summary
    header = json.loads(header_path.read_text(encoding="utf-8"))
    commit = header.get("git")
    if not commit:
        findings.append(f"{batch.name}: the run header records no git commit: the product's half is unpinned")
    else:
        if subprocess.run(["git", "-C", str(REPO), "cat-file", "-e", f"{commit}^{{commit}}"],
                          capture_output=True).returncode != 0:
            findings.append(f"{batch.name}: recorded git commit {commit[:12]} is not in this history")
        elif subprocess.run(["git", "-C", str(REPO), "merge-base", "--is-ancestor", commit, "HEAD"],
                            capture_output=True).returncode != 0:
            findings.append(f"{batch.name}: recorded git commit {commit[:12]} is not an ancestor of HEAD")
    analyses = {json.loads((EVAL / name).read_text(encoding="utf-8"))["analysis"]["sha256"] for name in MANIFESTS}
    if header.get("manifest_analysis_sha256") not in analyses:
        findings.append(f"{batch.name}: the batch ran an analysis rule that no manifest pins")
    # a batch that names its manifest must also match that manifest's limits, field by field
    named = header.get("manifest")
    manifest = None
    if named:
        if named not in MANIFESTS:
            findings.append(f"{batch.name}: the run header names manifest {named!r}, which does not exist")
        else:
            manifest = json.loads((EVAL / named).read_text(encoding="utf-8"))
            if header.get("manifest_sha256") != hashlib.sha256((EVAL / named).read_bytes()).hexdigest():
                # not a finding: the manifest's `surface` pin is refreshed when the harness changes, so the
                # bytes move on; its tasks, limits and analysis rule are what this batch ran, and those are
                # checked below and by review/eval_manifests.py.
                notes.append(f"{batch.name}: {named} has been re-frozen since this batch (the pins below still "
                             "have to agree with it)")
    results = batch / "results.jsonl"
    if not results.is_file():
        findings.append(f"{batch.name}: no results.jsonl")
        return summary
    trials = [json.loads(line) for line in results.read_text(encoding="utf-8").splitlines() if line.strip()]
    summary["trials"] = len(trials)
    for trial in trials:
        tag = f"{trial['task']}.{trial['group']}.{trial['repeat']}"
        kind = KIND_OF_GROUP.get(trial["group"])
        if kind is None:
            findings.append(f"{batch.name}: {tag} runs a group no manifest defines ({trial['group']!r})")
            continue
        if manifest is not None and trial.get("wall_s") is not None:
            for field, limit in (("max_steps", "reference_max_steps"), ("timeout_s", "trial_timeout_s")):
                value = (trial.get("surface") or {}).get(field)
                if value is not None and value != manifest["limits"][limit]:
                    findings.append(f"{batch.name}: {tag} ran {field}={value}, manifest {named} says "
                                    f"{manifest['limits'][limit]}")
        surface = trial.get("surface")
        if not surface:
            summary["legacy"] += 1
        else:
            summary["self_reported"] += 1
            if surface.get("instructions_kind") != kind:
                findings.append(f"{batch.name}: {tag} reports instructions_kind="
                                f"{surface.get('instructions_kind')!r}, group {trial['group']} is {kind!r}")
            digest_key = DIGEST_OF_KIND.get(kind, "")
            expected = pin.get(digest_key)
            if expected is None:
                findings.append(f"{batch.name}: {tag} runs kind {kind!r}, which no manifest pins an instruction "
                                "template for")
                continue
            if surface.get("instructions_template_sha256") != expected:
                findings.append(f"{batch.name}: {tag} ran instructions whose digest is not the pinned {kind} one "
                                "— the treatment changed and the recorded batches are no longer one experiment")
            if surface.get("tools") != pin["tools"]:
                findings.append(f"{batch.name}: {tag} was offered {surface.get('tools')}, the pin says "
                                f"{pin['tools']}")
            if (surface.get("request_options") or {}).get("reasoning_effort") != pin["reasoning_effort"]:
                findings.append(f"{batch.name}: {tag} ran with request options "
                                f"{surface.get('request_options')}, the pin says {pin['reasoning_effort']!r}")
            if manifest is not None and (surface.get("request_options") or {}).get("reasoning_effort") != \
                    manifest["model"]["reasoning_effort"]:
                findings.append(f"{batch.name}: {tag} ran effort "
                                f"{(surface.get('request_options') or {}).get('reasoning_effort')!r}, manifest "
                                f"{named} pins {manifest['model']['reasoning_effort']!r}")
        path = batch / "state" / tag / "session.sqlite"
        if not path.is_file():
            continue
        summary["state"] += 1
        template = pin.get(TEMPLATE_OF_KIND.get(kind, ""), "")
        pattern = text_pattern(template)
        blobs = profile_blob(path.read_bytes())
        if not blobs:
            continue
        summary["profiles"] += 1
        matching = [blob for blob in blobs if pattern.search(blob.get("instructions", ""))]
        if not matching:
            findings.append(f"{batch.name}: {tag}'s committed state holds {len(blobs)} profile blob(s) whose "
                            "instructions are not the pinned text — the prompt recorded there is not the "
                            "pre-registered one")
            continue
        summary["from_state"] += 1
        window = matching[0].get("context_window")
        pinned_window = manifest["model"]["context_window"] if manifest else None
        if pinned_window and window != pinned_window:
            findings.append(f"{batch.name}: {tag} ran a {window}-token window; the manifest pins {pinned_window}")
    return summary


def self_check() -> int:
    """The decoder's controls: escapes, continuations, both source shapes, and a mutation that must be seen."""
    cases = [
        ('const A: &str = "one\\ntwo";', "one\ntwo"),
        ('const A: &str = "a\\\n     b";', "ab"),
        ('const A: &str = "quote \\" backslash \\\\ tab \\t";', 'quote " backslash \\ tab \t'),
        ('const A: &str = "uni \\u{48}i";', "uni Hi"),
        ('// "not a literal"\nconst A: &str = "real";', "real"),
        ('/* "neither" */ const A: &str = "real";', "real"),
    ]
    bad = []
    for source, expected in cases:
        got = rust_literals(source)
        if got != [expected]:
            bad.append(f"{source!r} decoded to {got!r}, expected [{expected!r}]")
    # both shapes of the harness must decode to the same surface: the inline `format!` and the constants
    old = 'fn agent_instructions(w: &str) -> String {\n    format!(\n        "' + AGENT_ANCHOR + ' at {workspace}.\\n\\\n' \
          '         - Use tools.",\n        workspace = w,\n    )\n}\n' \
          'fn team_instructions(w: &str) -> String {\n    format!(\n        "{}\\n\\\n         - ' + TEAM_ANCHOR + \
          ': spawn workers.",\n        agent_instructions(w)\n    )\n}\n'
    new = 'const AGENT_INSTRUCTIONS: &str = "' + AGENT_ANCHOR + ' at {workspace}.\\n\\\n     - Use tools.";\n' \
          'const TEAM_EXTRA: &str = "\\n\\\n     - ' + TEAM_ANCHOR + ': spawn workers.";\n'
    if surface_of(old) != surface_of(new):
        bad.append("the two harness shapes decode to different surfaces, so the validator and the harness disagree")
    if surface_of(new)[1].replace(surface_of(new)[0], "", 1) not in surface_of(new)[1]:
        bad.append("the team template does not contain the agent template")
    mutated = new.replace("Use tools", "Use  tools")
    if digest(surface_of(mutated)[0]) == digest(surface_of(new)[0]):
        bad.append("a one-character edit to the instructions did not change the digest")
    try:
        surface_of('const A: &str = "no anchor here";')
        bad.append("a source without the instructions was accepted instead of refused")
    except SurfaceError:
        pass
    for line in bad:
        print(f"FAIL: self-check: {line}")
    if bad:
        return 1
    print(f"self-check ok: {len(cases)} literal rules, both harness shapes, one mutation, one refusal")
    return 0


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--print", action="store_true", help="print the decoded templates and their digests")
    parser.add_argument("--self-check", action="store_true", help="check the decoder's own rules and exit")
    args = parser.parse_args(argv)
    if args.self_check:
        return self_check()
    findings, notes = [], []
    try:
        pin = pins()
        if args.print:
            agent, team, directive = surface_of((REPO / HARNESS).read_text(encoding="utf-8"))
            for name, text in (("agent", agent), ("team", team), ("team-directive", directive)):
                print(f"{name}: {digest(text) if text else '(absent)'}\n{text}\n")
            return 0
        # the source: the tree now, and every revision the file has had
        agent, team, directive = surface_of((REPO / HARNESS).read_text(encoding="utf-8"))
        pin = {**pin, "agent_instructions": agent, "team_instructions": team,
               "team_directive_instructions": directive}
        for name, text in (("agent_instructions_sha256", agent), ("team_instructions_sha256", team),
                           ("team_directive_instructions_sha256", directive)):
            if text and pin.get(name) != digest(text):
                findings.append(f"the harness's instructions no longer match the manifest pin {name} "
                                f"(pin {str(pin.get(name))[:12]}, tree {digest(text)[:12]}) — re-run the batches "
                                "or revert")
            elif not text and pin.get(name):
                findings.append(f"a manifest pins {name} and the harness no longer defines that arm: the "
                                "treatment was removed while its batches stand as evidence")
        if not team.startswith(agent):
            findings.append("group C's text is no longer group A/B's text plus a paragraph: the treatment changed")
        if directive and not directive.startswith(agent):
            findings.append("group D's text is no longer group A/B's text plus a paragraph: the treatment changed")
        tools = tools_of((REPO / REFERENCE).read_text(encoding="utf-8"))
        if pin["tools"] != tools:
            findings.append(f"the harness now offers {tools}; the manifests pin {pin['tools']} — the offered "
                            "surface changed, so the recorded batches no longer describe the same experiment")
        history = harness_history()
        for commit, path in history:
            try:
                revision_agent, revision_team, revision_directive = surface_of(harness_at(commit, path))
            except SurfaceError as error:
                findings.append(f"{commit[:12]}:{path}: {error}")
                continue
            if (digest(revision_agent), digest(revision_team)) != (digest(agent), digest(team)):
                findings.append(f"{commit[:12]}:{path}: this revision's instructions differ from the pin, so a "
                                "recorded batch may have run a different text")
            if revision_directive and digest(revision_directive) != digest(directive):
                findings.append(f"{commit[:12]}:{path}: this revision's directive arm differs from the pin, so a "
                                "recorded batch may have run a different text")
        if not history:
            findings.append(f"{HARNESS} has no history: the recorded batches' harness revision is unknown")
        batches = sorted(batch for batch in RUNS.iterdir() if batch.is_dir()) if RUNS.is_dir() else []
        if not batches:
            findings.append(f"{RUNS} holds no batch")
        coverage = {}
        for batch in batches:
            try:
                coverage[batch.name] = check_batch(batch, pin, findings, notes)
            except SurfaceError as error:
                findings.append(f"{batch.name}: {error}")
    except SurfaceError as error:
        findings.append(str(error))
        coverage = {}
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"{len(history)} harness revision(s) and {len(MANIFESTS)} manifest(s) carry one surface; "
          f"{len(coverage)} recorded batch(es):")
    for name, counts in coverage.items():
        print(f"  {name}: {counts['trials']} trial(s); {counts['self_reported']} self-report the pin"
              + (f", {counts['legacy']} predate it (the harness history covers the text they ran)"
                 if counts["legacy"] else "")
              + f"; {counts['profiles']}/{counts['trials']} committed checkpoints still hold the product's "
                f"persisted profile and {counts['from_state']} of those carry the pinned prompt")
    for note in notes:
        print(f"note: {note}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

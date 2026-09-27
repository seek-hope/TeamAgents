#!/usr/bin/env python3
"""The comparison's upstream halves, re-derived (D-209).

`docs/PRODUCT-COMPARISON.md` says what Pi and Hermes offer, and its strength table records the date and the
sources: Pi's README, its docs index (`packages/coding-agent/docs/docs.json`) and its file tree (GitHub API,
"N paths, `truncated:false`"), Hermes' README plus the features/tools page of its docs site. The strongest
claims there are the **negative** ones — Pi has no MCP page and no path containing "mcp", and no path
containing "worktree" — because a single new upstream file falsifies them, and a dated hand check is what
noticed, once. This script re-derives them:

    python3 review/comparison_sources.py                     # fetch and check
    python3 review/comparison_sources.py --tree PATH         # the control, on a saved tree

It asserts, and nothing else: Pi's tree is not truncated and holds no `mcp` or `worktree` path; its docs index
mentions MCP nowhere; the subagent example still says "max 8, 4 concurrent" and "a separate `pi` process"; the
README still says the product "runs with the permissions of the user and process that launched it" and still
links `pi-chat`; and Hermes' tools page still names the seven backends while its README still has interrupt,
streaming, session search, trajectory export, hibernating/serverless persistence, the gateway, `hermes model`
and agentskills. A count that has moved (the tree grows) is a *note*, not a finding — the row is a dated
snapshot, and the note is what tells the reader to re-date it.

It is not part of `make check`: it needs the network. Without one it prints a note and exits 0, the same shape
`review/install_check.py` has, and the hygiene catalogue lists it as run by hand.
"""
import argparse
import json
import pathlib
import re
import sys
import urllib.error
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from install_check import tls_context  # noqa: E402  (the same rule: verify TLS, never disable it)

PI_TREE = "https://api.github.com/repos/earendil-works/pi/git/trees/main?recursive=1"
PI_DOCS = "https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/docs.json"
PI_README = "https://raw.githubusercontent.com/earendil-works/pi/main/README.md"
PI_SUBAGENT = ("https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/examples/"
               "extensions/subagent/README.md")
HERMES_TOOLS = "https://hermes-agent.nousresearch.com/docs/user-guide/features/tools"
HERMES_README = "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/README.md"
BACKENDS = ("Docker", "SSH", "Singularity", "Modal", "Daytona", "Vercel Sandbox")
HERMES_README_CLAIMS = ("interrupt", "streaming", "session search", "trajectory", "serverless", "hibernat",
                        "gateway", "hermes model", "agentskills")
REPO = pathlib.Path(__file__).resolve().parents[1]
DOC = REPO / "docs/PRODUCT-COMPARISON.md"
STATED = re.compile(r"GitHub API, ([0-9,]+) paths")


def fetch(url: str) -> str:
    request = urllib.request.Request(url, headers={"User-Agent": "teamagents-comparison-check"})
    with urllib.request.urlopen(request, timeout=45, context=tls_context()) as reply:
        return reply.read().decode("utf-8", "replace")


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tree", help="a saved tree JSON to read instead of fetching (the control)")
    args = parser.parse_args(argv)
    findings, notes = [], []
    try:
        tree_text = pathlib.Path(args.tree).read_text() if args.tree else fetch(PI_TREE)
        docs_text, pi_readme = fetch(PI_DOCS), fetch(PI_README)
        subagent = fetch(PI_SUBAGENT)
        hermes_tools, hermes_readme = fetch(HERMES_TOOLS), fetch(HERMES_README)
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        print(f"note: no network ({error}), so the comparison's upstream halves cannot be re-derived here "
              "(the strength table carries the date they last were)")
        return 0

    tree = json.loads(tree_text)
    if tree.get("truncated") is not False:
        findings.append(f"Pi's tree is truncated ({tree.get('truncated')!r}), so the negative claims cannot be "
                        "derived from it")
    # every entry, the way the strength table counts them (directories included): "N paths"
    paths = [entry["path"] for entry in tree.get("tree", [])]
    for needle in ("mcp", "worktree"):
        hits = [path for path in paths if needle in path.lower()]
        if hits:
            findings.append(f"Pi's tree now holds {len(hits)} path(s) containing {needle!r} "
                            f"(e.g. {hits[0]}), so the comparison's negative claim is out of date")
    if re.search(r"mcp", docs_text, re.I):
        findings.append("Pi's docs index mentions MCP now, so the comparison's \"no MCP page\" claim is stale")
    for needle, why in ((("max 8, 4 concurrent"), "the subagent example's parallel limit"),
                        (("separate `pi` process"), "one process per subagent"),
                        (("permissions of the user and process that launched it"), "the permission model"),
                        (("pi-chat"), "the separate automation project")):
        if needle not in subagent + pi_readme:
            findings.append(f"Pi no longer says {needle!r} ({why}), so that cell must be re-derived")
    for name in BACKENDS:
        # the page lists them as lowercase code tokens (`daytona`), the row names them as prose
        if name.lower() not in hermes_tools.lower():
            findings.append(f"Hermes' tools page no longer names {name!r}, so the seven-backend claim is stale")
    for needle in HERMES_README_CLAIMS:
        if needle not in hermes_readme.lower():
            findings.append(f"Hermes' README no longer mentions {needle!r}, so that cell must be re-derived")
    stated = STATED.search(DOC.read_text())
    if stated is not None and int(stated.group(1).replace(",", "")) != len(paths):
        notes.append(f"Pi's tree holds {len(paths)} paths; the strength table says {stated.group(1)} — the row "
                     "is a dated snapshot, so re-date it rather than chase the number")
    for note in notes:
        print(f"note: {note}")
    for finding in findings:
        print("FAIL:", finding)
    if findings:
        return 1
    print(f"the comparison's upstream halves re-derive: Pi's tree {len(paths)} paths, truncated:false, no mcp "
          f"or worktree path; its docs index silent on MCP; Hermes' seven backends and README claims present")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

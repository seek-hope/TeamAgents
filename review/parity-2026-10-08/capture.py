#!/usr/bin/env python3
"""Capture public comparison sources at fixed versions; no model calls or credentials.

Run with an explicit output directory. Receipts contain selected source lines, the full
source hash, retrieval status and a replay command. Full downloads stay in /var/tmp.
"""
import argparse
import concurrent.futures
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess

CODEX = "9b738582b13c2cdbeff54af0afd04c50c3e7ba09"
BASELINE = "a76722da"
PI = Path("/home/rimuru/.pi/agent/install/releases/1.1.0/node_modules/@earendil-works")
CODEX_PATHS = [
    "README.md", "codex-rs/cli/src/main.rs", "codex-rs/app-server/README.md",
    "codex-rs/tui/src/bottom_pane/command_popup.rs", "codex-rs/tui/src/render/highlight.rs",
    "docs/agents_md.md", "docs/config.md", "docs/sandbox.md", "docs/exec.md", "docs/skills.md",
    "codex-rs/core/src/config/mod.rs", "codex-rs/core/src/tools/handlers/multi_agents.rs",
    "codex-rs/exec/src/cli.rs", "codex-rs/core/src/compact.rs",
    "codex-rs/core/src/tools/handlers/multi_agents/spawn.rs",
]
PI_PATHS = [
    "pi-coding-agent/README.md", "pi-coding-agent/docs/cli.md", "pi-coding-agent/docs/usage.md",
    "pi-coding-agent/docs/configuration.md", "pi-coding-agent/docs/security.md",
    "pi-coding-agent/docs/sessions.md", "pi-coding-agent/docs/compaction.md",
    "pi-coding-agent/docs/mcp.md", "pi-coding-agent/docs/extensions.md",
    "pi-coding-agent/docs/skills.md", "pi-coding-agent/docs/rpc.md",
    "pi-coding-agent/docs/packages.md", "pi-tui/dist/autocomplete.js",
    "pi-coding-agent/dist/modes/interactive/theme/theme.js",
]
LOCAL_PATHS = ["docs/DESIGN.md", "docs/USER-GUIDE.md", "docs/CAPABILITIES.md",
               "tui/src/v2app.rs", "tui/src/graphics.rs", "tui/src/markdown.rs"]
PATTERN = re.compile(r"autocomplete|complet|highlight|resume|fork|steer|follow.?up|sandbox|permission|trust|mcp|skill|extension|plugin|worktree|agent|subagent|json|rpc|stream|compact|token|budget|deadline|approval|interrupt|receipt|test|publish|release|durab|snapshot|checkpoint|config", re.I)


def capture(item, out, downloads):
    product, name = item
    revision = CODEX if product == "codex" else "1.1.0" if product == "pi" else BASELINE
    key = product + "-" + name.replace("/", "_").replace(".", "_")
    if product == "codex":
        url = f"https://raw.githubusercontent.com/openai/codex/{CODEX}/{name}"
        cmd = ["curl", "--fail", "--silent", "--show-error", "--max-time", "30", url]
        reproduce = "curl --fail --silent --show-error " + url
    elif product == "pi":
        url = (PI / name).as_uri()
        cmd = None
        reproduce = f"Read {PI / name}; pi --version must report 1.1.0. Compare the source_sha256 before using these excerpts."
    else:
        url = f"git:{BASELINE}:{name}"
        cmd = ["git", "show", f"{BASELINE}:{name}"]
        reproduce = f"git show {BASELINE}:{name}"
    if cmd:
        p = subprocess.run(cmd, capture_output=True, timeout=40)
        if p.returncode:
            return {"product": product, "url": url, "status": p.returncode, "error": p.stderr.decode(errors="replace")}
        data = p.stdout
    else:
        data = (PI / name).read_bytes()
    (downloads / key).write_bytes(data)
    lines = data.decode(errors="replace").splitlines()
    selected = [{"line": i, "text": line} for i, line in enumerate(lines, 1)
                if PATTERN.search(line) and len(line) < 1200][:100]
    receipt = out / "receipts" / (key + ".json")
    record = {"product": product, "revision_or_version": revision, "url": url,
              "retrieved_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "source_sha256": hashlib.sha256(data).hexdigest(), "source_bytes": len(data),
              "source_lines": len(lines), "evidence_class": "source or documentation, not executed product behavior",
              "reproduce": reproduce, "excerpts": selected}
    receipt.write_text(json.dumps(record, indent=2, ensure_ascii=True) + "\n")
    return {**{k: record[k] for k in ["product", "revision_or_version", "url", "retrieved_at", "source_sha256", "reproduce"]},
            "receipt": str(receipt), "receipt_sha256": hashlib.sha256(receipt.read_bytes()).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    (args.out / "receipts").mkdir(parents=True, exist_ok=True)
    downloads = Path("/var/tmp/teamagents-parity-sources")
    downloads.mkdir(exist_ok=True)
    items = [("codex", p) for p in CODEX_PATHS] + [("pi", p) for p in PI_PATHS] + [("teamagents", p) for p in LOCAL_PATHS]
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        results = list(pool.map(lambda item: capture(item, args.out, downloads), items))
    sources = [r for r in results if "error" not in r]
    failures = [r for r in results if "error" in r]
    (args.out / "manifest.json").write_text(json.dumps({"sources": sources, "retrieval_failures": failures}, indent=2) + "\n")
    print(json.dumps({"sources": len(sources), "failures": failures, "downloads": str(downloads)}, indent=2))


if __name__ == "__main__":
    main()

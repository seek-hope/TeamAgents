#!/usr/bin/env python3
"""Verify the D-450 captured provenance, run logs and paired fixture without network access."""
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verified(path, sha):
    resolved = (ROOT / path).resolve()
    assert resolved.is_relative_to(ROOT) and resolved.is_file(), path
    data = resolved.read_bytes()
    assert digest(data) == sha, f"receipt hash differs: {path}"
    return data


def main():
    manifest = json.loads((HERE / "manifest.json").read_text())
    assert {"codex", "pi", "teamagents"} <= {s["product"] for s in manifest["sources"]}
    for source in manifest["sources"]:
        receipt = json.loads(verified(source["receipt"], source["receipt_sha256"]))
        for key in ("product", "revision_or_version", "url", "retrieved_at", "source_sha256", "reproduce"):
            assert source[key] and receipt[key] == source[key], (source["receipt"], key)
        if source["product"] == "teamagents":
            data = subprocess.check_output(["git", "show", source["url"].removeprefix("git:")], cwd=ROOT)
            assert digest(data) == source["source_sha256"]
    runs = {}
    for run in manifest["runs"]:
        receipt = json.loads(verified(run["receipt"], run["receipt_sha256"]))
        assert receipt["exit_code"] == 0, run["name"]
        for stream in ("stdout", "stderr"):
            verified(receipt[stream], receipt[stream + "_sha256"])
        runs[run["name"]] = receipt
    assert {"check", "pty", "sessions", "reconnect", "input-latency", "formal-final", "formal-controls", "formal-kani"} <= runs.keys()
    before = runs["before-bounded"]
    after = runs["after-bounded"]
    probe = "tui/examples/parity_probe.rs"
    assert before["execution_source_sha256"][probe] == after["source_sha256"][probe] == digest((ROOT / probe).read_bytes())
    for path, sha in before["execution_source_sha256"].items():
        if path == probe:
            continue
        original = subprocess.check_output(["git", "show", f"a76722da:{path}"], cwd=ROOT)
        assert digest(original) == sha, path
    # Final production/tests match the full gate; the later measurement example is checked separately.
    for path, sha in runs["check"]["source_sha256"].items():
        if path.startswith(("core/", "engine/", "tui/")) and path != probe:
            assert digest((ROOT / path).read_bytes()) == sha, f"product changed after full check: {path}"
    for path, sha in runs["formal-final"]["source_sha256"].items():
        if path.startswith(("verification/tla/", "verification/kani/")):
            assert digest((ROOT / path).read_bytes()) == sha, path
    left = json.loads((ROOT / before["stdout"]).read_text())
    right = json.loads((ROOT / after["stdout"]).read_text())
    assert left["fixture"] == right["fixture"]
    assert left["media_missing"] == [16, 17, 18, 19]
    assert right["media_missing"] == [] and right["media_seen"] == list(range(20))
    assert len(right["visits"]) == 24 and all(v["preview"] and v["wait_ms"] < 2000 for v in right["visits"])
    assert left["tab_draft"] == "/hel" and right["tab_draft"] == "/help"
    assert left["tab_effect"] == right["tab_effect"] == "None"
    assert left["model_calls"] == right["model_calls"] == 0
    assert left["draft_preserved"] and right["draft_preserved"]
    print(f"{len(manifest['sources'])} sources, {len(runs)} run receipts, baseline source identity, final gate inputs and 24/24 bounded media visits verified")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Freeze (--write) or verify D-451 evidence offline. Never execute a recorded command."""
import argparse
import datetime
import hashlib
import gzip
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
RECEIPTS = HERE / "receipts"
REQUIRED = {
    "baseline-corrected", "check-glob-fixed", "pty-signal-fixed", "offline-glob-fixed", "nobwrap-glob-fixed",
    "broken-sandbox-glob-fixed", "formal-all-final", "formal-controls-final", "formal-kani-final",
    "installed-artifact-final", "workflows-signal-fixed", "terminal-features-search-key-fixed",
    "media-revisit-final", "input-latency-final", "export-real-signal-first",
    "display-all-cold-grammar-fixed", "display-input-visible-fixed", "queue-attachments-session-root",
}
COMMANDS = {
    "check-glob-fixed":["make","check"], "pty-signal-fixed":["make","pty"],
    "offline-glob-fixed":["make","probe-offline"], "nobwrap-glob-fixed":["make","check-nobwrap"],
    "broken-sandbox-glob-fixed":["make","check-broken-sandbox"],
    "formal-all-final":["make","verify-model-all"],
    "formal-controls-final":["make","verify-model-counterexamples"],
    "formal-kani-final":["make","verify-kani"],
    "installed-artifact-final":["python3","review/maturity-2026-10-11/install.py"],
    "workflows-signal-fixed":["python3","review/maturity-2026-10-11/workflows.py"],
}
SHA = re.compile(r"[0-9a-f]{64}\Z")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read(relative, expected=None):
    path = ROOT / relative
    compressed = not path.exists() and path.with_suffix(path.suffix + ".gz").is_file()
    stored = path.with_suffix(path.suffix + ".gz") if compressed else path
    assert not stored.is_symlink() and stored.resolve().is_relative_to(ROOT) and stored.is_file(), relative
    data = gzip.decompress(stored.read_bytes()) if compressed else stored.read_bytes()
    if expected is not None:
        assert SHA.fullmatch(expected) and digest(data) == expected, f"hash differs: {relative}"
    return data


def files():
    names = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard"],
                                    cwd=ROOT, text=True).splitlines()
    return sorted(set(name for name in names if (ROOT / name).is_file()))


def product(name):
    return name.startswith(("core/", "engine/", "tui/"))


def executable(name):
    return product(name) and ("/src/" in name or name.endswith(("Cargo.toml", "Cargo.lock", "build.rs")))


def inputs():
    return [name for name in files() if (product(name) or name.startswith(("docs/", "verification/"))
            or name.endswith(".py") or name in ("Makefile", "rust-toolchain.toml", "install.sh", "AGENTS.md",
                "README.md", "README.zh-CN.md", ".github/release-notes.md", "review/README.md",
                "review/maturity-2026-10-11/REPORT.md"))
            and not name.startswith(("review/eval/", "review/benchmark/"))]


def freeze():
    runs = []
    for path in sorted(RECEIPTS.glob("*.json")):
        row = json.loads(path.read_text())
        runs.append({"name":path.stem, "receipt":str(path.relative_to(ROOT)),
                     "receipt_sha256":digest(path.read_bytes()), "exit_code":row["exit_code"]})
    manifest = {
        "schema":1, "baseline":"ea43869d8b7058ccea63a1d96a21f60ab04e9013",
        "formal_material_commit":"e9a0aa84431f", "recorded_head":subprocess.check_output(
            ["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
        "frozen_at":datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "environment":{name:subprocess.check_output(argv,cwd=ROOT,text=True).strip() for name,argv in
            [("rustc",["rustc","-Vv"]),("cargo",["cargo","-V"]),("platform",["uname","-smr"]) ]},
        "source_sha256":{name:digest(read(name)) for name in inputs()}, "runs":runs,
        "required_passes":sorted(REQUIRED),
        "compressed_streams":{str(p.relative_to(ROOT)):digest(p.read_bytes()) for p in RECEIPTS.glob("*.gz")},
        "evidence_boundary":"Deterministic local runtime, loopback HTTP, actual subprocess and PTY, TestBackend and bounded formal models; no new real-model or graphical-terminal acceptance.",
        "historical_media":{
            "receipt":"review/parity-2026-10-08/receipts/after-bounded.json",
            "receipt_sha256":digest(read("review/parity-2026-10-08/receipts/after-bounded.json")),
            "meaning":"Historical accepted D-450 result, same twenty-media/24-visit fixture; timing is not a new paired experiment."},
    }
    (HERE / "manifest.json").write_text(json.dumps(manifest,indent=2)+"\n")


def verify():
    manifest = json.loads((HERE / "manifest.json").read_text())
    assert manifest["schema"] == 1
    assert set(manifest["required_passes"]) == REQUIRED
    assert set(manifest["source_sha256"]) == set(inputs()), "source inventory changed"
    for name, sha in manifest["source_sha256"].items():
        read(name,sha)
    runs = {}
    for entry in manifest["runs"]:
        name = entry["name"]
        assert name not in runs and entry["receipt"] == str((RECEIPTS / (name+".json")).relative_to(ROOT))
        row = json.loads(read(entry["receipt"],entry["receipt_sha256"]))
        assert row["exit_code"] == entry["exit_code"]
        assert row["command"] and all(isinstance(x,str) for x in row["command"])
        assert row["cwd"] and row["seconds"] >= 0 and re.fullmatch(r"[0-9a-f]{40}",row["head"])
        datetime.datetime.fromisoformat(row["started_at"])
        assert row["source_sha256"] and all(SHA.fullmatch(s) for s in row["source_sha256"].values())
        for stream in ["stdout","stderr"]:
            assert row[stream] == str((RECEIPTS / (name+"."+stream)).relative_to(ROOT))
            read(row[stream],row[stream+"_sha256"])
        runs[name] = row
    assert set(runs) == {p.stem for p in RECEIPTS.glob("*.json")}, "unindexed receipt"
    for name, sha in manifest["compressed_streams"].items():
        read(name,sha)
    assert {p.name.removesuffix(".gz") for p in RECEIPTS.iterdir()} == {n+"."+s for n in runs for s in ["json","stdout","stderr"]}
    for name in REQUIRED:
        assert name in runs and runs[name]["exit_code"] == 0, f"required passing gate absent: {name}"
    for name, command in COMMANDS.items():
        assert runs[name]["command"] == command, f"gate command changed: {name}"
    for name in ["check-glob-fixed","nobwrap-glob-fixed","broken-sandbox-glob-fixed"]:
        expected = {p:sha for p,sha in manifest["source_sha256"].items() if product(p)}
        assert all(runs[name]["source_sha256"].get(p)==sha for p,sha in expected.items()), f"product changed after {name}"
    for name in ["pty-signal-fixed","offline-glob-fixed","installed-artifact-final","workflows-signal-fixed"]:
        for p,sha in manifest["source_sha256"].items():
            if executable(p):
                assert runs[name]["source_sha256"].get(p)==sha, f"executable source changed after {name}: {p}"
    for name in ["formal-all-final","formal-controls-final","formal-kani-final"]:
        for p,sha in runs[name]["source_sha256"].items():
            if p.startswith(("verification/tla/","verification/kani/")):
                read(p,sha)
    for name in ["baseline", "display-all-first", "queue-attachments-daemon-fixed", "maturity-check-first"]:
        assert runs[name]["exit_code"] != 0, f"retained failure lost: {name}"
    current = json.loads(read(runs["media-revisit-final"]["stdout"]))
    hist = manifest["historical_media"]
    old = json.loads(read(hist["receipt"],hist["receipt_sha256"]))
    before = json.loads(read(old["stdout"],old["stdout_sha256"]))
    assert current["fixture"] == before["fixture"]
    for data in [current,before]:
        assert data["media_seen"] == list(range(20)) and data["media_missing"] == []
        assert len(data["visits"]) == 24 and all(v["preview"] and v["wait_ms"] < 2000 for v in data["visits"])
        assert data["draft_preserved"] and data["model_calls"] == 0
    assert current["rust_body_color_count"] >= 2 and current["tab_effect"] == "None"
    for scenario in ["read-modify-test","checks-repair","checks-exhaust","daemon-sigkill-recovery","queued-attachment-edit-cancel"]:
        for run in ["workflows-signal-fixed","installed-artifact-final"]:
            assert f'"scenario": "{scenario}"' in read(runs[run]["stdout"]).decode()
    print(f"PASS: {len(runs)} immutable receipts ({sum(r['exit_code'] != 0 for r in runs.values())} retained failures), "
          f"{len(manifest['source_sha256'])} current inputs, final product/formal source identity and historical/current 24/24 media visits")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write",action="store_true")
    args = parser.parse_args()
    if args.write:
        freeze()
    verify()


if __name__ == "__main__":
    main()

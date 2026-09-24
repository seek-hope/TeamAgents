#!/usr/bin/env python3
"""R28/§14 legacy TeamAgents data cleanup: enumerate, then delete only what the
inventory listed. Refuses symlinks, never touches the live v2 root, credentials,
evidence or other applications.

    python3 review/r28-legacy-cleanup.py              # inventory + receipt (no deletion)
    python3 review/r28-legacy-cleanup.py --apply      # delete exactly the listed entries

Retention rules (plan §14): keep credentials (`~/.config/teamagents/config.toml`,
`~/.codex`, `~/.agents/skills`), review/eval evidence, git history and any other
application's data.
"""
import argparse, json, os, pathlib, shutil, sys

STATE = pathlib.Path(os.environ.get("XDG_STATE_HOME", pathlib.Path.home() / ".local/state")) / "teamagents"
KEEP_UNDER_STATE = {"v2"}  # the live v2 session root


def size_of(path: pathlib.Path) -> int:
    if path.is_symlink():
        return 0
    if path.is_file():
        return path.stat().st_size
    total = 0
    for root, dirs, files in os.walk(path):
        for name in files:
            file = pathlib.Path(root) / name
            if not file.is_symlink():
                total += file.stat().st_size
    return total


_CORPUS: dict[str, str] | None = None


def evidence_corpus() -> dict[str, str]:
    """Read the repository's evidence text once; scratch areas (review/tmp) are
    excluded so the inventory cannot reference itself."""
    global _CORPUS
    if _CORPUS is not None:
        return _CORPUS
    corpus = {}
    for root in ("review", "docs"):
        for path in pathlib.Path(root).rglob("*"):
            if not path.is_file() or path.suffix in {".png", ".svg"}:
                continue
            if str(path).startswith("review/tmp"):
                continue
            try:
                corpus[str(path)] = path.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
    for name in ("AGENTS.md", "README.md"):
        path = pathlib.Path(name)
        if path.is_file():
            corpus[name] = path.read_text(encoding="utf-8", errors="ignore")
    _CORPUS = corpus
    return corpus


def referenced(name: str) -> str | None:
    """Cleanup discipline (§14): anything the repository cites as evidence stays,
    even when it lives in /tmp."""
    for path, text in evidence_corpus().items():
        if name in text:
            return path
    return None


def inventory() -> list[dict]:
    entries = []
    if STATE.is_dir():
        for child in sorted(STATE.iterdir()):
            if child.name in KEEP_UNDER_STATE:
                continue  # live v2 root: never a cleanup target
            entries.append({
                "path": str(child),
                "type": "symlink" if child.is_symlink() else ("dir" if child.is_dir() else "file"),
                "bytes": size_of(child),
                "owner": "legacy-v1-state" if child.name in {"sessions", "ui.json", "composer-history.json", "engine-stderr.log"} else "unclassified-legacy",
                "reason_kept": "not owned by TeamAgents" if False else None,
            })
    # manifest-owned temporary run data (probe workspaces only; review/tmp evidence stays)
    for parent in ("/tmp",):
        for pattern in ("teamagents-*", "r2-p5-*", "p6-*"):
            for candidate in sorted(pathlib.Path(parent).glob(pattern), key=str):
                name = candidate.name
                if not name.startswith(("teamagents-", "r2-p5-", "p6-")):
                    continue
                if candidate.is_symlink():
                    continue
                # current-turn evidence lives under the repo, not /tmp; skip the
                # shared toolchain/state helpers other tests rely on
                if name in {"teamagents-toolchain"}:
                    continue
                kept = referenced(candidate.name)
                entries.append({
                    "path": str(candidate),
                    "type": "dir" if candidate.is_dir() else "file",
                    "bytes": size_of(candidate),
                    "owner": "temporary-run-data",
                    "reason_kept": f"referenced by {kept}" if kept else None,
                })
    return entries


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--out", default="review/tmp/r28")
    args = parser.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    entries = inventory()
    receipt = {"state_root": str(STATE), "keep_under_state": sorted(KEEP_UNDER_STATE), "entries": entries}
    (out / "inventory.json").write_text(json.dumps(receipt, ensure_ascii=False, indent=2), encoding="utf-8")
    total = sum(entry["bytes"] for entry in entries)
    print(f"inventory: {len(entries)} entries, {total/1e6:.1f} MB -> {out/'inventory.json'}")
    for entry in entries:
        print(f"  [{entry['owner']}] {entry['path']} ({entry['bytes']} bytes)")
    if not args.apply:
        print("dry run; pass --apply to delete exactly these entries")
        return 0
    cleaned, failed = [], []
    for entry in entries:
        if entry.get("reason_kept"):
            continue  # evidence the repository cites: never deleted
        path = pathlib.Path(entry["path"])
        if path.is_symlink():
            failed.append({"path": str(path), "error": "symlink refused"})
            continue
        try:
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists():
                path.unlink()
            cleaned.append(entry["path"])
        except OSError as error:
            failed.append({"path": entry["path"], "error": str(error)})
    (out / "receipt.json").write_text(
        json.dumps({"cleaned": cleaned, "failed": failed, "kept": sorted(KEEP_UNDER_STATE)},
                   ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"cleaned {len(cleaned)}, failed {len(failed)} -> {out/'receipt.json'}")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""The frozen evaluation manifests still describe the material in the tree (D-145)

`review/eval/r2-p6/` is pre-registered evidence: each manifest pins the analysis script's sha256 ("frozen
before the run") and every task's prompt, fixture and check-list digests, at a recorded git commit. Nothing
checked that the tree still matches, and it does not: `478d679` translated `analyze.py` into English (the
repository's language rule), so the file's digest differs from the one all three manifests record — silently, so
a reader who verifies gets a mismatch and cannot tell whether the *rule* changed.

This audit answers that, without touching the frozen material:

* every task's `prompt.md`, `fixture/**` and `checks` are recomputed and compared with the manifest;
* the analysis script (`analyze.py`) and the driver (`run.py`) are compared with the bytes at the manifest's
  **frozen commit**, recovered with `git show`, by **AST** with every string literal stripped — so a change to
  comments or printed text is reported as a note (the digest differs, the rule does not), while a change to the
  rule itself is a failure;
* a digest that matches the manifest is reported as such.

A structural change to the driver is allowed only when the manifest **records** it (`driver_changes`, one
entry per decision): the pre-registered bytes produced the recorded verdicts, so a later change to the code
that runs the trials is a deviation to be written down, not a silent edit (D-182's header field is the first).

    python3 review/eval_manifests.py

Ceiling: "the rule is identical" is a statement about syntax trees, not behaviour — a translated *format string*
cannot change a number, but a changed comparison would change the tree, and that is what fails. The audit needs
the frozen commits to be present in the repository's history (`git show <commit>:<path>`).
"""
import ast
import hashlib
import json
import pathlib
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
EVAL = REPO / "review/eval/r2-p6"
MANIFESTS = ["manifest.json", "manifest-r2.json", "manifest-r3.json", "manifest-r4.json",
             "manifest-r5-recon.json", "manifest-r5-calib12.json", "manifest-r6.json", "manifest-r7.json",
             "manifest-r8.json", "manifest-r8-4096.json", "manifest-r9-paratera.json"]
# The pins the manifests carry: the analysis and the driver (the latter is not pinned by any manifest, and is
# checked for the same reason — it is the code that ran the trials).
SCRIPTS = ["analyze.py", "run.py"]


def is_cache(path: pathlib.Path) -> bool:
    """A build artefact, not fixture material (D-259: a stray `__pycache__` moved a frozen digest once)."""
    return path.suffix == ".pyc" or "__pycache__" in path.parts


def tree_digest(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    for file in sorted(p for p in path.rglob("*") if p.is_file() and not is_cache(p)):
        h.update(str(file.relative_to(path)).encode())
        h.update(file.read_bytes())
    return h.hexdigest()


def skeleton(source: bytes) -> str:
    """The syntax tree with every string literal replaced: comments and text cannot make it differ."""
    tree = ast.parse(source.decode())
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            node.value = "<text>"
    return ast.dump(tree)


def at_commit(commit: str, relative: str):
    """The file's bytes at one commit, or None when it did not exist there."""
    done = subprocess.run(["git", "-C", str(REPO), "show", f"{commit}:{relative}"], capture_output=True)
    return done.stdout if done.returncode == 0 else None


def commit_of(digest: str, relative: str):
    """The newest commit whose version of `relative` hashes to `digest`.

    The bytes are the reference, not the manifest's `git` field: the manifests record the commit the freeze
    happened *at*, and these files were added by the commit that ran the trials afterwards, so `git show
    <frozen>:<path>` finds nothing (measured 2026-09-26). Searching the file's own history by digest finds the
    pre-registered bytes wherever they are — and fails when they are not in history at all.
    """
    log = subprocess.run(["git", "-C", str(REPO), "log", "--format=%H", "--", relative],
                         capture_output=True, text=True).stdout.split()
    for candidate in log:
        blob = at_commit(candidate, relative)
        if blob is not None and hashlib.sha256(blob).hexdigest() == digest:
            return candidate
    return None


def main() -> int:
    findings, notes = [], []
    for name in MANIFESTS:
        path = EVAL / name
        if not path.is_file():
            findings.append(f"{name} is missing")
            continue
        manifest = json.loads(path.read_text())
        for task in manifest["tasks"]:
            directory = EVAL / "tasks" / task["id"]
            if hashlib.sha256((directory / "prompt.md").read_bytes()).hexdigest() != task["prompt_sha256"]:
                findings.append(f"{name}: {task['id']} prompt.md no longer matches its digest")
            if task.get("fixture_sha256") and tree_digest(directory / "fixture") != task["fixture_sha256"]:
                findings.append(f"{name}: {task['id']} fixture no longer matches its digest")
            if (directory / "checks.txt").read_text().splitlines() != task["checks"]:
                findings.append(f"{name}: {task['id']} checks.txt no longer matches the manifest")
        # The pre-registered bytes are whatever hashes to the manifest's pin; the driver is compared against
        # the same snapshot, because that is the code that ran the pre-registered trials.
        pinned = manifest["analysis"].get("sha256")
        snapshot = commit_of(pinned, "review/eval/r2-p6/analyze.py") if pinned else None
        if snapshot is None:
            findings.append(f"{name}: no commit in this history holds the pinned analysis digest {pinned} — "
                            f"the pre-registered rule cannot be recovered")
            continue
        for script in SCRIPTS:
            relative = f"review/eval/r2-p6/{script}"
            current = (REPO / relative).read_bytes()
            if hashlib.sha256(current).hexdigest() == pinned:
                continue
            frozen = at_commit(snapshot, relative)
            if frozen is None:
                findings.append(f"{name}: {script} does not exist at the pre-registered commit {snapshot[:12]}")
            elif skeleton(frozen) == skeleton(current):
                notes.append(f"{name}: {script}'s digest differs from the recorded one (its text was "
                             f"translated), and its rule is identical to {snapshot[:12]}")
            else:
                # A structural change to the driver is allowed only when the manifest records it: the record
                # is part of the pre-registered material, so it is reviewed like the rest of it (D-182's
                # `driver_changes`, the first such entry).
                recorded = manifest.get("driver_changes") or []
                if script == "run.py" and recorded:
                    for change in recorded:
                        notes.append(f"{name}: {script} differs from the pre-registered {snapshot[:12]} by "
                                     f"decision {change.get('decision')}: {change.get('what')} — the recorded "
                                     "verdicts were produced by the frozen bytes, this change is afterwards")
                else:
                    findings.append(f"{name}: {script} differs from the pre-registered {snapshot[:12]} in its "
                                    f"*rule*, not only in its text (record it in the manifest's "
                                    "`driver_changes` if the change is intended)")
    print(f"{len(MANIFESTS)} manifests checked: tasks, fixtures, checks, the analysis and the driver")
    for note in sorted(set(notes)):
        print("note:", note)
    for finding in findings:
        print("FAIL:", finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

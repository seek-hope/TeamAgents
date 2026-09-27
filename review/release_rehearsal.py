#!/usr/bin/env python3
"""Rehearse the release workflow locally, short of publishing (D-217).

`.github/workflows/release.yml`'s build, package and smoke steps, run against the tree as it stands: the version
gate, the musl-static release build of both binaries, the archive carrying exactly the files the workflow packages,
SHA256SUMS over the archive and the installer, and the workflow's own smoke — install from the exact archive, `init`
writing a config and a state root, the TUI binary executable. Publishing is not rehearsed (it needs a tag and
GitHub); everything before it is.

    make release-rehearsal
    python3 review/release_rehearsal.py --keep    # leave the archive and the installed tree for inspection

It is a build, not a `make check`: the release profile is its own cache and full musl release builds cost minutes.
`review/build_references.py` (in `make hygiene`) covers the half a ledger can cover — every file the workflow copies
exists and is tracked — and this covers the half a reading cannot: that the steps run, and that the installer, `init`
and the TUI work on the archive this tree would publish. The one difference from the workflow is the machine: it runs
the pinned toolchain on ubuntu-latest, this runs it here, which is what makes the rehearsal cheap and is why its
result is evidence about the *steps*, not about the runner's image. The tag half of the version gate
(`GITHUB_REF_NAME = v<version>`) fires only on a tag push; the rehearsal reports the version it would require
rather than proving a tag matches.
"""
import argparse
import atexit
import hashlib
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[1]
TARGET = "x86_64-unknown-linux-musl"
# what the workflow puts in the archive, in its own order (D-216 gates that each of these exists)
ARCHIVE_FILES = ("README.md", "examples/config.toml", "install.sh", "docs", "examples", "AGENTS.md")


def version_of(crate: str) -> str:
    """The version one crate's manifest declares."""
    text = (REPO / crate / "Cargo.toml").read_text(encoding="utf-8")
    for line in text.split("\n"):
        if line.startswith("version = "):
            return line.split('"')[1]
    raise SystemExit(f"{crate}/Cargo.toml has no version line")


def version() -> str:
    text = (REPO / "engine" / "Cargo.toml").read_text(encoding="utf-8")
    for line in text.split("\n"):
        if line.startswith("version = "):
            return line.split('"')[1]
    raise SystemExit("engine/Cargo.toml has no version line")


def run(step: str, command: list, **kwargs) -> None:
    """One rehearsal step: print it, run it, and fail with the tail of its output."""
    print(f"[{step}] {' '.join(command)}")
    done = subprocess.run(command, cwd=kwargs.pop("cwd", REPO), text=True, capture_output=True, **kwargs)
    if done.returncode != 0:
        print(f"FAIL: {step} exited {done.returncode}")
        print("\n".join((done.stdout + done.stderr).strip().split("\n")[-12:]))
        raise SystemExit(1)


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keep", action="store_true", help="keep the scratch directory (the archive and the install)")
    args = parser.parse_args(argv)
    if not shutil.which("cargo") or not shutil.which("musl-gcc"):
        raise SystemExit("the rehearsal needs cargo and musl-gcc (the release build links musl statically)")
    found = subprocess.run(["rustup", "target", "list", "--installed"], capture_output=True, text=True)
    if TARGET not in found.stdout:
        raise SystemExit(f"the rehearsal needs the {TARGET} target: rustup target add {TARGET}")

    # 1. the version gate the workflow runs first, and the crates' agreement
    build_version = version()
    for crate in ("core", "tui"):
        if version_of(crate) != build_version:
            print(f"FAIL: {crate} carries {version_of(crate)}, engine carries {build_version}: the workflow would "
                  "refuse the tag")
            return 1
    print(f"[version] {build_version}, and core and tui agree")

    scratch = pathlib.Path(tempfile.mkdtemp(prefix="ta-release-rehearsal-"))
    if not args.keep:
        atexit.register(shutil.rmtree, scratch, ignore_errors=True)
    name = f"teamagents-{build_version}-{TARGET}"
    dist = scratch / "dist"
    (dist / name).mkdir(parents=True)

    # 2. the release build: the same flags (strip included), the same target, the same binaries
    strip = {**os.environ, "CARGO_PROFILE_RELEASE_STRIP": "symbols"}
    for crate, binary in (("engine", "teamagents"), ("tui", "teamagents-tui")):
        run(f"build {binary}", ["cargo", "build", "--locked", "--release", "--target", TARGET,
                                "--manifest-path", f"{crate}/Cargo.toml", "--bin", binary], env=strip)

    # 3. the archive: the workflow's own file list
    payload = dist / name
    for entry in ARCHIVE_FILES:
        source = REPO / entry
        if entry == "examples/config.toml":
            shutil.copy2(source, payload / "config.example.toml")
        elif entry == "docs" or entry == "examples":
            shutil.copytree(source, payload / entry, dirs_exist_ok=True)
        else:
            shutil.copy2(source, payload / entry)
    for crate, binary in (("engine", "teamagents"), ("tui", "teamagents-tui")):
        shutil.copy2(REPO / crate / "target" / TARGET / "release" / binary, payload / binary)
        os.chmod(payload / binary, 0o755)
    run("help", [str(payload / "teamagents"), "--help"])
    run("version", [str(payload / "teamagents"), "version"])
    archive = dist / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(payload, arcname=name)
    shutil.copy2(REPO / "install.sh", dist / "install.sh")
    manifest = dist / "SHA256SUMS"
    lines = []
    for entry in (archive, dist / "install.sh"):
        lines.append(f"{hashlib.sha256(entry.read_bytes()).hexdigest()}  {entry.name}")
    manifest.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"[package] {archive.name} ({archive.stat().st_size // 1024} KiB) + install.sh + SHA256SUMS")

    # 4. the workflow's smoke: install from the exact archive, then init. Both XDG homes point into the scratch,
    # which is the one thing a smaller rehearsal got wrong: with only the config home isolated, `init` prepared
    # its state root under the *host's* `~/.local/state` (read-only here, writable on a runner) — the smoke was
    # not hermetic and a local rehearsal of it could not run at all. The workflow isolates both now, so this is
    # the step it runs.
    bin_dir = scratch / "bin"
    config_home = scratch / "config"
    state_home = scratch / "state"
    smoke_env = {**os.environ, "XDG_CONFIG_HOME": str(config_home), "XDG_STATE_HOME": str(state_home)}
    run("install", ["sh", str(dist / "install.sh"), "--archive", str(archive), "--bin-dir", str(bin_dir)],
        env=smoke_env)
    run("init", [str(bin_dir / "teamagents"), "init"], env=smoke_env)
    written = config_home / "teamagents" / "config.toml"
    if not written.is_file() or written.stat().st_size == 0:
        print(f"FAIL: init wrote no config at {written}")
        return 1
    if not os.access(bin_dir / "teamagents-tui", os.X_OK):
        print("FAIL: the TUI binary is not executable after the install")
        return 1
    if not (state_home / "teamagents").is_dir():
        print(f"FAIL: init prepared no state root under {state_home}: the smoke reached a home outside the "
              "scratch, or init stopped after writing the config")
        return 1
    print(f"[smoke] installed into {bin_dir}, init wrote {written.relative_to(scratch)} and its state root "
          f"under {state_home.relative_to(scratch)}")
    if args.keep:
        print(f"kept: {scratch}")
    print(f"the release path rehearsed: {build_version} packages, installs and runs from this tree")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

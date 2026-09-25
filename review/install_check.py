#!/usr/bin/env python3
"""Check the documented install path (D-37) end to end, including its failure mode.

`docs/INSTALL.md` promises download-and-run: the installer picks a release, verifies SHA-256,
installs `teamagents` and `teamagents-tui` into one directory, and — the part that matters when
something goes wrong — **leaves an existing installation untouched when verification fails**.

    python3 review/install_check.py                 # the latest release, needs network
    python3 review/install_check.py --version 0.1.2 # a pinned release
    python3 review/install_check.py --offline       # only the local refusal check

It is not part of `make check` (it needs the network and a published release); everything it
writes stays under a fresh /tmp directory.
"""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import ssl
import subprocess
import sys
import urllib.request

REPO = pathlib.Path(__file__).resolve().parent.parent
INSTALLER = REPO / "install.sh"
ASSET = "teamagents-{version}-x86_64-unknown-linux-musl.tar.gz"


def tls_context() -> ssl.SSLContext:
    """Verify TLS properly: use the CA bundle the environment names, else the system
    bundle, else the platform default. Nothing here disables verification — the point
    of this check is the installer's checksum, so its transport must be verified too."""
    named = next(
        (os.environ[name] for name in ("SSL_CERT_FILE", "CURL_CA_BUNDLE", "REQUESTS_CA_BUNDLE") if os.environ.get(name)),
        None,
    )
    if named and pathlib.Path(named).is_file():
        return ssl.create_default_context(cafile=named)
    system = pathlib.Path("/etc/ssl/certs/ca-certificates.crt")
    return ssl.create_default_context(cafile=str(system)) if system.is_file() else ssl.create_default_context()


def fetch(url: str, timeout: int) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "teamagents-install-check"})
    with urllib.request.urlopen(request, timeout=timeout, context=tls_context()) as reply:
        return reply.read()


def latest_version() -> str:
    """The tag the docs' 'latest release' link resolves to."""
    return json.loads(fetch("https://api.github.com/repos/seek-hope/TeamAgents/releases/latest", 30))["tag_name"].lstrip("v")


def local_refusal_check(scratch: pathlib.Path, failures: list[str]) -> None:
    """The promise that a failed verification never disturbs an installation."""
    assets = scratch / "corrupt"
    bin_dir = scratch / "corrupt-bin"
    assets.mkdir(parents=True)
    bin_dir.mkdir(parents=True)
    archive = assets / ASSET.format(version="9.9.9")
    archive.write_text("this is not a tarball\n")
    (assets / "SHA256SUMS").write_text(f"{'0' * 64}  {archive.name}\n")
    done = subprocess.run(
        ["sh", str(INSTALLER), "--archive", str(archive), "--bin-dir", str(bin_dir)],
        capture_output=True, text=True,
    )
    output = (done.stdout + done.stderr).strip()
    print(f"  corrupted archive: exit={done.returncode}")
    print(f"  {output.splitlines()[-1] if output else '(no output)'}")
    if done.returncode == 0:
        failures.append("a corrupted archive was installed")
    if "verification failed" not in output and "did NOT match" not in output:
        failures.append(f"the failure does not name the checksum: {output[-200:]}")
    if any(bin_dir.iterdir()):
        failures.append(f"a refused install still wrote into the bin directory: {list(bin_dir.iterdir())}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", help="pin a release (leading v accepted)")
    parser.add_argument("--offline", action="store_true", help="only run the local refusal check")
    parser.add_argument("--state-dir", default="/tmp/ta-install-check")
    args = parser.parse_args()
    if not INSTALLER.is_file():
        raise SystemExit(f"{INSTALLER} is missing")
    scratch = pathlib.Path(args.state_dir)
    shutil.rmtree(scratch, ignore_errors=True)
    scratch.mkdir(parents=True)
    failures: list[str] = []

    local_refusal_check(scratch, failures)
    if args.offline:
        for failure in failures:
            print("FAIL:", failure)
        return 1 if failures else 0

    version = (args.version or "").lstrip("v")
    if not version:
        try:
            version = latest_version()
        except Exception as error:  # network or API refusal
            print(f"FAIL: cannot resolve the latest release: {error}")
            return 1
    print(f"  installing the published release v{version}")
    assets = scratch / "release"
    assets.mkdir()
    name = ASSET.format(version=version)
    base = f"https://github.com/seek-hope/TeamAgents/releases/download/v{version}"
    try:
        (assets / name).write_bytes(fetch(f"{base}/{name}", 120))
        (assets / "SHA256SUMS").write_bytes(fetch(f"{base}/SHA256SUMS", 60))
    except Exception as error:
        print(f"FAIL: cannot download v{version}: {error}")
        return 1
    # what the manifest claims, checked here as well as by the installer
    manifest = (assets / "SHA256SUMS").read_text()
    claimed = next((line.split()[0] for line in manifest.splitlines() if line.strip().endswith(name)), None)
    actual = hashlib.sha256((assets / name).read_bytes()).hexdigest()
    if claimed is None:
        failures.append(f"SHA256SUMS does not list {name}")
    elif claimed != actual:
        failures.append(f"the published checksum for {name} does not match the archive")

    bin_dir = scratch / "bin"
    done = subprocess.run(
        ["sh", str(INSTALLER), "--archive", str(assets / name), "--bin-dir", str(bin_dir)],
        capture_output=True, text=True,
    )
    print(f"  installer exit={done.returncode}")
    if done.returncode != 0:
        failures.append(f"the installer failed: {(done.stderr or done.stdout).strip()[-300:]}")
    for binary in ("teamagents", "teamagents-tui"):
        path = bin_dir / binary
        if not path.is_file() or not os.access(path, os.X_OK):
            failures.append(f"{binary} was not installed into {bin_dir}")
    if not failures:
        version_out = subprocess.run([str(bin_dir / "teamagents"), "version"], capture_output=True, text=True)
        print(f"  [mechanics] the installed binaries run: version exit={version_out.returncode}")
        if version_out.returncode != 0 or version not in version_out.stdout:
            failures.append(f"the installed binary does not report v{version}: {version_out.stdout.strip()}")
        help_out = subprocess.run([str(bin_dir / "teamagents"), "--help"], capture_output=True, text=True)
        help_text = help_out.stdout + help_out.stderr
        # a usage line, on whichever stream the release chose (the current tree prints
        # `--help` to stdout and a *usage error* to stderr)
        # A usage/help screen either way: the current tree prints an English `usage:` line,
        # while the published release's help is not in English at all (see the gap below), so
        # this looks for the entry-point list a help screen must carry instead of the word.
        if help_out.returncode != 0 or "teamagents" not in help_text or "--help" not in help_text:
            failures.append("the installed binary does not print its help")
        # the TUI must refuse a session-less start instead of pretending
        tui = subprocess.run([str(bin_dir / "teamagents-tui")], capture_output=True, text=True)
        if tui.returncode == 0:
            failures.append("teamagents-tui started with no session and no terminal")
        # …and the artifact must be *the documented product*. The released v0.1.2 is the
        # earlier implementation: a Chinese help that still offers validate/sessions/
        # --plain/--resume/--team, none of which the documented surface has (D-52/D-73).
        # Installing "the latest release" therefore does not install what the docs
        # describe, and the tree's own version already names that older release.
        stale = [entry for entry in ("validate", "sessions prune", "--plain", "--team SPEC") if entry in help_text]
        if stale:
            failures.append(
                f"the published v{version} predates the documented product: its help still offers "
                f"{stale} and is not the current surface - release the tree under a new version "
                f"(see ACCEPTANCE's known gaps) before this path installs what the docs describe"
            )
        else:
            print(f"  [product] the installed help describes the current surface")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

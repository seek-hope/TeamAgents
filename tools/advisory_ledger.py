#!/usr/bin/env python3
"""Hold the reviewed advisory exceptions in `deny.toml` to what cargo-deny actually reports.

An `ignore` entry is a promise: "this advisory was read, it affects only an accepted path, and no patched
release exists". Two ways that promise goes stale, and neither is visible in today's exit status:

* the entry stops being reported at all (the dependency was upgraded, or the advisory withdrawn), so the
  exemption outlives its reason and would silently re-permit a future regression under the same ID;
* an unmaintained advisory appears that no entry covers, so the reviewed set no longer describes the graph.

cargo-deny reports both as `warning[advisory-not-detected]` and `error[unmaintained]`, and *still exits 0* when
the second one is covered — so neither direction is a gate today. This audit turns them into a failure.

The three crates are three standalone graphs with no `[workspace]` table, so an advisory reachable from `tui`
is not reachable from `core`. An entry is therefore judged globally: it is stale only when no crate reports it.
This is a consistency audit of the policy, not a proof that an exempted crate is safe to use.

    python3 tools/advisory_ledger.py            # audit the three crate graphs
    python3 tools/advisory_ledger.py --self-check
"""
import argparse
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
CRATES = ("core", "engine", "tui")
IDENT = re.compile(r"RUSTSEC-[0-9]{4}-[0-9]+")
HEADER = re.compile(r"^(error|warning)\[([a-z-]+)\]")
IGNORE = re.compile(r"^ignore\s*=\s*\[(.*?)\]", re.M | re.S)


def ignore_list(text: str) -> list:
    """The advisory IDs in the config's `ignore` array, in file order."""
    match = IGNORE.search(text)
    return IDENT.findall(match.group(1)) if match else []


def diagnostics(text: str) -> list:
    """`(kind, block)` for each top-level cargo-deny diagnostic, in order.

    A block runs from its header to the next top-level `error[...]`/`warning[...]` line. Only the
    `advisory-not-detected` header names its advisory ID (in the quoted config line it points at); an
    `unmaintained` header names the crate in prose and its ID nowhere, which is why detection and staleness
    are read from different kinds.
    """
    out, current, kind = [], [], None
    for line in text.split("\n"):
        header = HEADER.match(line)
        if header:
            if kind is not None:
                out.append((kind, "\n".join(current)))
            kind, current = header.group(2), [line]
            continue
        if kind is not None:
            current.append(line)
    if kind is not None:
        out.append((kind, "\n".join(current)))
    return out


def classified(text: str) -> tuple:
    """`(unmaintained_blocks, stale_ids)` from one cargo-deny run."""
    parsed = diagnostics(text)
    unmaintained = [block for kind, block in parsed if kind == "unmaintained"]
    stale = {ident for kind, block in parsed if kind == "advisory-not-detected"
             for ident in IDENT.findall(block)}
    return unmaintained, stale


def run(crate: str, config: pathlib.Path) -> str:
    """cargo-deny's combined output for one crate *under `config`*; exit 0 and 1 are both verdicts.

    `--config` is passed explicitly: without it cargo-deny discovers the repository's own `deny.toml` by
    walking up from the manifest, so an audit of a proposed policy would silently re-audit the current one.
    """
    result = subprocess.run(
        ["cargo", "deny", "--manifest-path", f"{crate}/Cargo.toml",
         "--config", str(config), "check", "advisories"],
        cwd=REPO, capture_output=True, text=True,
    )
    if result.returncode not in (0, 1):
        raise SystemExit(f"cargo deny could not run for {crate} (exit {result.returncode}):\n"
                         f"{(result.stdout + result.stderr)[-2000:]}")
    return result.stdout + result.stderr


def audit(config: pathlib.Path) -> int:
    """Fail on an unmaintained advisory the reviewed list omits, or on an exemption no crate reports."""
    listed = ignore_list(config.read_text(encoding="utf-8"))
    print(f"{len(listed)} reviewed advisory exception(s) in {config.name}: {', '.join(listed) or '(none)'}")
    findings, stale_in, detected_in = [], {}, set()
    for crate in CRATES:
        unmaintained, stale = classified(run(crate, config))
        for ident in stale:
            stale_in.setdefault(ident, []).append(crate)
        detected_in |= set(listed) - stale
        for block in unmaintained:
            first = block.split("\n")[0]
            findings.append(f"{crate}: {first.strip()} — no entry in {config.name} covers it, so an "
                            f"unmaintained dependency is shipping unreviewed")
    for ident in listed:
        if ident not in detected_in and len(stale_in.get(ident, [])) == len(CRATES):
            findings.append(f"{config.name} exempts {ident}, but no crate reports it — the exemption has "
                            f"outlived its reason; remove the entry")
    for ident in sorted(detected_in):
        print(f"  {ident}: still reported and covered")
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"every reviewed exception is still reported, and no crate ships an unreviewed unmaintained advisory")
    return 0


# Fixtures captured from cargo-deny 0.20.2: the shapes this audit depends on, not invented ones.
NOT_DETECTED = """warning[advisory-not-detected]: advisory was not encountered
   ┌─ deny.toml:17:6
   │
17 │     "RUSTSEC-2024-0320",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

advisories ok
"""
UNMAINTAINED = """error[unmaintained]: Bincode is unmaintained
   ┌─ tui/Cargo.lock:12:1
   │
12 │ bincode 1.3.3 registry+https://github.com/rust-lang/crates.io-index
   │ ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ unmaintained advisory detected
"""


def self_check() -> int:
    """Both staleness directions must be reproduced from the real output shapes."""
    assert ignore_list('ignore = [\n    "RUSTSEC-2025-0141",\n    "RUSTSEC-2026-0543",\n]\n') == \
        ["RUSTSEC-2025-0141", "RUSTSEC-2026-0543"], "ignore parsing keeps file order"
    assert ignore_list("[advisories]\nversion = 2\n") == [], "an absent ignore array is empty, not an error"

    covered, stale = classified(NOT_DETECTED)
    assert covered == [], "a not-detected block is not an unmaintained finding"
    assert stale == {"RUSTSEC-2024-0320"}, "a not-detected block names the ID it exempts"

    live, none = classified(UNMAINTAINED)
    assert none == set(), "an unmaintained block carries no ID to misread as stale"
    assert len(live) == 1 and "Bincode is unmaintained" in live[0], "an unmaintained block is a finding"

    both, _ = classified(UNMAINTAINED + NOT_DETECTED)
    assert len(both) == 1, "one unmaintained header in a mixed stream"
    empty, _ = classified("advisories ok\nsources ok\n")
    assert empty == [], "a clean run has no unmaintained blocks"
    print("advisory ledger controls passed: ignore parsing, both staleness directions, clean run")
    return 0


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", default="deny.toml")
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args(argv)
    if args.self_check:
        return self_check()
    self_check()  # the controls run with the gate (citations.py's convention), not only on demand
    return audit(REPO / args.config)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

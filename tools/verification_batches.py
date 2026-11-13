#!/usr/bin/env python3
"""Check identical positive models once with the union of their declared claims.

No bounds, transitions, fairness, constraints or views are changed. Group only
our narrow SPECIFICATION/CONSTANTS/indented-claims grammar; unknown syntax fails
closed. Every result retains the source configs and generated replay config.
"""
import argparse
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import time

from tlc_verdict import verdict
from verification_catalogue import EXPECTS

CLAIM_SECTION = re.compile(r"(?m)^(INVARIANTS|PROPERTIES)\s*\n((?:[ \t]+[A-Za-z_]\w*[ \t]*\n)+)")
DIRECTIVES = set("INIT NEXT SPECIFICATION INVARIANT INVARIANTS PROPERTY PROPERTIES CONSTANT CONSTANTS "
                 "CONSTRAINT CONSTRAINTS ACTION_CONSTRAINT ACTION_CONSTRAINTS SYMMETRY VIEW ALIAS "
                 "POSTCONDITION CHECK_DEADLOCK TRUE FALSE".split())


def split_config(text):
    # These configurations contain only line comments, no block comments or
    # multiline strings. Do not silently normalize an unknown TLC directive.
    if EXPECTS.search(text):
        raise ValueError("fault controls must run separately")
    if "(*" in text or re.search(r'"[^"\n]*\\\*', text):
        raise ValueError("unsupported comments/strings in positive configuration")
    clean = re.sub(r"\\\*[^\n]*", "", text)
    claims = {"INVARIANTS": set(), "PROPERTIES": set()}
    for section, body in CLAIM_SECTION.findall(clean):
        names = set(body.split())
        if names & DIRECTIVES:
            raise ValueError("TLC directives cannot be indented claim names")
        claims[section].update(names)
    base = CLAIM_SECTION.sub("", clean)
    for line in base.splitlines():
        value = line.strip()
        if not value:
            continue
        if re.fullmatch(r"SPECIFICATION\s+\w+|CONSTANTS?(?:\s+\w+\s*(?:=|<-)\s*.+)?|CHECK_DEADLOCK (?:TRUE|FALSE)", value):
            continue
        if line[:1].isspace() and re.fullmatch(r"\w+\s*(?:=|<-)\s*.+", value):
            continue
        raise ValueError(f"unsupported positive configuration line: {value}")
    if len(re.findall(r"(?m)^SPECIFICATION\s+\w+\s*$", base)) != 1:
        raise ValueError("positive configuration needs one SPECIFICATION")
    # Preserve whitespace inside strings; normalized nonempty lines are enough
    # to match our identical domains without attempting to parse TLA expressions.
    key = tuple(line.strip() for line in base.splitlines() if line.strip())
    return key, base.strip() + "\n", claims


def batches(pairs):
    grouped, seen, basenames = {}, set(), set()
    for config, model in pairs:
        if config.resolve() in seen or config.name in basenames:
            raise ValueError(f"duplicate configuration path or basename: {config}")
        seen.add(config.resolve())
        basenames.add(config.name)
        if not config.is_file() or not model.is_file():
            raise ValueError("configuration or model does not exist")
        source = config.read_bytes()
        key, base, claims = split_config(source.decode())
        group = grouped.setdefault((model.resolve(), key),
                                   {"model": model, "configs": {}, "base": base,
                                    "claims": {"INVARIANTS": set(), "PROPERTIES": set()}})
        group["configs"][config] = source
        for section, names in claims.items():
            group["claims"][section].update(names)
    return list(grouped.values())


def configuration(group):
    return group["base"] + "".join(section + "\n" + "".join(f"    {name}\n" for name in sorted(names))
                                   for section, names in group["claims"].items() if names)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def heap_flag():
    """Size TLC's heap to avoid spurious low-memory liveness warnings on 16 GiB CI hosts."""
    try:
        for line in pathlib.Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                kib = int(line.split()[1])
                if kib >= 12 * 1024 * 1024:
                    return "-Xmx8g"
                break
    except (OSError, ValueError, IndexError):
        pass
    return "-Xmx4g"


def self_check():
    with tempfile.TemporaryDirectory(prefix="teamagents-batch-controls-") as directory:
        root = pathlib.Path(directory)
        model = root / "Model.tla"
        model.write_text("model identity\n")
        first, second = root / "a.cfg", root / "b.cfg"
        first.write_text('SPECIFICATION Spec\nCONSTANTS\n    N = 2\nINVARIANTS\n    Safe\nPROPERTIES\n    Live\n')
        second.write_text('\\* comment\nSPECIFICATION Spec\nCONSTANTS\n    N = 2\nINVARIANTS\n    Other\n')
        groups = batches([(first, model), (second, model)])
        assert len(groups) == 1
        assert split_config(configuration(groups[0]))[2] == {
            "INVARIANTS": {"Safe", "Other"}, "PROPERTIES": {"Live"}}
        for old, new in [('N = 2', 'N = 3'), ('Spec', 'OtherSpec')]:
            original = second.read_text()
            second.write_text(original.replace(old, new))
            assert len(batches([(first, model), (second, model)])) == 2
            second.write_text(original)
        other = root / "Other.tla"
        other.write_text(model.read_text())
        assert len(batches([(first, model), (second, other)])) == 2
        second.write_text(second.read_text() + "CHECK_DEADLOCK FALSE\n")
        assert len(batches([(first, model), (second, model)])) == 2
        for directive in ["CONSTRAINT Safe", "VIEW Project", "SYMMETRY Names", "PROPERTY Live",
                          "    VIEW Project", "    PROPERTIES\n    Other", "    INVARIANTS\n    Other",
                          "\\* expects: Safe", "(* unsupported block *)"]:
            try:
                split_config(first.read_text() + directive + "\n")
            except ValueError:
                continue
            raise AssertionError(f"unknown directive accepted: {directive}")
        nested = root / "nested"
        nested.mkdir()
        duplicate = nested / first.name
        duplicate.write_bytes(first.read_bytes())
        try:
            batches([(first, model), (duplicate, model)])
        except ValueError:
            pass
        else:
            raise AssertionError("duplicate basename accepted")
    print("positive batching controls passed: claim union, domain/spec/model separation, unknown directives and duplicate names")


def check_fixtures(jar):
    """Exercise the runner with a passing union and failures in the second config."""
    with tempfile.TemporaryDirectory(prefix="teamagents-batch-fixtures-") as directory:
        root = pathlib.Path(directory)
        model = root / "BatchFixture.tla"
        model.write_text("---- MODULE BatchFixture ----\nEXTENDS Naturals\nVARIABLE x\n"
                         "Init == x = 0\nNext == x' = 1\nSpec == Init /\\ [][Next]_x\n"
                         "Safe == x \\in 0..1\nOther == x >= 0\nBad == x = 0\n"
                         "Never == <>(x = 2)\n====\n")
        first, second = root / "first.cfg", root / "second.cfg"
        first.write_text("SPECIFICATION Spec\nINVARIANTS\n    Safe\n")
        for label, claim, expected in [("valid", "INVARIANTS\n    Other", 0),
                                       ("safety", "INVARIANTS\n    Bad", 12),
                                       ("temporal", "PROPERTIES\n    Never", 13)]:
            second.write_text(f"SPECIFICATION Spec\n{claim}\n")
            out = root / label
            run = subprocess.run([sys.executable, str(pathlib.Path(__file__).resolve()),
                                  "--jar", str(jar), "--out", str(out), "--workers", "1",
                                  f"{first}:{model}", f"{second}:{model}"],
                                 capture_output=True, text=True)
            result = json.loads((out / "results.json").read_text())
            if (result["configs"] != 2 or result["traversals"] != 1
                    or len(result["runs"]) != 1 or result["runs"][0]["exit"] != expected
                    or run.returncode != (0 if expected == 0 else 1)):
                raise ValueError(f"batch fixture {label} failed: {run.stdout}{run.stderr}")
    print("executed batch fixtures passed: union checked; second-config safety and liveness failures rejected")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pairs", nargs="*", help="CONFIG:MODEL pairs")
    parser.add_argument("--jar", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--self-check", action="store_true")
    parser.add_argument("--check-fixtures", action="store_true")
    args = parser.parse_args()
    if args.self_check:
        self_check()
        return 0
    if args.check_fixtures:
        if args.jar is None:
            parser.error("--check-fixtures requires --jar")
        check_fixtures(args.jar.resolve())
        return 0
    if not args.pairs or args.jar is None or args.workers < 1:
        parser.error("pairs, --jar and positive --workers are required")
    jar = args.jar.resolve()
    pin = re.search(r"^TLA_SHA256 := (\w+)$", (pathlib.Path(__file__).resolve().parents[1] / "Makefile").read_text(), re.M)
    if pin is None or digest(jar) != pin[1]:
        raise ValueError("TLC jar does not match the Makefile pin")
    pairs = [(pathlib.Path(a).resolve(), pathlib.Path(b).resolve()) for a, b in (p.split(":", 1) for p in args.pairs)]
    groups = batches(pairs)
    out = args.out.resolve() if args.out else pathlib.Path(tempfile.mkdtemp(prefix="teamagents-positive-"))
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise ValueError("batch output must be an empty directory")
    # Pinned TLC resolves configurations beside its root module, even for an
    # absolute -config path. Keep replay inputs together in the output directory.
    for directory in {model.parent for _, model in pairs}:
        for source in directory.glob("*.tla"):
            target = out / source.name
            if target.exists() and digest(target) != digest(source):
                raise ValueError(f"conflicting module source: {source.name}")
            shutil.copy2(source, target)
    report = {"configs": len(pairs), "traversals": len(groups), "jar_sha256": digest(jar), "runs": []}
    print(f"{len(pairs)} positive configurations in {len(groups)} traversals; replay logs: {out}", flush=True)
    for number, group in enumerate(groups):
        names = [p.name for p in group["configs"]]
        cfg = out / f"batch-{number:02}.cfg"
        cfg.write_text(configuration(group))
        log = cfg.with_suffix(".log")
        print(f"== {', '.join(names)} ==", flush=True)
        start = time.monotonic()
        meta = out / f"states-{number:02}"
        command = ["java", heap_flag(), "-XX:+UseParallelGC", "-cp", str(jar), "tlc2.TLC",
                   "-lncheck", "final", "-fp", "64", "-workers", str(args.workers),
                   "-metadir", str(meta), "-config", cfg.name, group["model"].name]
        with log.open("w") as stream:
            result = subprocess.run(command, cwd=out, stdout=stream, stderr=subprocess.STDOUT)
        error = verdict(cfg.read_text(), log.read_text(), result.returncode, False)
        for p, source in group["configs"].items():
            (out / f"batch-{number:02}-source-{p.name}").write_bytes(source)
        record = {"configs": {p.name: hashlib.sha256(source).hexdigest() for p, source in group["configs"].items()},
                  "model": group["model"].name, "model_sha256": digest(out / group["model"].name),
                  "command": command,
                  "generated_config_sha256": digest(cfg), "log_sha256": digest(log),
                  "seconds": time.monotonic() - start, "exit": result.returncode, "error": error}
        report["runs"].append(record)
        (out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        for line in log.read_text().splitlines():
            if re.search(r"states generated|No error|Finished in|Warning:|^Error:", line):
                print(line, flush=True)
        shutil.rmtree(meta, ignore_errors=True)
        if error:
            raise ValueError(f"{names}: {error}; full log: {log}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, OSError) as error:
        print(f"positive batching: {error}")
        raise SystemExit(1)

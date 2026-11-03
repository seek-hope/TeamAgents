#!/usr/bin/env python3
"""Replay D-496 control-model measurements and isolated reduction experiments.

The canonical and no-stutter variants are experiments, not accepted proof inputs.
Run serially on an otherwise idle host for timing comparisons. A timeout fails.
"""
import argparse
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BASELINE = "d7e284de9f5ebd614a938bdbb762389707d4f80d"
sys.path.insert(0, str(REPO / "review"))
from tlc_verdict import verdict  # noqa: E402


def original(name):
    return subprocess.run(["git", "show", f"{BASELINE}:verification/tla/{name}"],
                          cwd=REPO, capture_output=True, check=True).stdout


def prepare(kind, config):
    current = (REPO / "verification/tla/V2Control.tla").read_text()
    old = original("V2Control.tla").decode()
    cfg = original(config).decode()
    if cfg != (REPO / "verification/tla" / config).read_text():
        raise ValueError("scenario changed since baseline; choose an unchanged configuration")
    files = {config: cfg, "V2Control.tla": old if kind == "baseline" else current}
    model = "V2Control.tla"
    if kind == "canonical":
        # Choose one unused identity, but keep every response/outcome/owner choice.
        # This needs a separate trace/fairness preservation proof before adoption.
        files[model] = current.replace("\\E r \\in FreeReqs :",
            "\\E r \\in (IF FreeReqs = {} THEN {} ELSE {CHOOSE x \\in FreeReqs : TRUE}) :").replace(
            "\\E a \\in FreeAtts", "\\E a \\in (IF FreeAtts = {} THEN {} ELSE {CHOOSE x \\in FreeAtts : TRUE})")
    elif kind == "no-stutter":
        step = "  \\/ Stutter   \\* the system is open-ended: an idle step is always possible"
        if current.count(step) != 1:
            raise ValueError("explicit stutter changed since experiment")
        files[model] = current.replace(step, "")
        files[config] += "\nCHECK_DEADLOCK FALSE\n"
    elif kind in {"equivalence", "equivalence-control"}:
        if kind == "equivalence-control":
            needle = "\\E a \\in FreeAtts, kind \\in Result :"
            if current.count(needle) != 1:
                raise ValueError("response enumeration changed since experiment")
            files[model] = current.replace(needle, '\\E a \\in FreeAtts, kind \\in {"reply"} :')
        files["Baseline.tla"] = old.replace("MODULE V2Control", "MODULE Baseline", 1)
        actions = ["RecordAttempt", "LostAttempt", "ImportResponse", "ConsumeDecision",
                   "CancelRequest", "FailRequest", "TurnStep"]
        formula = " /\\ ".join(f"(\\A i \\in Instances : ({a}(i) <=> Old!{a}(i)))" for a in actions)
        files["Equivalence.tla"] = (
            "---- MODULE Equivalence ----\nEXTENDS V2Control\nOld == INSTANCE Baseline\n"
            "EquivalenceSpec == Init /\\ [][Next \\/ Old!Next]_vars\n"
            f"StepEquivalence == [][({formula})]_vars\n====\n")
        files[config] = cfg.split("INVARIANTS")[0].replace("SPECIFICATION Spec", "SPECIFICATION EquivalenceSpec")
        files[config] += "INVARIANTS\n    TypeOK\nPROPERTIES\n    StepEquivalence\n"
        if kind == "equivalence-control":
            files[config] = "\\* expects: StepEquivalence\n" + files[config]
        model = "Equivalence.tla"
    return files, model


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=["baseline", "current", "canonical", "no-stutter",
                                          "equivalence", "equivalence-control"], required=True)
    parser.add_argument("--config", default="MC.cfg")
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--jar", type=pathlib.Path,
                        default=pathlib.Path.home() / ".local/share/teamagents-verify/tla2tools.jar")
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--coverage", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"MC(?:_control_\w+|_wide)?\.cfg", args.config):
        parser.error("choose a control-model configuration")
    if args.workers < 1 or args.timeout < 1:
        parser.error("workers and timeout must be positive")
    jar = args.jar.resolve()
    pin = re.search(r"^TLA_SHA256 := (\w+)$", (REPO / "Makefile").read_text(), re.M)
    jar_hash = hashlib.sha256(jar.read_bytes()).hexdigest()
    if pin is None or jar_hash != pin[1]:
        raise ValueError("TLC jar does not match the Makefile pin")
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    files, model = prepare(args.kind, args.config)
    for name, content in files.items():
        (out / name).write_text(content)
    command = ["java", "-Xmx3g", "-XX:+UseParallelGC", "-cp", str(jar), "tlc2.TLC",
               "-lncheck", "final", "-fp", "64", "-workers", str(args.workers),
               "-metadir", str(out / "states"), "-config", args.config]
    if args.coverage:
        command += ["-coverage", "1"]
    command.append(model)
    start = time.monotonic()
    with (out / "tlc.log").open("w") as log:
        try:
            status = subprocess.run(command, cwd=out, stdout=log, stderr=subprocess.STDOUT,
                                    timeout=args.timeout).returncode
        except subprocess.TimeoutExpired:
            status = 124
    elapsed = time.monotonic() - start
    text = (out / "tlc.log").read_text()
    error = verdict(files[args.config], text, status, args.kind == "equivalence-control")
    counts = re.findall(r"(?m)^([\d,]+) states generated, ([\d,]+) distinct states found, ([\d,]+) states left on queue\.", text)
    report = {"kind": args.kind, "baseline": BASELINE, "config": args.config,
              "command": command, "jar_sha256": jar_hash, "seconds": elapsed,
              "exit": status, "error": error,
              "counts": [int(x.replace(",", "")) for x in counts[-1]] if counts else None,
              "sha256": {name: hashlib.sha256((out / name).read_bytes()).hexdigest() for name in [*files, "tlc.log"]}}
    (out / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    shutil.rmtree(out / "states", ignore_errors=True)
    print(json.dumps(report, indent=2))
    return int(error is not None)


if __name__ == "__main__":
    raise SystemExit(main())

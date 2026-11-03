#!/usr/bin/env python3
"""Find concrete positive paths without changing a model's constants or transitions.

Each case negates a state predicate and asks TLC for its counterexample. A path is
an existential reachability witness, not a safety proof or a mutation control.
"""
import argparse
import fnmatch
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

from verification_catalogue import CFG, EXPECTS, target_recipes

REPO = pathlib.Path(__file__).resolve().parents[1]
TLA = REPO / "verification/tla"
MANIFEST = REPO / "verification/witnesses.json"
CLAIM = "ReachabilityWitnessAbsent"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_cases(path=MANIFEST):
    document = json.loads(path.read_text())
    if document.get("schema") != 1:
        raise ValueError("unsupported witness manifest schema")
    makefile = (REPO / "Makefile").read_text()
    positive = set(CFG.findall(target_recipes(makefile)["verify-model-all"]))
    arms = [(pattern, module) for group, module in re.findall(
        r"([A-Za-z0-9_.*|]+)\)\s*echo\s+(V2[A-Za-z0-9_]*)\.tla", makefile)
        for pattern in group.split("|") if pattern != "*"]
    seen = set()
    for case in document["cases"]:
        name = case["id"]
        if not re.fullmatch(r"[a-z][a-z0-9_]+", name) or name in seen:
            raise ValueError(f"invalid or duplicate witness id: {name}")
        seen.add(name)
        if not re.fullmatch(r"V2[A-Za-z0-9]+", case["module"]):
            raise ValueError(f"invalid module: {name}")
        if not (TLA / (case["module"] + ".tla")).is_file():
            raise ValueError(f"missing module: {name}")
        if case["config"] not in positive:
            raise ValueError(f"witness must use a verify-model-all positive configuration: {name}")
        if EXPECTS.search((TLA / case["config"]).read_text()):
            raise ValueError(f"a negative control cannot supply a positive witness: {name}")
        module = next((module for pattern, module in arms if fnmatch.fnmatchcase(case["config"], pattern)), None)
        if module != case["module"]:
            raise ValueError(f"configuration/module mismatch: {name}")
        if not case["predicate"].strip() or not case["purpose"].strip():
            raise ValueError(f"missing predicate or purpose: {name}")
        if "path" in case and (not case["path"] or any(not step.strip() for step in case["path"])):
            raise ValueError(f"empty guided path: {name}")
    if not seen:
        raise ValueError("empty witness manifest")
    return document


def witness_error(output, status):
    if status != 12:
        return f"TLC exit {status}; no concrete witness (0 means exhaustive absence, 124 means timeout)"
    errors = [line for line in output.splitlines() if line.startswith("Error:")]
    if errors != [f"Error: Invariant {CLAIM} is violated.",
                  "Error: The behavior up to this point is:"]:
        return "TLC did not report only the intended reachability invariant"
    if "Warning:" in output or "State 2:" not in output:
        return "a witness requires a noninitial trace and no TLC warning"
    return None


def check_inventory(document):
    covered = {case["module"] for case in document["cases"]}
    historical = document.get("historical", {})
    if set(historical) != {"V2Isolation"} or covered & historical.keys():
        raise ValueError("only the D-480 isolation model is historical; current modules require witnesses")
    if any(not reason.strip() for reason in historical.values()):
        raise ValueError("historical modules need an explicit reason")
    modules = {path.stem for path in TLA.glob("V2*.tla")}
    missing = modules - covered - historical.keys()
    extra = (covered | historical.keys()) - modules
    if missing or extra:
        raise ValueError(f"witness inventory mismatch: missing={sorted(missing)}, unknown={sorted(extra)}")


def wrapper_for(case, configuration, model):
    wrapper = f"---- MODULE Witness ----\nEXTENDS {case['module']}\n"
    if "path" in case:
        # TLC accepts one next-state conjunct. Keep the model's Init, Next,
        # state tuple and exact fairness suffix; intersect Next with the path.
        spec = re.search(r"^Spec\s*==\s*Init\s*/\\\s*\[\]\[Next\]_(vars|monVars)\b(.*?)(?=^\w[^\n]*==|^====)",
                         model, re.M | re.S)
        if spec is None:
            raise ValueError("unsupported Spec shape for a guided witness")
        steps = "\n       [] ".join(f"witnessStep = {i} -> ({step})" for i, step in enumerate(case["path"]))
        wrapper += ("VARIABLE witnessStep\nWitnessPath ==\n"
                    f"  /\\ witnessStep < {len(case['path'])}\n"
                    f"  /\\ CASE {steps}\n  /\\ witnessStep' = witnessStep + 1\n"
                    f"WitnessSpec == Init /\\ witnessStep = 0 /\\ [][Next /\\ WitnessPath]_<<{spec[1]}, witnessStep>>"
                    f"{spec[2]}\n")
        configuration, count = re.subn(r"\bSPECIFICATION\s+Spec\b", "SPECIFICATION WitnessSpec", configuration)
        if count != 1:
            raise ValueError("guided paths require exactly one original Spec declaration")
    wrapper += f"{CLAIM} == ~({case['predicate']})\n====\n"
    return wrapper, configuration + f"\nINVARIANT {CLAIM}\n"


def run_case(case, jar, scratch, output, timeout):
    name = case["id"]
    root = scratch / name
    root.mkdir()
    for source in TLA.glob("*.tla"):
        shutil.copy2(source, root / source.name)
    wrapper, configuration = wrapper_for(case, (TLA / case["config"]).read_text(),
                                         (root / (case["module"] + ".tla")).read_text())
    (root / "Witness.tla").write_text(wrapper)
    (root / "Witness.cfg").write_text(configuration)
    (output / (name + ".tla")).write_text(wrapper)
    (output / (name + ".cfg")).write_text(configuration)
    log = output / (name + ".log")
    command = ["java", "-Xmx2g", "-XX:+UseParallelGC", "-cp", str(jar), "tlc2.TLC",
               "-workers", "1", "-lncheck", "final", "-config", "Witness.cfg", "Witness.tla"]
    with log.open("w") as stream:
        try:
            result = subprocess.run(command, cwd=root, stdout=stream, stderr=subprocess.STDOUT, timeout=timeout)
            status = result.returncode
        except subprocess.TimeoutExpired:
            status = 124
    text = log.read_text()
    error = witness_error(text, status)
    return {**case, "exit": status, "error": error, "log": str(log),
            "module_sha256": digest(root / (case["module"] + ".tla")),
            "config_sha256": digest(root / "Witness.cfg"), "log_sha256": digest(log)}


def self_check():
    trace = (f"Error: Invariant {CLAIM} is violated.\n"
             "Error: The behavior up to this point is:\nState 1:\nState 2:\n")
    assert witness_error(trace, 12) is None
    for status in [0, 1, 11, 13, 124, 151]:
        assert witness_error(trace, status)
    for bad in [trace.replace(CLAIM, "Other"), trace.replace("State 2:", ""),
                trace + "Warning: incomplete\n", trace + "Error: evaluation failed\n"]:
        assert witness_error(bad, 12)
    print("reachability verdict controls passed")


def fixture_check(jar):
    model = ("---- MODULE Fixture ----\nEXTENDS Naturals\nVARIABLE x\nvars == <<x>>\n"
             "Init == x = 0\nNext == x = 0 /\\ x' = 1\n"
             "Spec == Init /\\ [][Next]_vars /\\ WF_vars(Next)\n"
             "TypeOK == x \\in 0..2\n====\n")
    # TypeOK deliberately allows 2: only the production Next excludes the
    # fabricated transition. A generator that drops Next must fail this test.
    with tempfile.TemporaryDirectory(prefix="teamagents-witness-check-") as directory:
        for name, step, predicate, wanted in [("valid", "x' = 1", "x = 1", True),
                                              ("forged", "x' = 2", "x = 2", False),
                                              ("absent", "x' = 1", "x = 2", False)]:
            root = pathlib.Path(directory) / name
            root.mkdir()
            (root / "Fixture.tla").write_text(model)
            case = {"module": "Fixture", "path": [step], "predicate": predicate}
            wrapper, cfg = wrapper_for(case, "SPECIFICATION Spec\nINVARIANT TypeOK\n", model)
            (root / "Witness.tla").write_text(wrapper)
            (root / "Witness.cfg").write_text(cfg)
            run = subprocess.run(["java", "-Xmx512m", "-XX:+UseParallelGC", "-cp", str(jar),
                                  "tlc2.TLC", "-workers", "1", "-config", "Witness.cfg", "Witness.tla"],
                                 cwd=root, capture_output=True, text=True, timeout=30)
            output = run.stdout + run.stderr
            assert run.returncode in {0, 11, 12, 13}, output
            assert (witness_error(output, run.returncode) is None) == wanted, output
    print("guided-path controls passed: valid path accepted; forged and absent paths rejected")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="require a current path inventory for every module")
    parser.add_argument("--self-check", action="store_true")
    parser.add_argument("--check-fixtures", action="store_true", help="exercise the guided-path checker with TLC")
    parser.add_argument("--case", action="append", help="run only this witness id; repeatable")
    parser.add_argument("--jar", type=pathlib.Path,
                        default=pathlib.Path.home() / ".local/share/teamagents-verify/tla2tools.jar")
    parser.add_argument("--out", type=pathlib.Path, help="new directory for replay inputs, traces and verdicts")
    parser.add_argument("--timeout", type=int, default=120, help="seconds per witness; timeout is failure")
    args = parser.parse_args()
    if args.self_check:
        self_check()
        return 0
    document = load_cases()
    check_inventory(document)
    if args.check:
        print(f"witness inventory: {len(document['cases'])} paths")
        return 0
    cases = [case for case in document["cases"] if not args.case or case["id"] in args.case]
    if args.case and set(args.case) - {case["id"] for case in cases}:
        raise ValueError("unknown witness id")
    if args.timeout <= 0:
        raise ValueError("timeout must be positive")
    jar = args.jar.resolve()
    pinned = re.search(r"^TLA_SHA256\s*:=\s*([a-f0-9]{64})$", (REPO / "Makefile").read_text(), re.M)
    if pinned is None or digest(jar) != pinned[1]:
        raise ValueError("TLC jar does not match the Makefile pin")
    if args.check_fixtures:
        fixture_check(jar)
        return 0
    if args.out:
        output = args.out.resolve()
        output.mkdir(parents=True, exist_ok=False)
    else:
        output = pathlib.Path(tempfile.mkdtemp(prefix="teamagents-witnesses-"))
    report = {"manifest_sha256": digest(MANIFEST), "jar_sha256": digest(jar), "cases": []}
    with tempfile.TemporaryDirectory(prefix="teamagents-witness-models-") as directory:
        for case in cases:
            result = run_case(case, jar, pathlib.Path(directory), output, args.timeout)
            report["cases"].append(result)
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            print(f"{case['id']}: {result['error'] or 'witness reached'}", flush=True)
    print(f"replay inputs and traces: {output}")
    return int(any(case["error"] for case in report["cases"]))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError) as error:
        print(f"witness check: {error}", file=sys.stderr)
        raise SystemExit(1)

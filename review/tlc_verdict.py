#!/usr/bin/env python3
"""Reject ambiguous TLC verdicts; a generic temporal failure needs one expected property."""
import pathlib
import re
import sys

from verification_catalogue import EXPECTS, configuration_claims


def verdict(configuration: str, output: str, status: int, negative: bool) -> str | None:
    if "Warning:" in output:
        return "TLC warnings are failures"
    if not negative:
        if status != 0 or "No error has been found" not in output:
            return "TLC did not complete successfully"
        if re.search(r"is violated|properties were violated|is equal to FALSE|^Error:", output, re.M):
            return "a success marker cannot override a reported error"
        return None
    markers = EXPECTS.findall(configuration)
    if len(markers) != 1:
        return "a negative control must name exactly one expected property"
    expected = markers[0]
    invariants = configuration_claims(configuration, "INVARIANTS")
    properties = configuration_claims(configuration, "PROPERTIES")
    if expected not in invariants | properties:
        return f"the control does not check {expected}"
    # A constant-folded FALSE invariant is reported as a configuration verdict
    # (151) by pinned TLC 2.16. Accept only that exact named diagnostic, never
    # a general parse error. These are static refutations, not explored traces.
    constant_false = f"Error: The invariant of {expected} is equal to FALSE"
    errors = [line for line in output.splitlines() if line.startswith("Error:")]
    if status == 151 and errors == [constant_false]:
        return None
    # The pinned TLC's EC.ExitStatus assigns 12 to safety and 13 to temporal
    # violations. Assumption/deadlock/start errors are not evidence.
    if status not in {12, 13}:
        return f"TLC exit {status} is not a counterexample verdict"
    named = re.search(rf"(?:Invariant|Action property) {expected} (?:is violated|is equal to FALSE)|"
                      rf"invariant of {expected} is equal to FALSE", output)
    if named:
        return None
    if status == 13 and "Temporal properties were violated" in output and properties == {expected}:
        return None
    return f"TLC did not unambiguously refute {expected}"


def self_check() -> None:
    success = "Model checking completed. No error has been found."
    assert verdict("", success, 0, False) is None
    for output, status in [(success, 1), ("Warning: inconsistent model\n" + success, 0),
                           (success + "\nError: late failure", 0), ("10 states generated", 0)]:
        assert verdict("", output, status, False)
    config = "\\* expects: Target\nINVARIANTS\n    Safe\nPROPERTIES\n    Target\n"
    assert verdict(config, "Temporal properties were violated", 13, True) is None
    assert verdict(config + "    Other\n", "Temporal properties were violated", 13, True)
    assert verdict(config.replace("Target\n", "Other\n", 1), "Temporal properties were violated", 13, True)
    assert verdict(config, "Invariant Safe is violated", 12, True)
    assert verdict(config, "Action property Target is violated", 12, True) is None
    assert verdict(config, "Action property Target is violated", 13, True) is None
    assert verdict(config, "Action property Target is violated", 1, True)
    assert verdict(config, "Warning: bad model\nAction property Target is violated", 12, True)
    assert verdict(config, "Error: The invariant of Target is equal to FALSE", 151, True) is None
    assert verdict(config, "Error: The invariant of Other is equal to FALSE", 151, True)
    assert verdict(config, "Error: The invariant of Target is equal to FALSE\nError: parse failure", 151, True)
    print("TLC verdict controls passed")


def main() -> int:
    if sys.argv[1:] == ["--self-check"]:
        self_check()
        return 0
    if len(sys.argv) != 4 or sys.argv[1] not in {"positive", "negative"}:
        print("usage: tlc_verdict.py {positive|negative} CONFIG EXIT_STATUS", file=sys.stderr)
        return 2
    output = sys.stdin.read()
    error = verdict(pathlib.Path(sys.argv[2]).read_text(), output, int(sys.argv[3]), sys.argv[1] == "negative")
    for line in output.splitlines():
        if re.search(r"No error|violation|violated|is equal to FALSE|states generated|Warning:", line):
            print(line)
    if error:
        print(f"{sys.argv[2]}: {error}\n" + "\n".join(output.splitlines()[-12:]), file=sys.stderr)
    return int(error is not None)


if __name__ == "__main__":
    raise SystemExit(main())

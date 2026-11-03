#!/usr/bin/env python3
"""Check original-to-canonical successors and fair-action enabling on a bounded graph.

Generate every original transition first, then extend allocation maps and compute
canonical state. The reduced relation is checked as a property, never a guard.
This is bounded relational evidence, not a general machine-checked theorem.
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
sys.path.insert(0, str(REPO / "review"))
from tlc_verdict import verdict  # noqa: E402

WRAPPER = r'''----------------------- MODULE AllocationRelation -----------------------
EXTENDS V2Control
VARIABLES rm, am, ci, cg, cr, ca, co
cvars == <<ci, cg, cr, ca, co, approvals, dead, nearDeadline, warns>>
allvars == <<vars, rm, am, ci, cg, cr, ca, co>>
Reduced == INSTANCE ReducedControl WITH inst <- ci, goal <- cg, requests <- cr,
    attempts <- ca, ops <- co
ASSUME IsFiniteSet(ReqIds) /\ IsFiniteSet(AttIds) /\ nil \notin ReqIds \cup AttIds
ASSUME {BadAdmission, BadActiveRequest, EffectBeforeDispatch, KeepClosedReservation,
    OverReserve, SelectIncomplete, StaleExecutor, DoubleEffect, RewriteTerminalOp,
    RewriteTerminalGoal, CrossEpochReceipt, EffectUnauthorized, EffectOnTerminated,
    EffectAfterCancel, PreparedEffect} = {FALSE}
RUsed(rs) == {r \in ReqIds : rs[r].status # "none"}
AUsed(aset) == {a \in AttIds : aset[a].status # "none"}
Image(m, used) == {m[x] : x \in used}
Ref(x, m) == IF x = nil THEN nil ELSE m[x]
Inverse(x, m) == CHOOSE y \in DOMAIN m : m[y] = x
BlankR == [status |-> "none", instance |-> "", epoch |-> 0, est |-> 1,
           selected |-> FALSE, selectedAttempt |-> nil, result |-> "reply"]
BlankA == [req |-> nil, status |-> "none"]
MapI(iset, m) == [i \in Instances |-> [iset[i] EXCEPT !.activeReq = Ref(@, m)]]
MapG(g, m) == [g EXCEPT !.reserved = {<<m[e[1]], e[2]>> : e \in @}]
MapR(rs, m, n) == [r \in ReqIds |-> IF r \in Image(m, RUsed(rs))
    THEN [rs[Inverse(r, m)] EXCEPT !.selectedAttempt = Ref(@, n)] ELSE BlankR]
MapA(aset, m, n) == [a \in AttIds |-> IF a \in Image(n, AUsed(aset))
    THEN [aset[Inverse(a, n)] EXCEPT !.req = Ref(@, m)] ELSE BlankA]
MapO(oset, m) == [o \in Ops |-> [oset[o] EXCEPT !.request = Ref(@, m)]]
Extend(m, used, nextUsed, ids) ==
    IF nextUsed \ used = {} THEN m
    ELSE LET x == CHOOSE x \in nextUsed \ used : TRUE
             y == CHOOSE y \in ids \ Image(m, used) : TRUE
         IN [m EXCEPT ![x] = y]
RelInit ==
    /\ Init
    /\ rm = [r \in ReqIds |-> nil] /\ am = [a \in AttIds |-> nil]
    /\ ci = MapI(inst, rm) /\ cg = MapG(goal, rm)
    /\ cr = MapR(requests, rm, am) /\ ca = MapA(attempts, rm, am) /\ co = MapO(ops, rm)
RelNext ==
    /\ Next
    /\ rm' = Extend(rm, RUsed(requests), RUsed(requests'), ReqIds)
    /\ am' = Extend(am, AUsed(attempts), AUsed(attempts'), AttIds)
    /\ ci' = MapI(inst', rm') /\ cg' = MapG(goal', rm')
    /\ cr' = MapR(requests', rm', am') /\ ca' = MapA(attempts', rm', am')
    /\ co' = MapO(ops', rm')
RelSpec == RelInit /\ [][RelNext]_allvars
IsAllocationMap(m, used, ids) ==
    /\ m \in [ids -> ids \cup {nil}]
    /\ \A x \in ids : (m[x] = nil) <=> (x \notin used)
    /\ \A x, y \in used : m[x] = m[y] => x = y
MapDomains == IsAllocationMap(rm, RUsed(requests), ReqIds)
           /\ IsAllocationMap(am, AUsed(attempts), AttIds)
PristineUnused ==
    /\ \A r \in ReqIds \ RUsed(requests) : requests[r] = BlankR
    /\ \A a \in AttIds \ AUsed(attempts) : attempts[a] = BlankA
CanonicalTypeOK == Reduced!TypeOK
InitialMatch == Init => Reduced!Init
EnabledSame == \A i \in Instances :
    /\ (ENABLED <<Recover(i)>>_vars) <=> (ENABLED <<Reduced!Recover(i)>>_cvars)
    /\ (ENABLED <<ApplyQueued(i)>>_vars) <=> (ENABLED <<Reduced!ApplyQueued(i)>>_cvars)
    /\ (ENABLED <<TurnStep(i)>>_vars) <=> (ENABLED <<Reduced!TurnStep(i)>>_cvars)
ForwardStep == [][Reduced!Next]_allvars
FairOccurrences == [][\A i \in Instances :
    /\ (<<Recover(i)>>_vars <=> <<Reduced!Recover(i)>>_cvars)
    /\ (<<ApplyQueued(i)>>_vars <=> <<Reduced!ApplyQueued(i)>>_cvars)
    /\ (<<TurnStep(i)>>_vars <=> <<Reduced!TurnStep(i)>>_cvars)]_allvars
MonotoneSlots == [][RUsed(requests) \subseteq RUsed(requests')
                    /\ AUsed(attempts) \subseteq AUsed(attempts')]_allvars
=============================================================================
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", default="MC_control_attempts.cfg")
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--fault", choices=["none", "selected-reference", "response"], default="none")
    parser.add_argument("--timeout", type=int, default=1200)
    args = parser.parse_args()
    if not re.fullmatch(r"MC(?:_control_\w+|_wide)?\.cfg", args.config):
        parser.error("choose a control-model configuration")
    source = (REPO / "verification/tla/V2Control.tla").read_text()
    reduced = source.replace("MODULE V2Control", "MODULE ReducedControl", 1).replace(
        "\\E r \\in FreeReqs :", "\\E r \\in (IF FreeReqs = {} THEN {} ELSE {CHOOSE x \\in FreeReqs : TRUE}) :").replace(
        "\\E a \\in FreeAtts", "\\E a \\in (IF FreeAtts = {} THEN {} ELSE {CHOOSE x \\in FreeAtts : TRUE})")
    wrapper = WRAPPER
    if args.fault == "selected-reference":
        wrapper = wrapper.replace("!.selectedAttempt = Ref(@, n)", "!.selectedAttempt = @")
    if args.fault == "response":
        reduced = reduced.replace('kind \\in Result :', 'kind \\in {"reply"} :')
    config = (REPO / "verification/tla" / args.config).read_text().split("INVARIANTS")[0]
    config = config.replace("SPECIFICATION Spec", "SPECIFICATION RelSpec")
    if args.fault == "none":
        config += "INVARIANTS\n    TypeOK\n    MapDomains\n    PristineUnused\n    CanonicalTypeOK\n    InitialMatch\n    EnabledSame\nPROPERTIES\n    ForwardStep\n    FairOccurrences\n    MonotoneSlots\n"
    else:
        config = "\\* expects: ForwardStep\n" + config + "PROPERTIES\n    ForwardStep\n"
    jar = pathlib.Path.home() / ".local/share/teamagents-verify/tla2tools.jar"
    pin = re.search(r"^TLA_SHA256 := (\w+)$", (REPO / "Makefile").read_text(), re.M)
    if pin is None or hashlib.sha256(jar.read_bytes()).hexdigest() != pin[1]:
        raise ValueError("TLC jar does not match Makefile")
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    for name, text in {"V2Control.tla": source, "ReducedControl.tla": reduced,
                       "AllocationRelation.tla": wrapper, "Relation.cfg": config}.items():
        (out / name).write_text(text)
    command = ["java", "-Xmx3g", "-XX:+UseParallelGC", "-cp", str(jar), "tlc2.TLC",
               "-lncheck", "final", "-fp", "64", "-workers", "2", "-metadir", str(out / "states"),
               "-config", "Relation.cfg", "AllocationRelation.tla"]
    start = time.monotonic()
    with (out / "tlc.log").open("w") as log:
        try:
            status = subprocess.run(command, cwd=out, stdout=log, stderr=subprocess.STDOUT, timeout=args.timeout).returncode
        except subprocess.TimeoutExpired:
            status = 124
    output = (out / "tlc.log").read_text()
    error = verdict(config, output, status, args.fault != "none")
    result = {"config": args.config, "fault": args.fault, "exit": status, "error": error,
              "seconds": time.monotonic() - start, "command": command,
              "sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir() if p.is_file()}}
    (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    shutil.rmtree(out / "states", ignore_errors=True)
    print(json.dumps(result, indent=2))
    return int(error is not None)


if __name__ == "__main__":
    raise SystemExit(main())

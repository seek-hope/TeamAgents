------------------------------ MODULE V2Codemode ---------------------------
(***************************************************************************)
(* Codemode: what a script may do and what may reach the model's context    *)
(* (D-374, D-376).                                                           *)
(*                                                                           *)
(* A script may call any bound MCP tool — that is the point of the feature.  *)
(* Three claims are checked here:                                            *)
(*                                                                           *)
(*  1. a *nested* call's payload never enters the model's context: only what *)
(*     the script emits with `text()` (and its return value) does            *)
(*     (`OnlyScriptOutputEnters`, with `ScriptsAlwaysEmit` as the            *)
(*     non-vacuity companion);                                                *)
(*  2. a call the user's `pre_tool` hook vetoes never executes               *)
(*     (`VetoedNeverCalled`) — a script is not a way around the hook;        *)
(*  3. two runs of the same script are independent, which is what makes both *)
(*     claims hold under every interleaving of runs.                         *)
(*                                                                           *)
(* The code that keeps (1) true is the host bridge in                        *)
(* `engine/src/codemode.rs` — the sandbox's only capabilities are the host   *)
(* functions it registers — together with the toolkit advertising one        *)
(* `codemode` tool instead of the bound MCP tools themselves. (2) is the     *)
(* same bridge asking the user's hook once per nested call, exactly as the   *)
(* direct path does.                                                         *)
(*                                                                           *)
(* The two negative controls state the plausible bugs: letting a run push    *)
(* its nested payloads into the context, and ignoring the veto. Each must    *)
(* make TLC refute the matching property — a control that verifies would     *)
(* mean the model says nothing.                                              *)
(*                                                                           *)
(* Ceiling: safety only, and the *contents* of an item are abstract (a tag,  *)
(* not the payload). The JSON round trip, the identifier normalization, the  *)
(* `store`/`load` map, the output budget and the deadline are code facts     *)
(* pinned by the module's and the toolkit's tests, not modelled here.        *)
(*                                                                           *)
(* Code anchors: `engine/src/codemode.rs` (`run`, the prelude's `tools`      *)
(* proxy, `text`, the veto branch), `engine/src/tools.rs` (`mcp_schemas`,    *)
(* `call`, the `ToolWiring` veto), pinned by                                  *)
(* `engine/tests/v2_mcp.rs::codemode_runs_mcp_tools_and_only_its_output_reaches_the_context` *)
(* and `::a_pre_tool_hook_vetoes_a_nested_mcp_call_inside_codemode`.         *)
(***************************************************************************)
EXTENDS FiniteSets

CONSTANTS Runs,      \* codemode runs, e.g. {"r1","r2"}
          Tools,     \* the bound MCP tools, e.g. {"t1","t2"}
          Vetoed,    \* tools the user's pre_tool hook denies, e.g. {"t1"}
          IgnoreVeto, \* negative control: the veto is not applied to nested calls
          LeakNested \* negative control: a run pushes its nested payloads into the context

ToolNames == Tools \union {"none"}
Items == [kind : {"script", "nested"}, run : Runs, tool : ToolNames]
Calls == [run : Runs, tool : Tools]

VARIABLES
  done,    \* runs that happened
  called,  \* nested calls that executed
  context  \* what the model's context holds

vars == <<done, called, context>>

ScriptItem(r) == [kind |-> "script", run |-> r, tool |-> "none"]
NestedItem(r, t) == [kind |-> "nested", run |-> r, tool |-> t]

\* ------------------------------------------------------------------ actions --
\* One run attempts some subset of the tools. The guard *is* the rule: a vetoed
\* attempt does not execute (unless the negative control ignores the veto), the
\* script item always enters the context, and a nested payload only under the
\* leak control.
Run(r) ==
  \E attempted \in SUBSET Tools :
    LET executed == attempted \ (IF IgnoreVeto THEN {} ELSE Vetoed)
    IN
    /\ r \notin done
    /\ done' = done \union {r}
    /\ called' = called \union {[run |-> r, tool |-> t] : t \in executed}
    /\ context' = {ScriptItem(r)}
                  \union context
                  \union (IF LeakNested THEN {NestedItem(r, t) : t \in executed} ELSE {})

Stutter == UNCHANGED vars

Next == ( \E r \in Runs : Run(r) ) \/ Stutter

Init == done = {} /\ called = {} /\ context = {}

Spec == Init /\ [][Next]_vars

\* -------------------------------------------------------------- invariants --
TypeOK ==
  /\ done \subseteq Runs
  /\ called \subseteq Calls
  /\ context \subseteq Items

\* D-374: a nested call's payload is never in the model's context.
OnlyScriptOutputEnters ==
  \A item \in context : item.kind = "script"

\* Non-vacuity: a run that happened did emit its own output, so the model is
\* not satisfied by an empty context.
ScriptsAlwaysEmit ==
  \A r \in done : \E item \in context : item.kind = "script" /\ item.run = r

\* D-376: a nested call the user's hook vetoed never executes.
VetoedNeverCalled ==
  \A call \in called : call.tool \notin Vetoed

=============================================================================

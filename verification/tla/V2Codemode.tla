------------------------------ MODULE V2Codemode ---------------------------
(***************************************************************************)
(* Codemode: what a script may put into the model's context (D-374).        *)
(*                                                                         *)
(* A script may call any bound MCP tool — that is the point of the feature.  *)
(* The claim is that a *nested* call's payload never enters the model's      *)
(* context: only what the script emits with `text()` (and its return value)  *)
(* does. The code that keeps that true is the host bridge in                 *)
(* `engine/src/codemode.rs` (the sandbox's only capabilities are the host    *)
(* functions it registers) together with the toolkit advertising one         *)
(* `codemode` tool instead of the bound MCP tools themselves.                *)
(*                                                                           *)
(* The model records each run, the nested calls it made, and the context     *)
(* items it produced. `OnlyScriptOutputEnters` says every context item is a  *)
(* script output; `ScriptsAlwaysEmit` is the non-vacuity companion (a run    *)
(* that happened has an output item), so the property cannot be satisfied by *)
(* never running anything. The negative control lets a run push its nested   *)
(* payloads into the context as well, and TLC must refute the first          *)
(* property there — a control that verifies would mean the model says        *)
(* nothing.                                                                  *)
(*                                                                           *)
(* Ceiling: safety only, and the *contents* of an item are abstract (a tag,  *)
(* not the payload). The JSON round trip, the identifier normalization, the  *)
(* `store`/`load` map and the deadline are code facts pinned by the module's *)
(* and the toolkit's tests, not modelled here.                               *)
(*                                                                           *)
(* Code anchors: `engine/src/codemode.rs` (`run`, the prelude's `tools`      *)
(* proxy and `text`), `engine/src/tools.rs` (`mcp_schemas`, `call`,          *)
(* `is_mcp_tool`), pinned by                                            *)
(* `engine/tests/v2_mcp.rs::codemode_runs_mcp_tools_and_only_its_output_reaches_the_context`. *)
(***************************************************************************)
EXTENDS FiniteSets

CONSTANTS Runs,      \* codemode runs, e.g. {"r1","r2"}
          Tools,     \* the bound MCP tools, e.g. {"t1","t2"}
          LeakNested \* negative control: a run pushes its nested payloads into the context

ToolNames == Tools \union {"none"}
Items == [kind : {"script", "nested"}, run : Runs, tool : ToolNames]
Calls == [run : Runs, tool : Tools]

VARIABLES
  done,    \* runs that happened
  called,  \* nested calls that happened
  context  \* what the model's context holds

vars == <<done, called, context>>

ScriptItem(r) == [kind |-> "script", run |-> r, tool |-> "none"]
NestedItem(r, t) == [kind |-> "nested", run |-> r, tool |-> t]

\* ------------------------------------------------------------------ actions --
\* One run calls some subset of the tools and emits its own output. The guard
\* *is* the rule: the script item always enters the context, the nested
\* payloads only under the negative control.
Run(r) ==
  \E tools \in SUBSET Tools :
    /\ r \notin done
    /\ done' = done \union {r}
    /\ called' = called \union {[run |-> r, tool |-> t] : t \in tools}
    /\ context' = {ScriptItem(r)}
                  \union context
                  \union (IF LeakNested THEN {NestedItem(r, t) : t \in tools} ELSE {})

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

=============================================================================

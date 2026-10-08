------------------------------ MODULE V2Tui ------------------------------
(* D-445: minimal client interaction, not the engine's cancellation semantics.
   Key events observe one UI state. Arrival of an approval never steals focus.
   The last record remembers the state at dispatch, so later polling cannot
   change whether that dispatch was authorized by an explicit user key.
   Anchors: tui/src/v2app.rs handle_key, composer_key, approval_key;
   tui/tests/minimal.rs and tui/scripts/pty_v2_smoke.py bridge model to client.
   No claim about Unicode, rendering, transport delivery or stopping tools. *)
EXTENDS Naturals
CONSTANTS WrongTarget, AutoApprove, QuitCancels
VARIABLES
  focus,
  pending,
  connected,
  leader,
  open,
  last
vars == <<focus, pending, connected, leader, open, last>>
Keys == {"enter", "a", "d", "review", "escape", "interrupt", "exit", "tab"}
Effects == {"none", "send", "approve", "deny", "interrupt", "quit"}
Effect(k) ==
  CASE k = "exit" -> IF QuitCancels THEN "interrupt" ELSE "quit"
    [] k = "interrupt" /\ connected /\ leader -> "interrupt"
    [] k = "enter" /\ focus = "composer" /\ connected /\ leader -> "send"
    [] k = "a" /\ pending /\ (focus = "approvals" \/ AutoApprove) -> "approve"
    [] k = "d" /\ pending /\ focus = "approvals" -> "deny"
    [] OTHER -> "none"
Key(k) ==
  /\ open
  /\ last' = [key |-> k, focus |-> focus, pending |-> pending,
               connected |-> connected, leader |-> leader, effect |-> Effect(k),
               target |-> IF Effect(k) \in {"send", "interrupt"}
                         THEN IF WrongTarget THEN "worker" ELSE "leader" ELSE "none"]
  /\ focus' = IF k = "review" /\ pending THEN "approvals"
               ELSE IF k = "escape" THEN "composer" ELSE focus
  /\ open' = IF k = "exit" THEN FALSE ELSE open
  /\ UNCHANGED <<pending, connected, leader>>
Poll(p, c, l) ==
  /\ open
  /\ pending' = p
  /\ connected' = c
  /\ leader' = l
  /\ focus' = IF ~p THEN "composer" ELSE focus
  /\ UNCHANGED <<open, last>>
Init ==
  /\ focus = "composer" /\ pending = FALSE /\ connected = TRUE /\ leader = TRUE /\ open = TRUE
  /\ last = [key |-> "tab", focus |-> "composer", pending |-> FALSE,
              connected |-> TRUE, leader |-> TRUE, effect |-> "none", target |-> "none"]
Next == (\E k \in Keys : Key(k)) \/ (\E p,c,l \in BOOLEAN : Poll(p,c,l)) \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* ------------------------------------------------------------- invariants --
TypeOK ==
  /\ focus \in {"composer", "approvals"}
  /\ pending \in BOOLEAN /\ connected \in BOOLEAN /\ leader \in BOOLEAN /\ open \in BOOLEAN
  /\ last \in [key: Keys, focus: {"composer", "approvals"}, pending: BOOLEAN,
                 connected: BOOLEAN, leader: BOOLEAN, effect: Effects, target: {"none", "leader", "worker"}]
LeaderOnly == last.effect \in {"send", "interrupt"} => last.target = "leader"
ExplicitDecision == last.effect \in {"approve", "deny"} =>
  last.focus = "approvals" /\ last.pending /\ last.key \in {"a", "d"}
ExitOnlyDetaches == last.key = "exit" => last.effect = "quit"
=============================================================================

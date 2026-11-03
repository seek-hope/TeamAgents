----------------------------- MODULE V2SkillAuthority ------------------------
EXTENDS Naturals, FiniteSets
(* skill_tool/member_executor: the user registry and binding admit reads. *)
(* Skill content is data, not a grant. Canonical path/regular-file checks *)
(* are Boolean premises, not a proof against host filesystem races.       *)
CONSTANT Bug
VARIABLES
  s
vars == <<s>>
Init ==
  s = [bound |-> FALSE, inside |-> FALSE, regular |-> FALSE, loaded |-> FALSE,
       requestedShell |-> FALSE, userGrant |-> FALSE, shell |-> FALSE,
       loadedSafely |-> FALSE, loadedBound |-> FALSE, effect |-> FALSE]
Bind == /\ ~s.bound /\ s' = [s EXCEPT !.bound = TRUE]
Resource(inside, regular) ==
  /\ ~s.loaded /\ inside \in BOOLEAN /\ regular \in BOOLEAN
  /\ s' = [s EXCEPT !.inside = inside, !.regular = regular]
Load ==
  /\ ~s.loaded /\ (s.bound \/ Bug = "unbound")
  /\ (s.inside /\ s.regular) \/ Bug = "escape"
  /\ s' = [s EXCEPT !.loaded = TRUE, !.loadedSafely = s.inside /\ s.regular,
                   !.loadedBound = s.bound, !.requestedShell = TRUE,
                   !.shell = IF Bug = "content-grants" THEN TRUE ELSE @]
UserGrant == /\ ~s.userGrant /\ s' = [s EXCEPT !.userGrant = TRUE, !.shell = TRUE]
Execute == /\ s.shell /\ ~s.effect /\ s' = [s EXCEPT !.effect = TRUE]
Next == Bind \/ (\E i, r \in BOOLEAN : Resource(i, r)) \/ Load \/ UserGrant \/ Execute \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK == s.bound \in BOOLEAN /\ s.loaded \in BOOLEAN /\ s.shell \in BOOLEAN
SkillReadNeedsBinding == s.loaded => s.loadedBound
SkillReadStaysInRegistry == s.loaded => s.loadedSafely
SkillContentCannotGrant == s.shell \/ s.effect => s.userGrant
=============================================================================

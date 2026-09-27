------------------------------- MODULE V2Grants -------------------------------
(***************************************************************************)
(* Authority (plan §5.1/§6.1, A03/A04; decisions D-58/D-59): where a           *)
(* capability comes from, how it narrows, what revocation cascades to, and     *)
(* what the model-visible tool surface may offer.                             *)
(*                                                                          *)
(* Code anchors: core/src/v2/control.rs — scope_covers ("session" covers every *)
(* request below it; otherwise the scope is exact, and a hierarchical "a/b"     *)
(* scope is the "/"-prefix case of the same rule), active_grant (live grants    *)
(* only, the most specific wins), issue_grant (the user is the root of          *)
(* authority and the system identity may not issue; an instance must hold a     *)
(* live covering grant, where manage covers message and delegate below it; a    *)
(* parent must cover the child's action and scope), revoke_grant with           *)
(* revoke_grant_tree (the cascade follows parent_grant_id and only the user or  *)
(* the issuing subject may revoke) plus bump_grant_revision, capability_gap and *)
(* dispatch_operation (the operation's stamped revision must still be current   *)
(* and the covering grant is re-read at the linearization point, A04), and      *)
(* spawn_instance (a spawner holding manage@session derives                     *)
(* delegate@instance:<child>; the child itself gets no shell grant, §5.1);      *)
(* engine/src/v2/driver.rs — bootstrap (a session's default grants: the leader  *)
(* holds shell@workspace plus manage/delegate/message@session, D-58) and        *)
(* team_kernel (the offered tool surface follows the instance's grants, so a    *)
(* tool the instance cannot use is not offered).                                *)
(*                                                                          *)
(* The helpers are parameterised by the grant table so an action can talk about *)
(* the *next* state's surface (TLA+ cannot prime an operator application).      *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS Instances,   \* e.g. {"leader", "child"}
          Actions,     \* e.g. {"shell", "manage", "delegate", "message"}
          GrantIds,    \* grant slots, e.g. {"g1", ..., "g6"}
          BaseTools    \* tools that need no grant (the instance's bindings)

\* The scope vocabulary the code uses: the shared workspace, the session, and one
\* scope per instance ("instance:<id>", the scope a spawn derivation targets).
Scopes == {"session", "workspace"} \union {"instance:" \o i : i \in Instances}

\* scope_covers: "session" covers every request below it, otherwise the scope is
\* exact.
Covers == {<<s, t>> \in Scopes \X Scopes : s = "session" \/ s = t}

\* The session's default authority (the bootstrap, D-58 plus create_instance's
\* workspace grant): shell for the leader's shared workspace and the team
\* authority it manages with. A child spawned by the leader holds none of it.
BootstrapGrants ==
  [ g1 |-> [subject |-> "leader", action |-> "shell",    scope |-> "workspace"],
    g2 |-> [subject |-> "leader", action |-> "manage",   scope |-> "session"],
    g3 |-> [subject |-> "leader", action |-> "delegate", scope |-> "session"],
    g4 |-> [subject |-> "leader", action |-> "message",  scope |-> "session"] ]

\* The same table as a set, so the model can quantify over it (TLC cannot
\* enumerate a function value's range).
BootstrapSet == {BootstrapGrants.g1, BootstrapGrants.g2, BootstrapGrants.g3, BootstrapGrants.g4}

\* The operations the model registers: one the leader can be authorized for, and
\* one a spawned child can never be — a child holds no shell grant (§5.1).
Ops ==
  { [owner |-> "leader", action |-> "delegate", scope |-> "instance:child"],
    [owner |-> "child",  action |-> "shell",    scope |-> "workspace"] }

ASSUME {"g1", "g2", "g3", "g4"} \subseteq GrantIds
ASSUME {"leader", "child"} \subseteq Instances
ASSUME \A o \in Ops : o.owner \in Instances /\ o.action \in Actions /\ o.scope \in Scopes

\* The tool a grant makes usable.
ToolOf(a) == CASE a = "manage"   -> "spawn"
                [] a = "message"  -> "send"
                [] a = "delegate" -> "delegate"
                [] a = "shell"    -> "shell"
                [] OTHER          -> a

\* shell is a workspace capability: offering or dispatching it needs a grant
\* covering the workspace resource. The collaboration tools follow the
\* instance's own grant of that action (a stale schema never authorizes, so the
\* dispatch re-check stays the real gate).
WorkspaceTool(a) == a = "shell"

Grant == [subject : Instances \union {"none"},
          action  : Actions \union {"none"},
          scope   : Scopes,
          parent  : GrantIds \union {"none"},
          issuer  : Instances \union {"user"},
          live    : BOOLEAN]

VARIABLES
  grants,      \* grant id -> grant (subject "none" means the slot is unused)
  revision,    \* the session grant revision, bumped by every issue and revoke
  offered,     \* instance -> the tools its model is shown
  ops,         \* operation -> [stamp, effect]
  revokedIds,  \* monitor: grant ids that were ever revoked
  staleOps     \* monitor: operations that were prepared at an outdated revision

vars == <<grants, revision, offered, ops>>
monVars == <<vars, revokedIds, staleOps>>

UsedFor(gr, g) == gr[g].subject # "none"
LiveFor(gr, g) == gr[g].subject # "none" /\ gr[g].live
ChildrenFor(gr, g) == {c \in GrantIds : UsedFor(gr, c) /\ gr[c].parent = g}

RECURSIVE SubtreeFor(_, _), AncestryFor(_, _)
SubtreeFor(gr, g) == {g} \union UNION {SubtreeFor(gr, c) : c \in ChildrenFor(gr, g)}
AncestryFor(gr, g) == {g} \union (IF gr[g].parent = "none" THEN {} ELSE AncestryFor(gr, gr[g].parent))

\* A live grant of `action` for `subject` whose scope covers `resource`.
Covers2For(gr, subject, action, resource) ==
  \E g \in GrantIds : /\ LiveFor(gr, g)
                     /\ gr[g].subject = subject
                     /\ gr[g].action = action
                     /\ <<gr[g].scope, resource>> \in Covers

\* Any live grant of that action for the subject, whatever its scope.
HoldsFor(gr, subject, action) ==
  \E g \in GrantIds : LiveFor(gr, g) /\ gr[g].subject = subject /\ gr[g].action = action

\* The surface an instance is entitled to, given a grant table.
EntitledIn(gr, i) ==
  BaseTools \union
  { ToolOf(a) : a \in { b \in Actions :
                          \/ WorkspaceTool(b) /\ Covers2For(gr, i, b, "workspace")
                          \/ ~WorkspaceTool(b) /\ HoldsFor(gr, i, b) } }

Live(g) == LiveFor(grants, g)
Used(g) == UsedFor(grants, g)
Children(g) == ChildrenFor(grants, g)
Covers2(subject, action, resource) == Covers2For(grants, subject, action, resource)
Holds(subject, action) == HoldsFor(grants, subject, action)
Entitled(i) == EntitledIn(grants, i)
Subtree(g) == SubtreeFor(grants, g)
Ancestry(g) == AncestryFor(grants, g)

\* ------------------------------------------------------------------- actions --
Init ==
  /\ grants = [ g \in GrantIds |->
                  IF g \in DOMAIN BootstrapGrants
                  THEN [subject |-> BootstrapGrants[g].subject, action |-> BootstrapGrants[g].action,
                        scope |-> BootstrapGrants[g].scope, parent |-> "none",
                        issuer |-> "user", live |-> TRUE]
                  ELSE [subject |-> "none", action |-> "none", scope |-> CHOOSE s \in Scopes : TRUE,
                        parent |-> "none", issuer |-> "user", live |-> FALSE] ]
  /\ revision = 0
  /\ offered = [i \in Instances |-> EntitledIn(grants, i)]
  /\ ops = [o \in Ops |-> [stamp |-> 0, effects |-> 0]]
  /\ revokedIds = {}
  /\ staleOps = {}

\* The user issues any grant: the root of authority.
UserIssues(g, subject, action, scope) ==
  /\ ~UsedFor(grants, g) /\ subject \in Instances /\ action \in Actions /\ scope \in Scopes
  /\ grants' = [grants EXCEPT ![g] = [subject |-> subject, action |-> action, scope |-> scope,
                                     parent |-> "none", issuer |-> "user", live |-> TRUE]]
  /\ revision' = revision + 1
  /\ offered' = [i \in Instances |-> EntitledIn(grants', i)]
  /\ UNCHANGED <<ops, revokedIds, staleOps>>

\* An instance narrows what it holds: manage covers message/delegate below it,
\* and everything else narrowly repeats the issuer's own covering grant.
InstanceMints(g, parent, action, scope) ==
  /\ ~UsedFor(grants, g) /\ LiveFor(grants, parent)
  /\ grants[parent].subject \in Instances
  /\ \/ grants[parent].action = action
     \/ grants[parent].action = "manage" /\ action \in {"message", "delegate"}
  /\ <<grants[parent].scope, scope>> \in Covers
  /\ grants' = [grants EXCEPT ![g] = [subject |-> grants[parent].subject, action |-> action,
                                     scope |-> scope, parent |-> parent,
                                     issuer |-> grants[parent].subject, live |-> TRUE]]
  /\ revision' = revision + 1
  /\ offered' = [i \in Instances |-> EntitledIn(grants', i)]
  /\ UNCHANGED <<ops, revokedIds, staleOps>>

\* Spawning (spawn_instance): the spawner must hold manage@session and derives
\* delegate over the child it created, parented to that manage grant — so
\* revoking the manage grant takes the derived grant with it.
Spawn(g, child) ==
  /\ ~UsedFor(grants, g) /\ child \in Instances
  /\ \E m \in GrantIds : /\ LiveFor(grants, m) /\ grants[m].subject = "leader"
                        /\ grants[m].action = "manage" /\ grants[m].scope = "session"
                        /\ grants' = [grants EXCEPT ![g] =
                              [subject |-> "leader", action |-> "delegate",
                               scope |-> "instance:" \o child, parent |-> m,
                               issuer |-> "leader", live |-> TRUE]]
  /\ revision' = revision + 1
  /\ offered' = [i \in Instances |-> EntitledIn(grants', i)]
  /\ UNCHANGED <<ops, revokedIds, staleOps>>

\* The user or the issuing subject revokes; the whole subtree goes with it and
\* the revision moves, so every operation stamped before is stale (A03/A04).
Revoke(g) ==
  /\ LiveFor(grants, g)
  /\ \E actor \in Instances \union {"user"} : actor = "user" \/ actor = grants[g].issuer
  /\ LET gone == SubtreeFor(grants, g)
     IN grants' = [h \in GrantIds |->
                     IF h \in gone THEN [grants[h] EXCEPT !.live = FALSE] ELSE grants[h]]
  /\ revision' = revision + 1
  /\ offered' = [i \in Instances |-> EntitledIn(grants', i)]
  /\ revokedIds' = revokedIds \union SubtreeFor(grants, g)
  /\ UNCHANGED <<ops, staleOps>>

\* A request registers its operations with the revision of the moment. An
\* operation is registered once: it is neither re-stamped after it took effect nor
\* after a refusal (a new attempt is a new operation, as in the code).
Prepare(o) ==
  /\ ops[o].effects = 0 /\ o \notin staleOps
  /\ ops' = [ops EXCEPT ![o] = [ops[o] EXCEPT !.stamp = revision]]
  /\ UNCHANGED <<grants, revision, offered, revokedIds, staleOps>>

\* Dispatch (the linearization point): the stamp must still be current and a live
\* covering grant must exist right now.
Dispatch(o) ==
  /\ ops[o].effects = 0
  /\ ops[o].stamp = revision
  /\ Covers2For(grants, o.owner, o.action, o.scope)
  /\ ops' = [ops EXCEPT ![o] = [ops[o] EXCEPT !.effects = 1]]
  /\ UNCHANGED <<grants, revision, offered, revokedIds, staleOps>>

\* A dispatch attempt at an outdated revision: refused, recorded, no effect.
RefusedDispatch(o) ==
  /\ ops[o].effects = 0
  /\ ops[o].stamp # revision
  /\ staleOps' = staleOps \union {o}
  /\ UNCHANGED <<grants, revision, offered, ops, revokedIds>>

Next ==
  \/ \E g \in GrantIds, subject \in Instances, action \in Actions, scope \in Scopes :
       UserIssues(g, subject, action, scope)
  \/ \E g, parent \in GrantIds, action \in Actions, scope \in Scopes : InstanceMints(g, parent, action, scope)
  \/ \E g \in GrantIds, child \in Instances : Spawn(g, child)
  \/ \E g \in GrantIds : Revoke(g)
  \/ \E o \in Ops : Prepare(o)
  \/ \E o \in Ops : Dispatch(o)
  \/ \E o \in Ops : RefusedDispatch(o)
  \/ UNCHANGED monVars   \* the session idles: the model stays non-terminating

Spec == Init /\ [][Next]_monVars

\* ------------------------------------------------------------------ invariants --
TypeOKGrants == grants \in [GrantIds -> Grant]
TypeOKRevision == revision \in Nat
TypeOKOffered ==
  offered \in [Instances -> SUBSET (BaseTools \union {ToolOf(a) : a \in Actions})]
TypeOKOps == ops \in [Ops -> [stamp : Nat, effects : 0..2]]
TypeOK == TypeOKGrants /\ TypeOKRevision /\ TypeOKOffered /\ TypeOKOps
\* ------------------------------------------------------------------ properties --

\* Every effect is produced by a dispatch that held the authority at that very
\* step: a live covering grant and a still-current stamped revision (A03/A04).
\* The code anchor is the guard of dispatch_operation, which re-reads both.
AuthorizedEffectsOnly ==
  [][ \A o \in Ops : (ops'[o].effects > ops[o].effects) =>
        (ops[o].stamp = revision /\ Covers2For(grants, o.owner, o.action, o.scope)) ]_monVars

\* An operation has at most one effect (recovery never replays it, A08/A10).
EffectAtMostOnce ==
  \A o \in Ops : ops[o].effects <= 1

\* An operation refused at an outdated revision can never take effect afterwards:
\* the revision only grows, so its stamp stays behind (A04).
OnceStaleNeverExecutes ==
  \A o \in Ops : o \in staleOps => ops[o].effects = 0

\* A derived grant never exceeds its parent (issue_grant's parent check).
ChildGrantsAreCoveredByTheirParent ==
  \A g \in GrantIds : Used(g) /\ grants[g].parent # "none" =>
    \E p \in GrantIds : /\ grants[g].parent = p /\ Used(p)
                       /\ \/ grants[p].action = grants[g].action
                          \/ grants[p].action = "manage" /\ grants[g].action \in {"message", "delegate"}
                       /\ <<grants[p].scope, grants[g].scope>> \in Covers

\* Authority is never invented: every live grant traces back to one the user
\* issued.
AuthorityTracesToTheUser ==
  \A g \in GrantIds : Live(g) => \E a \in Ancestry(g) : Used(a) /\ grants[a].issuer = "user"

\* Revocation is final, and it takes the whole subtree.
RevokedStaysRevoked == \A g \in GrantIds : g \in revokedIds => ~Live(g)

CascadeTakesTheSubtree ==
  \A g \in GrantIds : Used(g) /\ ~Live(g) => \A c \in Children(g) : ~Live(c)

\* The offered surface never promises what the instance cannot use.
OfferedToolsAreAuthorized ==
  \A i \in Instances :
    /\ ("shell" \in offered[i] => Covers2(i, "shell", "workspace"))
    /\ ("spawn" \in offered[i] => Holds(i, "manage"))
    /\ ("send" \in offered[i] => Holds(i, "message"))
    /\ ("delegate" \in offered[i] => Holds(i, "delegate"))

\* The session boots with exactly the authority D-58 promises — the leader's
\* workspace shell plus manage/delegate/message@session — and the spawned child
\* holds none of it. Guarded by the revision, so it constrains the bootstrap and
\* says nothing about what the user later grants (which is the user's business).
BootstrappedAuthority ==
  revision = 0 =>
    /\ Holds("leader", "manage") /\ Holds("leader", "delegate") /\ Holds("leader", "message")
    /\ Covers2("leader", "shell", "workspace")
    /\ ~Covers2("child", "shell", "workspace")
    /\ \A g \in GrantIds : Used(g) => \E b \in BootstrapSet : b.subject = grants[g].subject
                                            /\ b.action = grants[g].action /\ b.scope = grants[g].scope

=============================================================================

------------------------------ MODULE V2Authority ------------------------------
(***************************************************************************)
(* The user's authority surface (D-61, §5.1/§9, A02/A03): what the surface      *)
(* reads (`grants`), what it may write (`issue_grant`, `revoke_grant`) and what *)
(* a client-side guard refuses before anything is written.                      *)
(*                                                                             *)
(* Code anchors: engine/src/v2/authority.rs (the surface: the pair guard, the    *)
(* warning for a subject that does not exist yet, revoke by id or unambiguous    *)
(* prefix), engine/src/v2/daemon.rs `read_method("grants")` (the view: id,       *)
(* issuer, parent, revoked, session revision), core/src/v2/capability.rs          *)
(* (ACTIONS / asks_about / authorizes_something — the one place that says which   *)
(* action and scope shapes some check consults), core/src/v2/control.rs           *)
(* issue_grant / revoke_grant / revoke_grant_tree / dispatch_operation, and       *)
(* engine/src/v2/driver.rs team_kernel (the model-visible surface is recomputed   *)
(* at every request, so a live grant appears and a revoked one disappears).       *)
(*                                                                             *)
(* V2Grants models where authority *comes from*; this module models the surface  *)
(* a user drives it with. Three questions exist only once the user can:          *)
(*  1. can every live grant be named, and therefore revoked, at all?            *)
(*     `EveryLiveGrantBecomesRevocable` — the view must carry the id, and before *)
(*     D-61 it did not (the SELECT list omitted it), so no client could revoke.   *)
(*  2. does a grant reach the model-visible surface, and does a revocation leave  *)
(*     it again? `GrantReachesTheSurface` — modelled with the surface as a        *)
(*     *cached* variable that only an `Observe` step refreshes, because the code  *)
(*     recomputes it per request and not per grant.                               *)
(*  3. does a revocation take exactly the subtree and nothing else?               *)
(*     `CascadeOnlyTakesTheSubtree` — the other half of V2Grants'                 *)
(*     `CascadeTakesTheSubtree`: that one says children die, this one says        *)
(*     nothing else dies.                                                         *)
(*                                                                             *)
(* Three of the claims below are the reason the two switches `TrustSurface` and  *)
(* `RefreshSurface` and the constant `ViewFields` exist: a configuration that      *)
(* models the plausible mistake must *refute* the claim, or the claim says        *)
(* nothing (see `make verify-model-counterexamples` and verification/REPORT.md).  *)
(* The default configuration states the code as it is.                            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS Instances,     \* e.g. {"leader", "worker"}
          Actions,       \* e.g. {"shell", "manage", "delegate", "message"}
          GrantIds,      \* grant slots, e.g. {"g1", ..., "g5"}
          BaseTools,     \* tools that need no grant (the instance's bindings)
          ViewFields,    \* the fields the grant view carries, e.g.
                         \* {"id", "issuer", "subject", "action", "scope", "revoked"}
          Granter,       \* the identity a user grant records, e.g. "user"
          TrustSurface,  \* counterfactual: dispatch trusts the cached surface
          RefreshSurface \* counterfactual: an instance refreshes its surface

ASSUME {"g1", "g2", "g3"} \subseteq GrantIds
ASSUME {"leader", "worker"} \subseteq Instances
ASSUME {"shell", "manage", "delegate", "message"} \subseteq Actions
\* The model covers the four actions whose checks decide the model-visible tool
\* surface. `task_result` is asked by task settlement (complete_task) and modelled
\* by V2Task, not here.

\* The scope vocabulary: the session, the shared workspace, and one scope per
\* instance ("instance:<id>", the scope a spawn derivation targets).
Scopes == {"session", "workspace"} \union {"instance:" \o i : i \in Instances}

\* The *shape* of a scope: the code builds every resource it asks about from one
\* of these (capability.rs `ScopeShape::of`).
Shapes == {"session", "workspace", "instance"}
ScopeShape ==
  [ s \in Scopes |->
      CASE s = "session"   -> "session"
        [] s = "workspace" -> "workspace"
        [] OTHER           -> "instance" ]   \* instance:leader / instance:worker

\* The resource shapes the checks of an action build (capability.rs `asks_about`):
\* what the runtime asks a grant about.
Asks(a, shape) ==
  \/ a = "shell"    /\ shape = "workspace"
  \/ a = "manage"   /\ shape \in {"session", "instance"}
  \/ a = "message"  /\ shape = "instance"
  \/ a = "delegate" /\ shape = "instance"

\* The pairs a grant of `action` over a scope of shape `granted` could ever
\* authorize: it must cover a resource some check asks that action about. The
\* session covers every resource below it; otherwise a grant covers exactly the
\* shape it names (the code's hierarchical "a/b" prefix rule has no instance in
\* this scope vocabulary). `capability::authorizes_something` accepts exactly
\* these pairs — pinned by that module's table test.
CoversShape(granted, asked) == granted = "session" \/ granted = asked
CoveredPairs ==
  { <<a, s>> \in Actions \X Shapes : \E r \in Shapes : Asks(a, r) /\ CoversShape(s, r) }

\* The pairs the authority surface lets a user write: exactly the covered pairs.
\* A pair outside this set is a *dead grant* — no check consults it, so it
\* authorizes nothing, is never offered and never refused — and the surface
\* refuses it with the reason instead of writing the row (D-61). Pinned by
\* engine/src/v2/authority.rs' refusals and the CLI test.
GrantablePairs ==
  { <<"shell", "workspace">>, <<"shell", "session">>,
    <<"manage", "session">>, <<"manage", "instance">>,
    <<"message", "session">>, <<"message", "instance">>,
    <<"delegate", "session">>, <<"delegate", "instance">> }

\* The session's default authority (the bootstrap, D-58/D-60): the leader holds
\* the shared-workspace shell and the team authority it manages with (its fourth
\* grant, `message@session`, is modelled by V2Grants; this module keeps one free
\* slot for the grant the user writes). A worker the leader spawns holds none of
\* it (§5.1), which is exactly why the user needs the authority surface to hand it
\* one.
BootstrapGrants ==
  [ g1 |-> [subject |-> "leader", action |-> "shell",    scope |-> "workspace"],
    g2 |-> [subject |-> "leader", action |-> "manage",   scope |-> "session"],
    g3 |-> [subject |-> "leader", action |-> "delegate", scope |-> "session"] ]

\* The operation the model registers: the worker's workspace shell, which only a
\* user grant can authorize (the leader's delegation, covered by the bootstrap, is
\* V2Grants' subject). One operation is enough for the claims here and keeps the
\* liveness check small; the dispatch re-check is exercised by the grant the user
\* writes and the revocation that follows it.
Ops ==
  { [owner |-> "worker", action |-> "shell", scope |-> "workspace"] }

ASSUME \A o \in Ops : o.owner \in Instances /\ o.action \in Actions /\ o.scope \in Scopes

\* The tool a grant makes usable (driver.rs `team_kernel` → kernel tool schemas).
ToolOf(a) == CASE a = "manage"   -> "spawn"
                [] a = "message"  -> "send"
                [] a = "delegate" -> "delegate"
                [] a = "shell"    -> "shell"
                [] OTHER          -> a

\* shell is a workspace capability: offering or dispatching it needs a grant
\* covering the workspace resource. The collaboration tools follow the instance's
\* own live grant of that action, whatever its scope (team_kernel), so the
\* dispatch re-check stays the real gate.
WorkspaceTool(a) == a = "shell"

\* Whether the grant view carries the grant's id. Without it a client can list the
\* grants and still not name one: `revoke_grant` takes an id (D-61).
CarriesIds == "id" \in ViewFields

Grant == [subject : Instances \union {"none"},
          action  : Actions \union {"none"},
          scope   : Scopes,
          parent  : GrantIds \union {"none"},
          issuer  : Instances \union {Granter},
          live    : BOOLEAN]

VARIABLES
  grants,      \* grant id -> grant (subject "none" means the slot is unused)
  revision,    \* the session grant revision, bumped by every issue and revoke
  offered,     \* instance -> the tools its model was last shown (one request behind)
  listed,      \* the grant ids the last `list` returned (the user's view)
  ops,         \* operation -> [stamp, effects]
  revokedIds,  \* monitor: grant ids that were ever revoked
  staleOps     \* monitor: operations refused at an outdated revision

vars == <<grants, revision, offered, listed, ops>>
monVars == <<vars, revokedIds, staleOps>>

UsedFor(gr, g) == gr[g].subject # "none"
LiveFor(gr, g) == UsedFor(gr, g) /\ gr[g].live
ChildrenFor(gr, g) == {c \in GrantIds : UsedFor(gr, c) /\ gr[c].parent = g}

RECURSIVE SubtreeFor(_, _), AncestryFor(_, _)
SubtreeFor(gr, g) == {g} \union UNION {SubtreeFor(gr, c) : c \in ChildrenFor(gr, g)}
AncestryFor(gr, g) == {g} \union (IF gr[g].parent = "none" THEN {} ELSE AncestryFor(gr, gr[g].parent))

\* scope_covers over this vocabulary: the session covers every resource below it,
\* otherwise the granted scope must be the requested resource exactly.
CoversScope(granted, asked) == granted = "session" \/ granted = asked

\* A live grant of `action` for `subject` whose scope covers `resource` — the
\* question `dispatch_operation` re-asks at the linearization point.
Covers2For(gr, subject, action, resource) ==
  \E g \in GrantIds : /\ LiveFor(gr, g)
                     /\ gr[g].subject = subject
                     /\ gr[g].action = action
                     /\ CoversScope(gr[g].scope, resource)

\* Any live grant of that action for the subject, whatever its scope: what
\* team_kernel asks for the collaboration tools.
HoldsFor(gr, subject, action) ==
  \E g \in GrantIds : LiveFor(gr, g) /\ gr[g].subject = subject /\ gr[g].action = action

\* Whether the instance is entitled to the action right now.
EntitledIn(gr, i, a) ==
  \/ WorkspaceTool(a) /\ Covers2For(gr, i, a, "workspace")
  \/ ~WorkspaceTool(a) /\ HoldsFor(gr, i, a)

\* The model-visible surface an instance is entitled to, given a grant table.
SurfaceOf(gr, i) ==
  BaseTools \union { ToolOf(a) : a \in { b \in Actions : EntitledIn(gr, i, b) } }

Live(g) == LiveFor(grants, g)
Used(g) == UsedFor(grants, g)
Subtree(g) == SubtreeFor(grants, g)
Ancestry(g) == AncestryFor(grants, g)
Surface(i) == SurfaceOf(grants, i)

\* ------------------------------------------------------------------- actions --
Init ==
  /\ grants = [ g \in GrantIds |->
                  IF g \in DOMAIN BootstrapGrants
                  THEN [subject |-> BootstrapGrants[g].subject, action |-> BootstrapGrants[g].action,
                        scope |-> BootstrapGrants[g].scope, parent |-> "none",
                        issuer |-> Granter, live |-> TRUE]
                  ELSE [subject |-> "none", action |-> "none", scope |-> CHOOSE s \in Scopes : TRUE,
                        parent |-> "none", issuer |-> Granter, live |-> FALSE] ]
  /\ revision = 0
  /\ offered = [i \in Instances |-> SurfaceOf(grants, i)]
  /\ listed = {}
  /\ ops = [o \in Ops |-> [stamp |-> 0, effects |-> 0]]
  /\ revokedIds = {}
  /\ staleOps = {}

\* `teamagents authority`: the user reads the session's grants. A grant is
\* nameable from the view only if the view carries the id at all.
List ==
  /\ listed' = IF CarriesIds THEN {g \in GrantIds : UsedFor(grants, g)} ELSE {}
  /\ UNCHANGED <<grants, revision, offered, ops, revokedIds, staleOps>>

\* `authority grant [--parent G]`: the user issues one scoped grant. The surface
\* refuses a pair no check asks about, so only `GrantablePairs` reach here; and
\* because the user is the root of authority, such a grant can only add authority
\* the user already held — it never widens an instance's own. With a parent the
\* new grant is a *derived* one and must be covered by it (issue_grant's parent
\* check), which is what makes the cascade below reachable from the user's side.
UserGrants(g, subject, action, shape, parent) ==
  /\ ~UsedFor(grants, g)
  /\ subject \in Instances
  /\ <<action, shape>> \in GrantablePairs
  /\ parent \in GrantIds \union {"none"}
  /\ \E scope \in Scopes : ScopeShape[scope] = shape
  /\ (parent = "none" \/ \E p \in GrantIds :
        /\ p = parent /\ LiveFor(grants, p)
        /\ \/ grants[p].action = action
           \/ grants[p].action = "manage" /\ action \in {"message", "delegate"}
        /\ \/ grants[p].scope = "session"
           \/ grants[p].scope = CHOOSE s \in Scopes : ScopeShape[s] = shape)
  /\ LET scope == CHOOSE s \in Scopes : ScopeShape[s] = shape
     IN grants' = [grants EXCEPT ![g] = [subject |-> subject, action |-> action, scope |-> scope,
                                        parent |-> parent, issuer |-> Granter, live |-> TRUE]]
  /\ revision' = revision + 1
  /\ UNCHANGED <<offered, listed, ops, revokedIds, staleOps>>

\* A spawn the leader runs narrows its own authoritiy into a derived grant over the
\* worker (spawn_instance → issue_grant with the spawner's manage grant as parent).
\* That is the tree a user revocation must take with it, and nothing more.
Spawn(g) ==
  /\ ~UsedFor(grants, g)
  /\ \E m \in GrantIds : /\ LiveFor(grants, m) /\ grants[m].subject = "leader"
                        /\ grants[m].action = "manage" /\ grants[m].scope = "session"
                        /\ grants' = [grants EXCEPT ![g] =
                              [subject |-> "leader", action |-> "delegate",
                               scope |-> "instance:worker", parent |-> m,
                               issuer |-> "leader", live |-> TRUE]]
  /\ revision' = revision + 1
  /\ UNCHANGED <<offered, listed, ops, revokedIds, staleOps>>

\* `authority revoke`: the user revokes a grant *it can name* — the id comes from
\* the view, which is why the view's completeness is a property and not a detail.
\* The whole subtree goes with it and the revision moves, so every operation
\* stamped before is stale (A03/A04).
UserRevokes(g) ==
  /\ g \in listed
  /\ LiveFor(grants, g)
  /\ LET gone == SubtreeFor(grants, g)
     IN grants' = [h \in GrantIds |->
                     IF h \in gone THEN [grants[h] EXCEPT !.live = FALSE] ELSE grants[h]]
  /\ revision' = revision + 1
  /\ revokedIds' = revokedIds \union SubtreeFor(grants, g)
  /\ UNCHANGED <<offered, listed, ops, staleOps>>

\* The instance's next request: it recomputes its model-visible surface from the
\* grants of that moment (driver::team_kernel runs once per request). The surface
\* is therefore *cached* between requests — a revocation lands in `grants` first
\* and in `offered` at the next turn, which is what makes the two properties below
\* say something.
Observe(i) ==
  /\ offered' = IF RefreshSurface THEN [offered EXCEPT ![i] = SurfaceOf(grants, i)] ELSE offered
  /\ UNCHANGED <<grants, revision, listed, ops, revokedIds, staleOps>>

\* Every instance takes its next turn. This is the fairness the liveness property
\* needs: an instance that never runs again is a session that ended, and nothing is
\* owed to it (same assumption as V2Wait's parked drain).
ObserveAll ==
  /\ offered' = IF RefreshSurface THEN [i \in Instances |-> SurfaceOf(grants, i)] ELSE offered
  /\ UNCHANGED <<grants, revision, listed, ops, revokedIds, staleOps>>

\* A request registers its operations with the revision of the moment; an
\* operation is registered once (a new attempt is a new operation).
Prepare(o) ==
  /\ ops[o].effects = 0 /\ o \notin staleOps
  /\ ops' = [ops EXCEPT ![o] = [ops[o] EXCEPT !.stamp = revision]]
  /\ UNCHANGED <<grants, revision, offered, listed, revokedIds, staleOps>>

\* Dispatch (the linearization point): the stamp must still be current, and the
\* authority is re-read from the live grants. `TrustSurface` models the mistake the
\* design forbids (§6.1/A04) — trusting the cached model-visible surface — and the
\* negative control must refute `AuthorizedEffectsOnly`.
Dispatch(o) ==
  /\ ops[o].effects = 0
  /\ ops[o].stamp = revision
  /\ (IF TrustSurface THEN ToolOf(o.action) \in offered[o.owner]
                      ELSE Covers2For(grants, o.owner, o.action, o.scope))
  /\ ops' = [ops EXCEPT ![o] = [ops[o] EXCEPT !.effects = 1]]
  /\ UNCHANGED <<grants, revision, offered, listed, revokedIds, staleOps>>

\* A dispatch attempt at an outdated revision: refused, recorded, no effect.
RefusedDispatch(o) ==
  /\ ops[o].effects = 0
  /\ ops[o].stamp # revision
  /\ staleOps' = staleOps \union {o}
  /\ UNCHANGED <<grants, revision, offered, listed, ops, revokedIds>>

Next ==
  \/ List
  \/ \E g \in GrantIds, subject \in Instances, action \in Actions, shape \in Shapes,
        parent \in GrantIds \union {"none"} :
       UserGrants(g, subject, action, shape, parent)
  \/ \E g \in GrantIds : Spawn(g)
  \/ \E g \in GrantIds : UserRevokes(g)
  \/ \E i \in Instances : Observe(i)
  \/ ObserveAll
  \/ \E o \in Ops : Prepare(o)
  \/ \E o \in Ops : Dispatch(o)
  \/ \E o \in Ops : RefusedDispatch(o)
  \/ UNCHANGED monVars   \* the session idles: the model stays non-terminating

\* `List` and `ObserveAll` are the client's and the instances' own progress: the
\* properties below are conditional on them (a client that never reads qualifies
\* nothing, and neither does an instance that never takes another turn).
Spec == Init /\ [][Next]_monVars /\ WF_monVars(List) /\ WF_monVars(ObserveAll)

\* ------------------------------------------------------------------ properties --
TypeOK ==
  /\ grants \in [GrantIds -> Grant]
  /\ revision \in Nat
  /\ offered \in [Instances -> SUBSET (BaseTools \union {ToolOf(a) : a \in Actions})]
  /\ listed \subseteq GrantIds
  /\ ops \in [Ops -> [stamp : Nat, effects : 0..1]]

\* The surface writes no pair that authorizes nothing: what it accepts is exactly
\* what some check asks about. `CoveredPairs` is derived from the asks table,
\* `GrantablePairs` is the surface's own; if either table drifts, this fails here
\* instead of a user silently writing a grant nothing consults.
NoDeadGrantPair == GrantablePairs = CoveredPairs

\* The view never invents an id.
ListedIdsAreUsed == listed \subseteq {g \in GrantIds : UsedFor(grants, g)}

\* Every live grant eventually appears in the view, and so can be revoked. This is
\* the claim the missing `id` field refuted before D-61 (the negative control
\* drops it from `ViewFields`).
EveryLiveGrantBecomesRevocable ==
  \A g \in GrantIds : [](Live(g) => <>(g \in listed))

\* Authority is never invented: every live grant traces back to one the user
\* issued — the four bootstrap grants, or a grant written through the surface.
AuthorityTracesToTheUser ==
  \A g \in GrantIds : Live(g) => \E a \in Ancestry(g) : Used(a) /\ grants[a].issuer = Granter

\* Revocation is final...
RevokedStaysRevoked == \A g \in GrantIds : g \in revokedIds => ~Live(g)

\* ...and a derived grant never exceeds its parent (issue_grant's parent check).
ChildGrantsAreCoveredByTheirParent ==
  \A g \in GrantIds : Used(g) /\ grants[g].parent # "none" =>
    \E p \in GrantIds : /\ grants[g].parent = p /\ Used(p)
                       /\ \/ grants[p].action = grants[g].action
                          \/ grants[p].action = "manage" /\ grants[g].action \in {"message", "delegate"}
                       /\ CoversScope(grants[p].scope, grants[g].scope)

\* A revocation takes the whole subtree (V2Grants)...
CascadeTakesTheSubtree ==
  \A g \in GrantIds : Used(g) /\ ~Live(g) => \A c \in ChildrenFor(grants, g) : ~Live(c)

\* ...and exactly the subtree: revoking the worker's shell must not take the
\* leader's authority with it. Only a grant the user named (from the view) may be
\* the root of a revocation.
CascadeOnlyTakesTheSubtree ==
  [][ \A h \in GrantIds : (Live(h) /\ ~Live(h)') =>
        \E g \in listed : h \in Subtree(g) ]_monVars

\* Every effect is produced by a dispatch that held the authority at that very
\* step: a live covering grant and a still-current stamped revision. This holds
\* even though the cached surface may lag (the negative control that trusts the
\* surface must refute it).
AuthorizedEffectsOnly ==
  [][ \A o \in Ops : (ops'[o].effects > ops[o].effects) =>
        (ops[o].stamp = revision /\ Covers2For(grants, o.owner, o.action, o.scope)) ]_monVars

\* An operation has at most one effect (recovery never replays it, A08/A10).
EffectAtMostOnce == \A o \in Ops : ops[o].effects <= 1

\* An operation refused at an outdated revision can never take effect afterwards.
OnceStaleNeverExecutes == \A o \in Ops : o \in staleOps => ops[o].effects = 0

\* The surface only ever changes to the entitlement of that moment: it is
\* recomputed, never invented and never carried over from another instance.
SurfaceChangesOnlyToTheCurrentEntitlement ==
  [][ \A i \in Instances : (offered[i] # offered'[i]) => offered'[i] = SurfaceOf(grants, i) ]_monVars

\* A surface that lags the entitlement catches up at the instance's next request.
\* (Not "a grant is always offered afterwards": the user may revoke it again before
\* the instance takes its next turn, and then the correct surface has no shell.)
\* This is what makes the grant useful rather than decorative; the negative
\* control where the surface is computed once and never refreshed refutes it.
StaleSurfaceCatchesUp ==
  \A i \in Instances : []( offered[i] # Surface(i) => <>(offered[i] = Surface(i)) )

\* The session boots with exactly the authority D-58 promises — the leader's
\* workspace shell plus manage/delegate/message@session — and a spawned worker
\* holds none of it, so the user's own grant is the only way one gets a shell.
BootstrappedAuthority ==
  revision = 0 =>
    /\ HoldsFor(grants, "leader", "manage") /\ HoldsFor(grants, "leader", "delegate")
    /\ Covers2For(grants, "leader", "shell", "workspace")
    /\ ~Covers2For(grants, "worker", "shell", "workspace")
    /\ \A g \in GrantIds : UsedFor(grants, g) =>
         \E b \in {"g1", "g2", "g3"} : grants[g].subject = BootstrapGrants[b].subject
                                             /\ grants[g].action = BootstrapGrants[b].action
                                             /\ grants[g].scope = BootstrapGrants[b].scope

=============================================================================

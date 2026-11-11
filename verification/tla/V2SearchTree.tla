----------------------------- MODULE V2SearchTree -----------------------------
(***************************************************************************
D-506: bounded session exploration-tree control commands.

Code anchors: core/src/v2/control.rs::{read_search_tree,apply_search_tree,
compress_context}, core/src/v2/control/search_tree.rs. One session has one
Leader-maintained tree. User/Leader reads and writes are authorized; members
and the runtime are not ordinary writers. A new patch needs the current base
revision. A matching cause replays its old result without growing history;
a changed cause payload is rejected. Patches preserve root/reachability/focus,
and merge unions references and reparents children. Compaction commits its
summary even when the tree patch fails, while a failed patch leaves the whole
tree and revision unchanged. Source freshness compares instance/epoch/entry.

Abstraction: at most root+a+b, one cause id, one entry per epoch and integer
revisions stand in for JSON text and SQLite rows. Request records model the
five operation families. Multi-operation patch order, arbitrary strings and
provider parsing remain in Rust tests. This is finite behavioral evidence, not
Rust/SQLite refinement or proof of semantic tree quality. Bad selects one
missing guard; every behavioral claim has its own refuting configuration.
***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Bad, Family

Actors == {"user", "leader", "member", "system"}
NodeIds == {"root", "a", "b", "c"}
BaseNodes == {"root", "a", "b"}
AllNodes == NodeIds \cup {"none", "missing"}
StatusIds == {"active", "open", "resolved", "blocked", "abandoned"}
CauseIds == {"c1"}
NullSource == [instance |-> "none", epoch |-> (0 - 1), entry |-> "none"]

Request(op, base, node, parent, status) ==
  [op |-> op, base |-> base, node |-> node, parent |-> parent, status |-> status]

NoRequest == Request("none", (0 - 1), "none", "none", "none")

AddRequests ==
  { Request("add", base, node, parent, "open") :
      base \in 0..3, node \in {"a", "b"}, parent \in NodeIds }
UpdateRequests ==
  { Request("update", base, node, "none", status) :
      base \in 0..3, node \in {"a", "b"}, status \in {"resolved", "abandoned"} }
MoveRequests ==
  { Request("move", base, node, parent, "open") :
      base \in 0..3, node \in {"a", "b"}, parent \in NodeIds }
MergeRequests ==
  { Request("merge", base, node, parent, "open") :
      base \in 0..3, node \in {"a", "b"}, parent \in NodeIds }
FocusRequests ==
  { Request("focus", base, node, "none", "open") :
      base \in 0..3, node \in NodeIds }

Requests == CASE Family = "add" -> AddRequests
              [] Family = "update" -> UpdateRequests
              [] Family = "move" -> MoveRequests
              [] Family = "merge" -> MergeRequests
              [] OTHER -> FocusRequests

VARIABLES
  nodes,
  parent,
  refs,
  status,
  focus,
  source,
  position,
  revision,
  history,
  causeRevision,
  causeRequest,
  usedIds,
  summaries,
  lastApply,
  lastRead,
  lastCompact

vars == <<nodes, parent, refs, status, focus, source, position, revision,
          history, causeRevision, causeRequest, usedIds, summaries,
          lastApply, lastRead, lastCompact>>

Tree == [nodes |-> nodes, parent |-> parent, refs |-> refs, status |-> status,
         focus |-> focus, source |-> source]

NoApply == [kind |-> "none", actor |-> "none", request |-> NoRequest,
            accepted |-> FALSE, replay |-> FALSE, before |-> Tree,
            beforeRevision |-> revision, beforePosition |-> position]
NoRead == [kind |-> "none", actor |-> "none", accepted |-> FALSE,
           stale |-> FALSE, before |-> Tree, beforeRevision |-> revision]
NoCompact == [kind |-> "none", applied |-> FALSE, error |-> FALSE,
              before |-> Tree, beforeRevision |-> revision, beforeSummaries |-> summaries]

RefFor(node) == CASE node = "a" -> "ra"
                  [] node = "b" -> "rb"
                  [] OTHER -> "other"

Descendant(candidate, ancestor) ==
  \/ candidate = ancestor
  \/ parent[candidate] = ancestor
  \/ parent[parent[candidate]] = ancestor
  \/ parent[parent[parent[candidate]]] = ancestor

ReachesRoot(node) ==
  \/ node = "root"
  \/ parent[node] = "root"
  \/ parent[parent[node]] = "root"
  \/ parent[parent[parent[node]]] = "root"

Valid(request) ==
  CASE request.op = "add" ->
         /\ request.node \in {"a", "b"}
         /\ request.node \notin usedIds
         /\ request.parent \in nodes
         /\ Cardinality(nodes) < 3
    [] request.op = "update" ->
         /\ request.node \in nodes
         /\ request.node # "root"
         /\ request.status \in {"resolved", "abandoned"}
    [] request.op = "move" ->
         /\ request.node \in nodes
         /\ request.node # "root"
         /\ request.parent \in nodes
         /\ (Bad = "cycle" \/ ~Descendant(request.parent, request.node))
    [] request.op = "merge" ->
         /\ request.node \in nodes
         /\ request.parent \in nodes
         /\ request.node # "root"
         /\ request.node # request.parent
         /\ (Bad = "cycle" \/ ~Descendant(request.parent, request.node))
    [] request.op = "focus" -> request.node \in nodes
    [] OTHER -> FALSE

Evaluate(request) ==
  LET isAdd == request.op = "add"
      isMove == request.op = "move"
      isMerge == request.op = "merge"
      isUpdate == request.op = "update"
      isFocus == request.op = "focus"
      newNodes == CASE isAdd -> nodes \cup {request.node}
                    [] isMerge -> nodes \ {request.node}
                    [] OTHER -> IF Bad = "root_delete" THEN nodes \ {"root"}
                                ELSE IF Bad = "node_limit" THEN nodes \cup {"c"} ELSE nodes
      newParent == [node \in AllNodes |->
        CASE isAdd /\ node = request.node -> request.parent
          [] isMove /\ node = request.node -> IF Bad = "parent_drop" THEN "none" ELSE request.parent
          [] isMerge /\ node = request.node -> "none"
          [] isMerge /\ parent[node] = request.node ->
               IF Bad = "merge_structure" THEN parent[node] ELSE request.parent
          [] OTHER -> parent[node]]
      newRefs == [node \in AllNodes |->
        CASE isAdd /\ node = request.node -> {RefFor(request.node)}
          [] isMerge /\ node = request.parent ->
               IF Bad = "merge_refs" THEN refs[request.parent]
               ELSE refs[request.parent] \cup refs[request.node]
          [] isMerge /\ node = request.node -> {}
          [] OTHER -> refs[node]]
      newStatus == [node \in AllNodes |->
        IF isUpdate /\ node = request.node THEN request.status ELSE status[node]]
      newFocus == CASE isFocus -> IF Bad = "focus" THEN "missing" ELSE request.node
                    [] isMerge /\ focus = request.node ->
                         IF Bad = "merge_structure" THEN focus ELSE request.parent
                    [] OTHER -> focus
      newSource == IF Bad = "source" THEN source ELSE position
  IN [nodes |-> newNodes, parent |-> newParent, refs |-> newRefs,
      status |-> newStatus, focus |-> newFocus, source |-> newSource]

CommitTree(result) ==
  /\ nodes' = result.nodes
  /\ parent' = result.parent
  /\ refs' = result.refs
  /\ status' = result.status
  /\ focus' = result.focus
  /\ source' = result.source

Stale ==
  IF revision = 0 /\ position.entry = "none" THEN FALSE ELSE source # position

Apply(actor, cause, request) ==
  LET result == Evaluate(request)
      replay == causeRevision[cause] >= 0
      matching == causeRequest[cause] = request
      authorized == actor \in {"user", "leader"} \/ Bad = "member_write"
      fresh == ~replay /\ (request.base = revision \/ Bad = "stale")
                /\ Valid(request) /\ revision < 3
      accepted == authorized /\ ((replay /\ matching) \/ fresh)
      writes == accepted /\ (~replay \/ Bad = "replay")
  IN
  /\ lastApply' = [kind |-> "apply", actor |-> actor,
                   request |-> IF accepted THEN request ELSE NoRequest,
                   accepted |-> accepted, replay |-> replay, before |-> Tree,
                   beforeRevision |-> revision, beforePosition |-> position]
  /\ IF writes THEN
       /\ CommitTree(result)
       /\ revision' = revision + 1
       /\ history' = IF Bad = "delete_history" THEN (history \cup {revision + 1}) \ {0}
                     ELSE history \cup {revision + 1}
       /\ usedIds' = usedIds \cup IF request.op = "add" THEN {request.node} ELSE {}
       /\ causeRevision' = [causeRevision EXCEPT ![cause] = revision + 1]
       /\ causeRequest' = [causeRequest EXCEPT ![cause] = request]
     ELSE IF Bad = "rejected_revision" /\ ~accepted /\ revision < 3 THEN
       /\ revision' = revision + 1
       /\ history' = history \cup {revision + 1}
       /\ UNCHANGED <<nodes, parent, refs, status, focus, source,
                      usedIds, causeRevision, causeRequest>>
     ELSE
       UNCHANGED <<nodes, parent, refs, status, focus, source, revision,
                   history, usedIds, causeRevision, causeRequest>>
  /\ lastRead' = NoRead
  /\ lastCompact' = NoCompact
  /\ position' = IF Bad = "apply_position" THEN [position EXCEPT !.epoch = 1] ELSE position
  /\ UNCHANGED summaries

Read(actor) ==
  /\ lastRead' = [kind |-> "read", actor |-> actor,
                  accepted |-> actor \in {"user", "leader"} \/ Bad = "member_read",
                  stale |-> IF Bad = "read_stale" THEN ~Stale ELSE Stale,
                  before |-> Tree, beforeRevision |-> revision]
  /\ lastApply' = NoApply
  /\ lastCompact' = NoCompact
  /\ focus' = IF Bad = "read_writes" THEN IF focus = "root" THEN "a" ELSE "root" ELSE focus
  /\ UNCHANGED <<nodes, parent, refs, status, source, position, revision,
                 history, causeRevision, causeRequest, usedIds, summaries>>

AdvanceEntry ==
  /\ position.entry = "none"
  /\ position' = [position EXCEPT !.entry = IF position.epoch = 0 THEN "e0" ELSE "e1"]
  /\ lastApply' = NoApply
  /\ lastRead' = NoRead
  /\ lastCompact' = NoCompact
  /\ UNCHANGED <<nodes, parent, refs, status, focus, source, revision,
                 history, causeRevision, causeRequest, usedIds, summaries>>

ResetEpoch ==
  /\ position.epoch = 0
  /\ position' = [position EXCEPT !.epoch = 1, !.entry = "none"]
  /\ lastApply' = NoApply
  /\ lastRead' = NoRead
  /\ lastCompact' = NoCompact
  /\ UNCHANGED <<nodes, parent, refs, status, focus, source, revision,
                 history, causeRevision, causeRequest, usedIds, summaries>>

Compact(request) ==
  LET result == Evaluate(request)
      validPatch == request.base = revision /\ Valid(request) /\ revision < 3
      applies == validPatch
      partial == Bad = "partial_compact" /\ ~validPatch
  IN
  /\ summaries < 4
  /\ summaries' = IF Bad = "drop_summary" THEN summaries ELSE summaries + 1
  /\ lastCompact' = [kind |-> "compact", applied |-> applies,
                     error |-> IF Bad = "compact_error_flag" THEN FALSE ELSE ~applies,
                     before |-> Tree, beforeRevision |-> revision, beforeSummaries |-> summaries]
  /\ lastApply' = NoApply
  /\ lastRead' = NoRead
  /\ IF applies THEN
       /\ CommitTree(result)
       /\ revision' = IF Bad = "compact_no_advance" THEN revision ELSE revision + 1
       /\ history' = IF Bad = "compact_no_advance" THEN history ELSE history \cup {revision + 1}
       /\ usedIds' = usedIds \cup IF request.op = "add" THEN {request.node} ELSE {}
       /\ causeRevision' = causeRevision
       /\ causeRequest' = causeRequest
     ELSE IF partial THEN
       /\ revision' = revision + 1
       /\ history' = history \cup {revision + 1}
       /\ UNCHANGED <<nodes, parent, refs, status, focus, source,
                      usedIds, causeRevision, causeRequest>>
     ELSE
       UNCHANGED <<nodes, parent, refs, status, focus, source, revision,
                   history, usedIds, causeRevision, causeRequest>>
  /\ UNCHANGED position

Stutter == UNCHANGED vars

Next ==
  \/ \E actor \in Actors : \E cause \in CauseIds : \E request \in Requests : Apply(actor, cause, request)
  \/ \E actor \in Actors : Read(actor)
  \/ \E request \in Requests : Compact(request)
  \/ AdvanceEntry
  \/ ResetEpoch
  \/ Stutter

Init ==
  \* Move/update/merge/focus start with a valid populated tree. Merge starts
  \* with a focused source and child, exercising reparenting and focus transfer.
  /\ nodes = IF Family \in {"update", "move", "merge", "focus"} THEN BaseNodes ELSE {"root"}
  /\ parent = [node \in AllNodes |-> IF Family = "merge" /\ node = "b" THEN "a"
                                    ELSE IF node \in {"a", "b"} THEN "root" ELSE "none"]
  /\ refs = [node \in AllNodes |-> IF node = "a" THEN {"ra"} ELSE IF node = "b" THEN {"rb"} ELSE {}]
  /\ status = [node \in AllNodes |-> IF node = "root" THEN "active" ELSE "open"]
  /\ focus = IF Family = "merge" THEN "a" ELSE "root"
  /\ source = NullSource
  /\ position = [instance |-> "leader", epoch |-> 0, entry |-> "none"]
  /\ revision = 0
  /\ history = {0}
  /\ causeRevision = [cause \in CauseIds |-> (0 - 1)]
  /\ causeRequest = [cause \in CauseIds |-> NoRequest]
  /\ usedIds = {"root"} \cup nodes
  /\ summaries = 0
  /\ lastApply = NoApply
  /\ lastRead = NoRead
  /\ lastCompact = NoCompact

Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ nodes \subseteq NodeIds
  /\ \A node \in AllNodes : parent[node] \in AllNodes
  /\ \A node \in AllNodes : refs[node] \subseteq {"ra", "rb", "other"}
  /\ \A node \in AllNodes : status[node] \in StatusIds
  /\ focus \in AllNodes
  /\ revision \in 0..3
  /\ history \subseteq 0..3
  /\ usedIds \subseteq NodeIds
  /\ summaries \in 0..4
  /\ position.epoch \in 0..1
  /\ position.entry \in {"none", "e0", "e1"}

OnlyAuthorizedReads ==
  lastRead.kind = "read" /\ lastRead.actor \in {"member", "system"} => ~lastRead.accepted

OnlyAuthorizedWrites ==
  lastApply.kind = "apply" /\ lastApply.actor \in {"member", "system"} => ~lastApply.accepted

RejectedApplyPreservesTree ==
  lastApply.kind = "apply" /\ ~lastApply.accepted =>
    Tree = lastApply.before /\ revision = lastApply.beforeRevision

ReplayDoesNotGrow ==
  lastApply.kind = "apply" /\ lastApply.accepted /\ lastApply.replay =>
    revision = lastApply.beforeRevision

FreshApplyRequiresCurrentRevision ==
  lastApply.kind = "apply" /\ lastApply.accepted /\ ~lastApply.replay =>
    lastApply.request.base = lastApply.beforeRevision

ApplyPreservesPosition ==
  lastApply.kind = "apply" => position = lastApply.beforePosition

ReadDoesNotChangeTree ==
  lastRead.kind = "read" => Tree = lastRead.before

ReadReportsStale ==
  lastRead.kind = "read" /\ lastRead.accepted => lastRead.stale = Stale

RootPresent == "root" \in nodes
ParentsExist == \A node \in nodes \ {"root"} : parent[node] \in nodes
Acyclic == \A node \in nodes : ReachesRoot(node)
FocusExists == focus \in nodes
NodeLimit == Cardinality(nodes) <= 3
RevisionHistoryComplete == history = 0..revision

AcceptedApplyRefreshesSource ==
  lastApply.kind = "apply" /\ lastApply.accepted /\ ~lastApply.replay => source = position

MergePreservesRefs ==
  lastApply.kind = "apply" /\ lastApply.accepted /\ ~lastApply.replay
  /\ lastApply.request.op = "merge" =>
    refs[lastApply.request.parent] =
      lastApply.before.refs[lastApply.request.parent] \cup lastApply.before.refs[lastApply.request.node]

MergePreservesStructure ==
  lastApply.kind = "apply" /\ lastApply.accepted /\ ~lastApply.replay
  /\ lastApply.request.op = "merge" =>
    /\ focus = IF lastApply.before.focus = lastApply.request.node
               THEN lastApply.request.parent ELSE lastApply.before.focus
    /\ \A node \in AllNodes:
         lastApply.before.parent[node] = lastApply.request.node /\ node # lastApply.request.node =>
           parent[node] = lastApply.request.parent

CompactionAlwaysCommitsSummary ==
  lastCompact.kind = "compact" => summaries = lastCompact.beforeSummaries + 1

FailedCompactionPreservesTree ==
  lastCompact.kind = "compact" /\ ~lastCompact.applied =>
    Tree = lastCompact.before /\ revision = lastCompact.beforeRevision

FailedCompactionMarksError ==
  lastCompact.kind = "compact" /\ ~lastCompact.applied => lastCompact.error

ValidCompactionAdvancesTree ==
  lastCompact.kind = "compact" /\ lastCompact.applied =>
    revision = lastCompact.beforeRevision + 1 /\ source = position
=============================================================================

# DSec technical report: kernel reference notes (2026-09-24)

Source: DeepSeek, "DSec: A Sandbox Infrastructure for Effective Agentic Training at Scale"
(arXiv:2609.22978v1, 2026-09-19; a local copy at `/tmp/dsec.txt`). The user suggested reading it for its
reference value to the TeamAgents kernel. DSec is a sandbox execution platform for agentic RL
training/evaluation (~3M sandboxes per day, a peak of ~380K concurrent, 5K creations per second), which sits
at a different layer than TeamAgents — it manages execution backends while we manage multi-agent transactions
and collaboration — but four of its production lessons map directly onto the design, and one is a strong input
to the completion-check work. They follow in order of relevance; § numbers are the report's sections.

## 1. Authoritative execution state outside the executor, reconnect instead of replay (§6.2) — confirms R19/A25

DSec's evolution: the early agent loop ran inside preemptible GPU pods and reconciled after a preemption by
**replaying a command log** (completed operations reused their recorded result, avoiding duplicate side
effects of non-idempotent commands); from V4.1 the rollout execution moved out of the GPU pool, with a worker
container plus an agent sandbox holding the complete rollout state as the **single source of truth**, so a
preempted training job simply reconnects and continues and the replay reconciliation was deleted entirely.

Mapping to this codebase: the supervisor plus the SQLite WAL are the single authoritative state outside the
client processes, and the TUI reconnects through the daemon protocol from the last watermark; a tool call that
was already DISPATCH_COMMITTED before a crash becomes OUTCOME_UNKNOWN and is never replayed blindly. Two
independent systems converged on the same conclusion: **when a non-idempotent outcome is uncertain, never
re-execute; recover from authoritative state plus a reconnect, never from a replay.** DSec's scale is
supporting evidence for the direction; no action needed.

## 2. Bounded delegation (§3.2 IAM) — verified equivalent here

DSec: projects nest, "a principal cannot grant permissions it does not hold", and a child project's policy and
quota can never exceed its parent's; humans and agents use the same management API and authorization model.

The corresponding paths in this codebase (each verified during that round):

- `core/src/v2/control.rs::spawn_instance`: spawning needs a `manage@session` grant and otherwise fails
  closed (test `spawn_requires_a_manage_grant`); a new child instance gains **no shell@workspace
  automatically** (that path belongs to user-driven creation only); the spawner's delegate permission derives
  from its manage grant; and the initial task plus its narrow return path register in the same transaction.
- `core/src/v2/control.rs::issue_grant`: an instance may only issue a **strict narrowing** of its own
  effective grants (same action plus `scope_covers`; manage may only cover message/delegate), and the parent
  chain is validated (the parent must exist, be unrevoked and cover the child), so revoking a parent
  invalidates its children.

Conclusion: DSec's bounded-delegation principle already holds here; no action needed.

## 3. Final checks are not proof, plus a misbehavior catalogue (§6.4/§6.5) — direct input to the completion checks

DSec states plainly: "**Final-output checks alone cannot reliably establish whether the agent solved the task
as intended.**" That is exactly the premise behind the completion-check repair loop: a self-reported
`outcome=success` may not settle as SUCCEEDED, required checks must actually run, and a check binds the
declared inputs and artifact versions (sha256) together with the observation time. The misbehavior patterns
the report records give check design a concrete list of bypasses:

- Forged internal RPCs (sending constructed messages to a control socket) and reading platform logs for
  leftover answers
  → our counterpart: the event log and receipts are visible inside the session, so a check may never trust
    what an instance *claims* to have read — only independently executed check commands, their exit codes and
    their hashes.
- Overwriting `/bin/bash` to inject into later sessions, or `XFS_IOC_SWAPEXT` to swap file extents and bypass
  file protection
  → counterpart: the sha256 binding of required checks must be **re-observed at the moment of completion**,
    never taken from an instance's own report (the completion-check design already says so).
- Port scanning and service discovery against reachable images, or fetching reference implementations through
  a Go module proxy
  → counterpart: information permissions (`audience` scoping in the core views) only govern visibility inside
    the session; network-level control does not exist here and is listed as a `ponytail:` candidate below.
- Non-malicious damage: `yes` accumulating tens of GB of output, or a recursive grep from the root walking
  `/proc` and tripping a kernel bug
  → verified counterpart: `engine/src/tools.rs` captures at most 200KB of a command's head plus bounded
    artifact spooling with an explicit truncation marker (test
    `output_capture_spools_bounded_previews...`).

The general principle in §6.5 applies to the hardening roadmap as well: "No single mechanism can prevent all
agent misbehavior... strengthen observability and continuously harden". Our event sourcing (one event per
command, inside the same transaction) is the foundation of that observability and already holds.

## 4. Stateless coordination layer (§3.2/§3.3) — confirms the storage layering here

DSec: the apiserver holds no per-sandbox state (the sandbox id encodes its edge, so any instance can route);
the placement engine and watcher need no durable state and rebuild their view by polling after a restart.
Durable state lives only at the edge and in the sandbox. The same holds here: the SQLite WAL is the only
durable state and the supervisor's in-memory view is rebuilt from the store (the driver's crash-recovery path
relies on exactly that). No action needed.

## 5. Deliberately incomplete abstraction (§2.1) — confirms the provider boundary philosophy

The DSec SDK is "intentionally not a full semantic abstraction over all backends": one access path and a
similar operation model, with the caller choosing the backend. That is the same philosophy as our provider
edge: differences stay at the adapter boundary instead of being flattened into a fake uniform semantics (see
§7 of the design baseline and the pi-ai comparison in the earlier provider review, which is reachable through
Git history). No action needed.

## `ponytail:` candidates (recorded only, no code)

- **Network-level access control**: DSec enforces a per-sandbox eBPF allowlist per domain/image and can update
  it as a task progresses. Our authorization stops at the tool-binding layer
  (`bindings=[files,shell,web,skills]`) and web fetches have no target-domain control. If a requirement ever
  appears for "an instance must not fetch a reference implementation it should not see", the upgrade path is a
  target-domain allowlist for the web tool expressed as a grant (`resource_scope=domain:...`).
- **Instance suspend/transparent resume**: DSec's pause/resume is transparent to callers (the next request
  wakes the sandbox). Our idle instances hold no heavy resources (no resident sandbox memory), so there is no
  need today; if a heavy execution backend (container sandbox) is ever added, the DSec suspend protocol is the
  direct reference.
- **Versioned environment layering**: DSec versions base image, workspace and toolkit independently to avoid
  O(m·N) rebuilds. That loosely matches our profile / context layer / bound tools split; current scale carries
  no maintenance pressure, so it is recorded as a concept only.

## Conclusion

DSec's main value for this project is **direction confirmation**: authoritative state outside the executor plus
reconnect, bounded delegation, final checks not being proof, a stateless coordination layer and an
adapter-boundary philosophy all already exist here or have an equivalent, and two of them were verified item by
item during that round. The only direct action item concerns the completion checks: a completion verdict must
use independently executed checks, re-observe hashes at that moment and never trust an instance's own report —
the design already covers this, so it is implemented as planned.

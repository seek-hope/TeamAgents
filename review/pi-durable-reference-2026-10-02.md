# Pi 1.0 and Pi Durable (Pico5): reference notes

Read on 2026-10-02. Two things shipped on 2026-10-01 and are treated separately here, because they are
different products:

- **Pi 1.0.0** — the coding-agent CLI this machine already runs (installed at
  `~/.pi/agent/install/releases/1.0.0`): the TUI plus the `pi-agent-core` agent loop. Its changelog is
  `CHANGELOG.md` in that install.
- **`@earendil-works/pi-durable` 1.0.0** — a **separate** package ("Durable conversation, task, and document
  runtime for Pi"), whose normative document calls it **Pico5**: "a durable, extensible agent harness".
  The CLI does **not** depend on it (`pi-coding-agent`'s dependency list holds `pi-agent-core`, not
  `pi-durable`), so this is a reference architecture, not the thing the `pi` binary runs. Its README calls
  itself **Experimental**: "The API changes without notice between releases."

Sources used (no network claim beyond these): the local 1.0.0 install; the published package README, its
normative specification document, and its implementation-handoff and Chord-usage notes (npm tarball of
`@earendil-works/pi-durable@1.0.0`, plus the same files in the earendil-works/pi repository at github.com); npm
metadata for the `@earendil-works` package set.

## 1. What Pico5 is

A durable agent harness in which *everything visible is committed first*:

> Conversations, model turns, tool calls, and your own state are committed to storage before anything is shown.
> If the process dies mid-turn, reopening the storage picks the work up where it stopped.

Its vocabulary, and the nearest thing this repository has:

| Pico5 | What it is | Nearest here |
|---|---|---|
| Session | one **mutation line**: conversations, entries, tasks, submissions, documents | the state root's SQLite database plus the single-writer worker |
| Conversation | a transcript scope; may fork another | a session, and `sessions fork` |
| Entry | an immutable transcript record (user, assistant, tool result, system, reset, or app-defined) | a context entry |
| **Commit** | one atomic write of entries **and** documents **and** task records together | one `Control::submit` transaction |
| Document | typed JSON state living beside the transcript, changed only in commits | the tables the control plane writes (goals, limits, usage) |
| **Task** | a durable state machine with a checkpoint at every step | a task/goal, but see the state model in section 4 below |
| Submission | what you hand a conversation, with a `wait()`; input, or a write | an input envelope, and a queued input |
| Extension / Registry | named bundle of tools, prompt sections, hooks, tasks; installed per process, **stored by name** | tools, bindings, skills, hooks |
| Turn / run | one model response and its tools / the turns from an input to its answer | a turn, and a goal's work |

The spec's first-section invariants are worth reading in full; four of them name failures this project has also had to
choose an answer for:

1. one commit is atomic across records and documents;
2. a document update is published only after storage commits;
3. **all visible progress is durable — there is no volatile publication path**;
4. external effects never run inside the mutation transaction;
5. entries and IDs are immutable and never reused;
6. drafts are revoked when the transaction callback settles, values copied by value and strict JSON;
7. the mutation line stays held through storage settlement and committed-state adoption;
8. **an uncertain storage failure is fatal to the open Session** — it publishes nothing and must be reopened,
   while preparation/checkpoint failures roll back normally.

## 2. Durability and recovery, concretely

- **Storage** is Memory, SQLite (one file; "WAL mode with `synchronous = NORMAL`: commits survive process
  crashes; the newest may be lost on power or host failure") or JSONL (append-only files, `{ fsync: true }`
  optional). **"One process owns a storage at a time; there is no cross-process locking."** This repository is
  **stricter** on both counts today: `synchronous = FULL`, and a coordinator lock plus a socket that refuses a
  second daemon on a state root. That is a deliberate trade Pico5 made the other way, and it is the single
  biggest durability difference between the two designs.
- **Resume**: after a crash or a clean close, work stays pending, `harness.resume()` starts the scheduler, and
  a retried submission with the same `requestId` returns the existing submission instead of doing it twice.
  `harness.submission(id)` reacquires a submission by id after a restart.
- **Tool replay is declared per tool**, not inferred: a tool call's *intent* is committed before `execute()`
  runs; on reopen the call reruns **only** if the tool declared `replay: "safe"`, otherwise the model receives
  an `interrupted` error result together with whatever output had been committed. Subagent tools declare
  `replay: "safe"` precisely so that a rerun finds the same child conversation and the same submission.
- **Task states**: `pending`, `running`, `waiting`, `completing`, `terminal`. `waiting` carries the set of tasks
  it depends on (`on`) and a join policy (`failFast` or `allSettled`) and runs no code until they are terminal.
  `completing` is the interesting one: the outcome is already decided, but the task becomes `terminal` — and
  `waitForTask()` returns — only once the ordinary work it owns has drained. Abort runs **bottom-up**: aborting
  a task aborts the work it owns first, and its own abort handler starts only after that, so each task undoes
  its own effects. A task created `background: true` is a boundary: its work survives the parent's abort and
  does not keep the parent busy.
- **Task outcomes**: `completed`, `failed`, `aborted`, `orphaned` (a blocked task with no live owned work left
  to settle it), `faulted`.
- **After a restart, tasks that were `running` show as `pending` until they run again** — the graph reports
  committed status only, and `harness.inspect()` is what answers "what is live and possibly blocked" for a
  recovery decision.

## 3. What is directly worth borrowing, in this repository's terms

Ordered by how much it would change a decision we have already made.

1. **Per-effect replay policy instead of a global "unknown".** Pico5 makes the question "may this run again
   after a crash?" a property of the *tool* (`replay: "never" | "safe"`), decided by whoever wrote the effect,
   with the intent committed first and a partial-output `interrupted` result as the model-facing answer. Here,
   a command's outcome after a runner kill is `OUTCOME_UNKNOWN` (D-119/D-88) and the policy is global. A
   per-tool/per-binding replay declaration is a small, testable change with the same guarantee, and it makes
   the safe cases usable again instead of parking them.
2. **`orphaned` as a first-class outcome, and `waiting {on, policy}`.** Our delegator waits and BLOCKED
   handoff (D-68, D-88) currently need the user to cancel a task so a wait is satisfied. Pico5's model says a
   blocked task with nothing live below it settles as `orphaned`, with a reason, and a waiting task resumes
   from its checkpoint when its dependencies are terminal — no user lever required for the mechanical case.
3. **`completing` as an explicit state.** "Outcome decided, terminal only when owned work drains" is exactly
   the invariant a team product needs (a delegator must not finish while its worker runs), and stating it as a
   state rather than as a check makes it auditable. Our supervisor's retire/terminate path and the TLA models
   around goals could adopt this shape.
4. **Watch/stream coalescing.** A slow consumer keeps at most 100 undelivered frames; beyond that the pending
   frames are replaced by one frame holding the newest whole view, and "a client that joins late or reconnects
   starts from the current view; nothing is replayed". Projections are committed at most every 100 ms, so a
   crash loses at most that window. Our events + watermark + TUI reconnect solve the same problem; the
   *coalescing rule* is the part we do not have in one sentence, and it is cheap to state.
5. **Compaction as a task with its own accounting**, a background trigger (`backgroundTokens`), a reserve
   (`reserveTokens`), a verbatim window (`keepRecentTokens`), and a **staleness rule**: a summary whose cut
   lands before the start of the current context settles as `stale` when placed, so with several in flight the
   furthest cut stays in effect. Our compaction is a code path, not a task, and the stale-summary rule is a
   correctness idea worth copying wherever two compactions can race.
6. **Extensions stored by name, resolved at use.** A conversation stores the *names* of the extensions, tools
   and agent choices it runs with; an uninstalled extension simply stops applying until it is installed again,
   and a stored name outlives its code. For us the analogue is profiles/skills/bindings; storing names and
   resolving per use is what would let a state root survive a tool set change without rewriting rows.
7. **The spec-plus-handoff pairing.** Pico5 ships a normative specification (records, invariants, state
   machines, ownership, document bases/checkpoints/versions/forks) *and* an implementation handoff with one
   section per layer. That is the same discipline this repository keeps with `docs/DESIGN.md` plus the
   verification report, and it is a good template for a new subsystem: normative section first, then the plan.

## 4. Pi 1.0.0 (the CLI) — what is new and relevant

From the 1.0.0 changelog:

- **Fullscreen TUI by default** (`tuiMode: "regular"` opts out) — a product decision about the terminal
  contract, not a protocol one.
- **Leaner codemode**: about **40 % fewer prompt tokens** (a GPT-5.6 request with the default tools drops from
  about 5,300 to 3,300), the tool description lists script globals one line each and points at a doc the model
  reads when it needs it, declared tools say in one line how scripts call them, and the MCP server section is
  shorter. **Errors now say how to recover**: an unknown tool or `models` member names the close matches, a
  malformed call rejects with the expected shape, an unknown model points at `models.getAvailableOfType()`.
  This is directly applicable to our codemode (D-374/D-376): our description is emitted from the same
  schemas, and "errors that tell the model how to recover" is a quality bar we have not set yet.
- **Image generation from codemode** (`models.generateImages()` with the session's credentials; usage counts
  toward session cost) and model classification calls (`models.classify()`). We have no script-visible model
  API at all, which is a real capability gap rather than a parity detail — our `image()` accepts only data URLs
  because v2 has no image context flow.
- **MCP hardening**: per-server-name-and-URL OAuth credentials, an `oauth.authServerMetadataUrl` override, an
  RFC 9207 `iss` check on the authorization response, step-up sign-in that keeps previously granted scopes, and
  `"auth": { "provider": ... }` to reuse a provider login token. Our MCP speaks stdio and streamable HTTP with a
  bearer token; the OAuth items are a concrete parity list.
- **Deferred MCP tools** no longer block the first prompt and are no longer listed in the tool description;
  scripts find them with `searchTools()`/`describeNamespace()`. We landed the same *shape* recently; the
  difference is that Pico5/Pi connect them in the background and wait only when a script names one.
- Smaller items: memory retained per rendered message cut to about a fifth on a long assistant message;
  prompt-submission cost no longer growing with session length; a stale-model-catalog lookup made non-quadratic.

## 5. What this does **not** say

- Pico5 is experimental and is **not** the harness the `pi` binary runs; nothing here is evidence about Pi's
  Terminal-Bench behaviour, which is a separate question this repository measures on its own harness.
- Pico5 has **no** analogue of our required-check boundary (a goal cannot settle on a failing user check, with
  bounded repair) or of the TLA+/Kani layer. On verification gating, this repository is ahead; on durable task
  modelling and on codemode ergonomics, it is ahead of us.
- The durability trade differs in kind: Pico5 chooses `synchronous = NORMAL` and single-process ownership
  without cross-process locking, and says so; we choose `synchronous = FULL` plus a coordinator lock. Neither
  is a defect; they are different answers to "how much may a host failure lose".

## 6. Open questions for this repository

1. Would a per-binding `replay` declaration let us answer a killed command more precisely than
   `OUTCOME_UNKNOWN`, without weakening the "never replay on a guess" rule?
2. Do our delegator waits want `orphaned` and `waiting {on, policy}` instead of the user-driven cancel lever?
3. Is codemode's prompt cost and error quality worth a refactor pass now that the mechanism exists?
4. Which MCP OAuth items are in scope for a product that speaks bearer tokens today?

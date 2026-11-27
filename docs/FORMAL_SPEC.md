# Formal Specification and Verification Contract

TeamAgents models task orchestration, collaborative coordination, compaction, streaming bounds, and action decoding as explicit state-transition systems verified by machine-checked model checking (`src/formal_tests.rs`) alongside end-to-end SQLite refinement checks.

## 1. Execution Tree & Convergence ($\mathcal{M}_{\text{tree}}$)

- **State**: `phase ∈ {Work, Check, ReportReady, Reporting, Complete}`, `round ≥ 1`, `active_root`, `tasks` (with `status ∈ {Queued, Running, Waiting, Approval, Done, Failed, Cancelled, Unknown}`), `operations` (with `state ∈ {Approval, Ready, Started, Done, Unknown, Cancelled}`), `check_attempts ∈ 0..=10`, and `continuation_node`.
- **Safety Invariants**:
  1. **One-way forward tree (`Inv_AcyclicTree`)**: A `handoff` closes its sender (`status = Done`) and creates `1..=64` children atomically within `(root, round)`. Across continuation rounds (`continue`), promoted continuation nodes satisfy `parent.root = root ∧ parent.round <= round ∧ parent.rowid < rowid`; descendants never target ancestors or siblings within a round, and each member takes at most one stage per `(root, round)`.
  2. **Convergence rule (`Inv_ConvergenceRule`)**: When all stages settle in `phase = Work` and verification passes (or is `not_requested`), a round with `1` terminal leaf settles directly at `Complete` with `continuation_node = Some(leaf_id)` (unless `outcome == Unknown`). A round with `≥ 2` terminal leaves transitions through `ReportReady -> Reporting -> Complete` via an internal `summarizer` node (`TurnKind::Reporter`) with zero workspace tools.
  3. **Authorized verification (`Inv_CheckAuthorization`, `Inv_AttemptCap`)**: When `check_command` is present, settlement into `Done` requires the latest `verify` operation on `(active_root, round)` matching the exact user-submitted command to be `Done` with `ok = true`. Each failed check dispatches a fresh forward repair stage (`status = Queued`) while `check_attempts < 10`, and stops cleanly into `Failed` at `check_attempts = 10`.
  4. **Unknown outcome absorption & cancellation fencing (`Inv_UnknownAbsorbing`, `Inv_NoReplay`)**: Any `Started` operation lost to crash (`recover()`) or active cancellation becomes `Unknown`, never replays, forces aggregate `outcome = Unknown` and `continuation_node = None`, and late worker/tool callbacks fail `discard_late_completion` without mutating settled or cancelled state.
- **Liveness / Termination**: Every reachable non-terminal state has a finite path to `Complete` (verified exhaustively via backward reachability in `formal_bfs_tree_check_repair_convergence_and_unknown_absorption`).

## 2. Collaborative Whiteboard & Deadlock Freedom ($\mathcal{M}_{\text{wb}}$)

- **State**: Per-root `wb_seq ≥ 0`, `entries: Key ⇀ (kind, content, version, seq, status ∈ {Active, Resolved})`, and per-task wait columns `(wb_wait_key, wb_wait_seq)`.
- **Safety Invariants**:
  1. **Monotonic sequence & version CAS (`Inv_WbLinearizable`)**: Every committed `set`, `append`, `resolve`, or `delete` increments `wb_seq` strictly and updates affected entry `seq = wb_seq`. `set` and `delete` with `expected_version = Some(v)` succeed iff `v == current_version`, returning a structured `conflict: true` receipt otherwise without mutating `wb_seq` or `entries`. `append` and `resolve` apply lock-free atomic read-modify-write inside the single-writer SQLite transaction.
  2. **Waiter cleanup (`Inv_WaiterStatusConsistency`)**: Every terminal or running stage (`status ∈ {Running, Done, Failed, Cancelled, Unknown}`) satisfies `wb_wait_key = NULL ∧ wb_wait_seq = NULL`.
  3. **Deadlock freedom (`Inv_WbDeadlockFreedom`)**: At every quiescent step end in `phase = Work`, if any task in the active round is parked on `wb_wait` (`status = Waiting ∧ wb_wait_seq IS NOT NULL`), then at least one other potential writer remains unparked in `(root, round)`:
     $$\text{waiters}(r, R) \neq \varnothing \implies \text{other\_active}(r, R) > 0.$$
     If an operation leaves `other_active(r, R) == 0` (through self-parking, completion, failure, or subtree cancellation), `wake_wb_waiters` immediately unblocks all remaining waiters with `woke = "no_other_writers"`. If a write satisfies a waiter filter (`wb_seq > wait_seq` and matching prefix/key or wildcard), `wake_wb_waiters` wakes it with `woke = "updated"`.
  4. **Trust boundary (`Inv_UnverifiedBoard`)**: Digest injection (`<= 4096` bytes) into `Store::claim` is transient (never persisted to `node_messages`) and Summarizer receives `whiteboard` entries strictly as unverified collaborative notes distinct from tool/verify receipts.

## 3. Auto-Compaction Termination ($\mathcal{M}_{\text{compact}}$)

- For persisted history length $L$: `prepare_compaction` accepts only $L \ge 4$ with `head_keep = min(2, L - 2)`, `tail_keep = 6` if $L \ge 10$ else `min(2, L - head_keep - 2)`, and `mid_end = L - tail_keep`, guarded by `mid_end > head_keep + 1`.
- Replacing `[head_keep..mid_end]` (length $d = \text{mid\_end} - \text{head\_keep} \ge 2$) with a single `checkpoint` message yields new length:
  $$L' = L - d + 1 \le L - 1 < L.$$
- **Theorem (`Thm_CompactionTerminates`)**: By well-founded induction on $L \in \mathbb{N}$, repeated compactions strictly decrease message count until reaching a fixed point $L \le 9$ (`< 6` for $L < 6$, or $L = 9$ when `head_keep = 2, tail_keep = 6` yields `mid_end = 3 <= head_keep + 1`) in finite steps $\le L$ (`formal_compaction_termination_and_progress_all_lengths`).

## 4. StreamBuffer Conservation & UTF-8 Safety ($\mathcal{M}_{\text{stream}}$)

- **State**: `(head, tail, omitted_bytes, omitted_lines)` with `HALF_OUTPUT = 65536` (`OUTPUT_LIMIT = 131072`).
- **Invariants (`Inv_StreamBoundedMemory`, `Inv_StreamConservation`, `Inv_StreamUtf8Safe`)**:
  1. `head.len() ≤ HALF_OUTPUT ∧ tail.len() ≤ HALF_OUTPUT`, so live memory never exceeds `OUTPUT_LIMIT`.
  2. Before finish: `head.len() + omitted_bytes + tail.len() == total_bytes_pushed`.
  3. On `finish`: trimming incomplete trailing lead/continuation bytes from `head` and leading continuation bytes from `tail` preserves exact byte conservation `head_end + omitted_bytes + tail_len == total_bytes_pushed` and emits valid UTF-8 with zero replacement characters (`\u{FFFD}`) for any valid UTF-8 stream under arbitrary chunking.

## 5. Boundary Contracts & Machine Verification Mapping

| Formal Module | Verified Properties | Checker in `src/formal_tests.rs` |
|---|---|---|
| $\mathcal{M}_{\text{wb}}$ | Deadlock freedom, CAS linearizability, wake soundness, reachability | `formal_bfs_whiteboard_deadlock_freedom_and_cas_linearizability` |
| $\mathcal{M}_{\text{tree}}$ | Check auth, 10-attempt cap, 1/2-leaf convergence, `Unknown` absorption | `formal_bfs_tree_check_repair_convergence_and_unknown_absorption` |
| SQLite Refinement | Every SQL transition preserves schema & state invariants | `formal_refinement_real_sqlite_store_preserves_all_invariants` |
| $\mathcal{M}_{\text{compact}}$ | Strict progress $L' \le L - 1$, finite termination | `formal_compaction_termination_and_progress_all_lengths` |
| $\mathcal{M}_{\text{stream}}$ | `128 KiB` bound, exact byte conservation, UTF-8 boundary safety | `formal_stream_buffer_conservation_and_utf8_safety` |
| Decoder Boundary | Role gating, clean vs prose fences, `i64` bounds, secret redaction | `formal_action_decoder_fence_and_redaction_soundness` |

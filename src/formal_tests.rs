//! Bounded explicit-state model checks and integration tests (`docs/FORMAL_SPEC.md`).
//! Combines exhaustive BFS over hand-written abstractions (safety + liveness backward
//! reachability), a scripted real-SQLite invariant scenario, bounded compaction checks,
//! sampled stream conservation tests, and sampled decoder/redaction checks. None of
//! these is a refinement proof of the production implementation.

use crate::{
    model::{self, ModelError, ModelSettings},
    protocol::TurnKind,
    store::{compute_compaction_slice, Store},
    tools::{StreamBuffer, HALF_OUTPUT, OUTPUT_LIMIT},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::PathBuf,
};
use uuid::Uuid;

// ============================================================================
// 1. Exhaustive BFS Model Checker: Collaborative Whiteboard Deadlock Freedom
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum WaitFilter {
    All,
    PrefixK0,
    ExactK1,
}

impl WaitFilter {
    fn matches_key(self, key: usize) -> bool {
        match self {
            Self::All => true,
            Self::PrefixK0 => key == 0,
            Self::ExactK1 => key == 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum LeafState {
    Queued,
    Running,
    ToolApproval,
    ToolReady,
    WbWait { filter: WaitFilter, wait_seq: u8 },
    Done,
    Failed,
    Cancelled,
    Unknown,
}

impl LeafState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled | Self::Unknown)
    }

    fn is_other_writer(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::ToolApproval | Self::ToolReady)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum EntryStatus {
    Active,
    Resolved,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Entry {
    version: u8,
    seq: u8,
    status: EntryStatus,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum RoundPhase {
    Work,
    Complete,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct WbModelState {
    phase: RoundPhase,
    wb_seq: u8,
    writes_left: u8,
    entries: [Option<Entry>; 2],
    leaves: Vec<LeafState>,
}

impl WbModelState {
    fn new(num_leaves: usize, max_writes: u8) -> Self {
        Self {
            phase: RoundPhase::Work,
            wb_seq: 0,
            writes_left: max_writes,
            entries: [None, None],
            leaves: vec![LeafState::Queued; num_leaves],
        }
    }

    fn assert_invariants(&self) {
        // Every entry seq <= global wb_seq and version >= 1.
        for e in self.entries.iter().flatten() {
            assert!(e.version >= 1 && e.seq >= 1 && e.seq <= self.wb_seq, "entry seq/version out of bounds: {self:?}");
        }
        if self.phase == RoundPhase::Work {
            // Active round cannot have all leaves settled.
            assert!(
                self.leaves.iter().any(|l| !l.is_terminal()),
                "Work phase must have at least one non-terminal leaf: {self:?}"
            );
            // Inv_WbDeadlockFreedom: if any waiter exists, at least one potential writer remains unparked.
            let any_waiter = self.leaves.iter().any(|l| matches!(l, LeafState::WbWait { .. }));
            if any_waiter {
                let other_active = self.leaves.iter().filter(|l| l.is_other_writer()).count();
                assert!(other_active > 0, "DEADLOCK DETECTED: waiters exist with zero active writers: {self:?}");
            }
        } else {
            // Complete phase has zero active or waiting tasks.
            assert!(self.leaves.iter().all(|l| l.is_terminal()), "Complete phase has non-terminal leaf: {self:?}");
        }
    }

    /// Mirrors `tree::wake_wb_waiters` + `tree::advance` atomically.
    fn wake_and_advance(&mut self) {
        if self.phase != RoundPhase::Work {
            return;
        }
        let has_waiter = self.leaves.iter().any(|l| matches!(l, LeafState::WbWait { .. }));
        if has_waiter {
            let mut woke_any = false;
            for leaf in &mut self.leaves {
                if let LeafState::WbWait { filter, wait_seq } = *leaf {
                    if self.wb_seq > wait_seq {
                        let any_match = self
                            .entries
                            .iter()
                            .enumerate()
                            .any(|(k, opt)| opt.is_some_and(|e| e.seq > wait_seq && filter.matches_key(k)));
                        if any_match || matches!(filter, WaitFilter::All) {
                            *leaf = LeafState::Queued;
                            woke_any = true;
                        }
                    }
                }
            }
            if !woke_any && self.leaves.iter().filter(|l| l.is_other_writer()).count() == 0 {
                for leaf in &mut self.leaves {
                    if matches!(leaf, LeafState::WbWait { .. }) {
                        *leaf = LeafState::Queued;
                    }
                }
            }
        }
        if self.leaves.iter().all(|l| l.is_terminal()) {
            self.phase = RoundPhase::Complete;
        }
    }

    fn apply_wb_write(&self, actor: usize, key: usize, op: WbOp) -> Option<Self> {
        if self.wb_seq >= 7 || self.writes_left == 0 {
            return None;
        }
        let mut next = self.clone();
        let cur_ver = next.entries[key].map_or(0, |e| e.version);
        match op {
            WbOp::Set { cas } => {
                if let Some(exp) = cas {
                    if exp != cur_ver {
                        // Conflict leaves wb_seq and entries unmodified; worker returns to Queued.
                        assert_eq!(next.entries, self.entries);
                        next.leaves[actor] = LeafState::Queued;
                        return Some(next);
                    }
                }
                next.wb_seq += 1;
                next.writes_left -= 1;
                next.entries[key] = Some(Entry { version: cur_ver + 1, seq: next.wb_seq, status: EntryStatus::Active });
            }
            WbOp::Append => {
                next.wb_seq += 1;
                next.writes_left -= 1;
                next.entries[key] = Some(Entry { version: cur_ver + 1, seq: next.wb_seq, status: EntryStatus::Active });
            }
            WbOp::Resolve => {
                let e = next.entries[key]?;
                next.wb_seq += 1;
                next.writes_left -= 1;
                next.entries[key] =
                    Some(Entry { version: e.version + 1, seq: next.wb_seq, status: EntryStatus::Resolved });
            }
            WbOp::Delete { cas } => {
                let _e = next.entries[key]?;
                if let Some(exp) = cas {
                    if exp != cur_ver {
                        assert_eq!(next.entries, self.entries);
                        next.leaves[actor] = LeafState::Queued;
                        return Some(next);
                    }
                }
                next.wb_seq += 1;
                next.writes_left -= 1;
                next.entries[key] = None;
            }
        }
        next.leaves[actor] = LeafState::Queued;
        next.wake_and_advance();
        Some(next)
    }

    fn successors(&self) -> Vec<Self> {
        if self.phase == RoundPhase::Complete {
            return Vec::new();
        }
        let mut out = Vec::new();
        let filters = if self.leaves.len() <= 2 {
            [WaitFilter::All, WaitFilter::PrefixK0, WaitFilter::ExactK1].as_slice()
        } else {
            [WaitFilter::All, WaitFilter::PrefixK0].as_slice()
        };
        let key_limit = if self.leaves.len() <= 2 { 2 } else { 1 };

        for i in 0..self.leaves.len() {
            match self.leaves[i] {
                LeafState::Queued => {
                    let mut s = self.clone();
                    s.leaves[i] = LeafState::Running;
                    out.push(s);
                }
                LeafState::Running => {
                    // Finish / Fail
                    for term in [LeafState::Done, LeafState::Failed] {
                        let mut s = self.clone();
                        s.leaves[i] = term;
                        s.wake_and_advance();
                        out.push(s);
                    }
                    // Tool request (auto -> Ready, review -> Approval)
                    for tool in [LeafState::ToolReady, LeafState::ToolApproval] {
                        let mut s = self.clone();
                        s.leaves[i] = tool;
                        out.push(s);
                    }
                    // wb_wait with current or stale wait_seq
                    for &filter in filters {
                        let mut seqs = vec![self.wb_seq];
                        if self.wb_seq > 0 {
                            seqs.push(self.wb_seq - 1);
                        }
                        for wait_seq in seqs {
                            let mut s = self.clone();
                            s.leaves[i] = LeafState::WbWait { filter, wait_seq };
                            s.wake_and_advance();
                            out.push(s);
                        }
                    }
                    // wb_write operations (set with CAS hit/miss/unconditional, append, resolve, delete)
                    for k in 0..key_limit {
                        let cur_v = self.entries[k].map_or(0, |e| e.version);
                        let ops = [
                            WbOp::Set { cas: None },
                            WbOp::Set { cas: Some(cur_v) },
                            WbOp::Set { cas: Some(cur_v.saturating_add(1)) },
                            WbOp::Append,
                            WbOp::Resolve,
                            WbOp::Delete { cas: None },
                            WbOp::Delete { cas: Some(cur_v) },
                            WbOp::Delete { cas: Some(cur_v.saturating_add(1)) },
                        ];
                        for op in ops {
                            if let Some(s) = self.apply_wb_write(i, k, op) {
                                out.push(s);
                            }
                        }
                    }
                }
                LeafState::ToolApproval => {
                    let mut approve = self.clone();
                    approve.leaves[i] = LeafState::ToolReady;
                    out.push(approve);
                    let mut deny = self.clone();
                    deny.leaves[i] = LeafState::Queued;
                    out.push(deny);
                }
                LeafState::ToolReady => {
                    let mut ok = self.clone();
                    ok.leaves[i] = LeafState::Queued;
                    out.push(ok);
                    let mut lost = self.clone();
                    lost.leaves[i] = LeafState::Unknown;
                    lost.wake_and_advance();
                    out.push(lost);
                }
                LeafState::WbWait { .. }
                | LeafState::Done
                | LeafState::Failed
                | LeafState::Cancelled
                | LeafState::Unknown => {}
            }
            // User subtree cancel of any non-terminal leaf (models subtree/branch cancel triggering wake_wb_waiters).
            if !self.leaves[i].is_terminal() {
                let mut cancel_leaf = self.clone();
                cancel_leaf.leaves[i] = LeafState::Cancelled;
                cancel_leaf.wake_and_advance();
                out.push(cancel_leaf);
            }
        }

        // Whole-root cancel settles the entire round cleanly.
        let mut cancel_root = self.clone();
        for l in &mut cancel_root.leaves {
            if !l.is_terminal() {
                *l = LeafState::Cancelled;
            }
        }
        cancel_root.phase = RoundPhase::Complete;
        out.push(cancel_root);

        // Daemon crash + `Store::recover()` (`Running -> Queued`, waits preserved except lost active writes trigger wakeup).
        if self.leaves.iter().any(|l| matches!(l, LeafState::Running | LeafState::ToolReady | LeafState::WbWait { .. }))
        {
            let mut rec = self.clone();
            for l in &mut rec.leaves {
                match l {
                    LeafState::Running => *l = LeafState::Queued,
                    LeafState::ToolReady => *l = LeafState::Unknown,
                    _ => {}
                }
            }
            rec.wake_and_advance();
            out.push(rec);
        }

        out
    }
}

#[derive(Clone, Copy)]
enum WbOp {
    Set { cas: Option<u8> },
    Append,
    Resolve,
    Delete { cas: Option<u8> },
}

fn run_exhaustive_bfs<S, F>(init: &[S], succ: F, check: impl Fn(&S), is_terminal: impl Fn(&S) -> bool) -> (usize, usize)
where
    S: Clone + Eq + std::hash::Hash + std::fmt::Debug,
    F: Fn(&S) -> Vec<S>,
{
    let mut visited: HashMap<S, usize> = HashMap::new();
    let mut states: Vec<S> = Vec::new();
    let mut rev_adj: Vec<Vec<usize>> = Vec::new();
    let mut queue = VecDeque::new();

    for s in init {
        if !visited.contains_key(s) {
            let id = states.len();
            visited.insert(s.clone(), id);
            states.push(s.clone());
            rev_adj.push(Vec::new());
            queue.push_back(id);
        }
    }

    while let Some(u) = queue.pop_front() {
        let su = states[u].clone();
        check(&su);
        for sv in succ(&su) {
            let v = if let Some(&existing) = visited.get(&sv) {
                existing
            } else {
                let id = states.len();
                visited.insert(sv.clone(), id);
                states.push(sv);
                rev_adj.push(Vec::new());
                queue.push_back(id);
                id
            };
            rev_adj[v].push(u);
        }
    }

    // Backward reachability: 100% of reachable states can reach a terminal `Complete` state.
    let mut can_finish = vec![false; states.len()];
    let mut back_q = VecDeque::new();
    for (idx, s) in states.iter().enumerate() {
        if is_terminal(s) {
            can_finish[idx] = true;
            back_q.push_back(idx);
        }
    }
    while let Some(v) = back_q.pop_front() {
        for &u in &rev_adj[v] {
            if !can_finish[u] {
                can_finish[u] = true;
                back_q.push_back(u);
            }
        }
    }
    if let Some((idx, bad)) = states.iter().enumerate().find(|(i, _)| !can_finish[*i]) {
        panic!("LIVENESS VIOLATION at state #{idx}: cannot reach Complete: {bad:?}");
    }
    let terminals = states.iter().filter(|s| is_terminal(s)).count();
    (states.len(), terminals)
}

#[test]
fn formal_bfs_whiteboard_deadlock_freedom_and_cas_linearizability() {
    // Model A: 2 sibling leaves, 2 keys, write budget = 3, full CAS + approval + tool loss + crash recover.
    let (states_2, term_2) = run_exhaustive_bfs(
        &[WbModelState::new(2, 3)],
        WbModelState::successors,
        WbModelState::assert_invariants,
        |s| s.phase == RoundPhase::Complete,
    );
    assert!(states_2 >= 2_000 && term_2 >= 50, "unexpectedly tiny 2-leaf model: states={states_2}, term={term_2}");

    // Model B: 3 sibling leaves, write budget = 2, verifying multi-waiter cascading deadlock prevention.
    let (states_3, term_3) = run_exhaustive_bfs(
        &[WbModelState::new(3, 2)],
        WbModelState::successors,
        WbModelState::assert_invariants,
        |s| s.phase == RoundPhase::Complete,
    );
    println!("wb_bfs: n2=({states_2} states, {term_2} terminal), n3=({states_3} states, {term_3} terminal)");
    assert!(states_3 >= 2_000 && term_3 >= 50, "unexpectedly tiny 3-leaf model: states={states_3}, term={term_3}");
}

// ============================================================================
// 2. Exhaustive BFS Model Checker: Tree Lifecycle, Check Authorization & Unknown
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum TreePhase {
    RootWork,
    LeavesWork,
    CheckPending,
    CheckStarted,
    RepairQueued,
    RepairRunning,
    ReportReady,
    Reporting,
    Complete,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum VerifyState {
    None,
    Ready,
    Started,
    Passed,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Outcome {
    Active,
    Done,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Stage {
    Queued,
    Running,
    OpStarted,
    Done,
    Failed,
    Cancelled,
    Unknown,
}

impl Stage {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled | Self::Unknown)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct TreeLifecycleState {
    has_check: bool,
    multi_leaf: bool,
    phase: TreePhase,
    root: Stage,
    leaves: [Stage; 2],
    check_attempts: u8,
    verify: VerifyState,
    reporter_used: bool,
    outcome: Outcome,
    can_continue: bool,
}

impl TreeLifecycleState {
    fn roots() -> Vec<Self> {
        let mut init = Vec::new();
        for has_check in [false, true] {
            for multi_leaf in [false, true] {
                init.push(Self {
                    has_check,
                    multi_leaf,
                    phase: TreePhase::RootWork,
                    root: Stage::Queued,
                    leaves: [Stage::Queued; 2],
                    check_attempts: 0,
                    verify: VerifyState::None,
                    reporter_used: false,
                    outcome: Outcome::Active,
                    can_continue: false,
                });
            }
        }
        init
    }

    fn settle_outcome(&self, base: Outcome) -> Outcome {
        if self.root == Stage::Unknown
            || self.leaves.iter().take(if self.multi_leaf { 2 } else { 1 }).any(|&s| s == Stage::Unknown)
            || self.verify == VerifyState::Unknown
        {
            Outcome::Unknown
        } else {
            base
        }
    }

    fn enter_settle_or_report(mut self, base: Outcome, can_cont_leaves: bool) -> Self {
        let eff = self.settle_outcome(base);
        if self.multi_leaf && can_cont_leaves {
            self.phase = TreePhase::ReportReady;
            self.outcome = eff;
            self.can_continue = false;
        } else {
            self.phase = TreePhase::Complete;
            self.outcome = eff;
            // Single leaf (or finished root) can continue iff outcome != Unknown; whole-root cancel of 2 leaves -> None.
            self.can_continue = eff != Outcome::Unknown && (!self.multi_leaf || self.reporter_used);
        }
        self
    }

    fn check_invariants(&self) {
        assert!(self.check_attempts <= 10, "Inv_AttemptCap violated: {self:?}");
        if self.has_check && self.phase == TreePhase::Complete && self.outcome == Outcome::Done {
            assert_eq!(self.verify, VerifyState::Passed, "Inv_CheckAuthorization violated: {self:?}");
            assert!(self.check_attempts >= 1, "verified run must have >=1 attempt: {self:?}");
        }
        if self.phase == TreePhase::Complete {
            assert!(self.root.is_terminal(), "Inv_CompleteStagesTerminal violated: root still active: {self:?}");
            if self.multi_leaf {
                assert!(
                    self.leaves.iter().all(|s| s.is_terminal()),
                    "Inv_CompleteStagesTerminal violated: leaf still active: {self:?}"
                );
            }
            if !self.multi_leaf {
                assert!(!self.reporter_used, "Inv_ConvergenceRule violated: 1 leaf invoked reporter: {self:?}");
            }
            if self.outcome == Outcome::Unknown {
                assert!(!self.can_continue, "Inv_UnknownAbsorbing violated: unknown allows continuation: {self:?}");
            }
        }
        if self.verify == VerifyState::Unknown && self.phase == TreePhase::Complete {
            assert_eq!(self.outcome, Outcome::Unknown, "unknown check failed to force Outcome::Unknown: {self:?}");
        }
    }

    fn successors(&self) -> Vec<Self> {
        let mut out = Vec::new();
        match self.phase {
            TreePhase::RootWork => match self.root {
                Stage::Queued => {
                    let mut s = *self;
                    s.root = Stage::Running;
                    out.push(s);
                }
                Stage::Running => {
                    // Finish directly (1 leaf at root).
                    let mut fin = *self;
                    fin.root = Stage::Done;
                    fin.multi_leaf = false;
                    if fin.has_check {
                        fin.phase = TreePhase::CheckPending;
                        fin.verify = VerifyState::Ready;
                        out.push(fin);
                    } else {
                        out.push(fin.enter_settle_or_report(Outcome::Done, true));
                    }
                    // Handoff to 1 or 2 leaves (configured leaf count).
                    let mut ho = *self;
                    ho.root = Stage::Done;
                    ho.phase = TreePhase::LeavesWork;
                    out.push(ho);
                    // Start local tool.
                    let mut op = *self;
                    op.root = Stage::OpStarted;
                    out.push(op);
                    // Crash recover -> Queued.
                    let mut rec = *self;
                    rec.root = Stage::Queued;
                    out.push(rec);
                }
                Stage::OpStarted => {
                    let mut done = *self;
                    done.root = Stage::Queued;
                    out.push(done);
                    let mut lost = *self;
                    lost.root = Stage::Unknown;
                    lost.multi_leaf = false;
                    out.push(lost.enter_settle_or_report(Outcome::Unknown, true));
                }
                _ => {}
            },
            TreePhase::LeavesWork => {
                let n = if self.multi_leaf { 2 } else { 1 };
                for i in 0..n {
                    match self.leaves[i] {
                        Stage::Queued => {
                            let mut s = *self;
                            s.leaves[i] = Stage::Running;
                            out.push(s);
                        }
                        Stage::Running => {
                            for term in [Stage::Done, Stage::Failed, Stage::Cancelled] {
                                let mut s = *self;
                                s.leaves[i] = term;
                                if s.leaves[..n].iter().all(|l| l.is_terminal()) {
                                    if s.leaves[..n].contains(&Stage::Unknown) {
                                        out.push(s.enter_settle_or_report(Outcome::Unknown, true));
                                    } else if s.leaves[..n]
                                        .iter()
                                        .any(|&l| matches!(l, Stage::Failed | Stage::Cancelled))
                                    {
                                        out.push(s.enter_settle_or_report(Outcome::Failed, true));
                                    } else if s.has_check {
                                        s.phase = TreePhase::CheckPending;
                                        s.verify = VerifyState::Ready;
                                        out.push(s);
                                    } else {
                                        out.push(s.enter_settle_or_report(Outcome::Done, true));
                                    }
                                } else {
                                    out.push(s);
                                }
                            }
                            let mut op = *self;
                            op.leaves[i] = Stage::OpStarted;
                            out.push(op);
                        }
                        Stage::OpStarted => {
                            let mut ok = *self;
                            ok.leaves[i] = Stage::Queued;
                            out.push(ok);
                            let mut lost = *self;
                            lost.leaves[i] = Stage::Unknown;
                            if lost.leaves[..n].iter().all(|l| l.is_terminal()) {
                                out.push(lost.enter_settle_or_report(Outcome::Unknown, true));
                            } else {
                                out.push(lost);
                            }
                        }
                        _ => {}
                    }
                }
            }
            TreePhase::CheckPending => {
                let mut claim = *self;
                claim.phase = TreePhase::CheckStarted;
                claim.verify = VerifyState::Started;
                out.push(claim);
            }
            TreePhase::CheckStarted => {
                // Check passes.
                let mut pass = *self;
                pass.check_attempts += 1;
                pass.verify = VerifyState::Passed;
                out.push(pass.enter_settle_or_report(Outcome::Done, true));
                // Check fails: spawn repair up to 10 total attempts, else fail.
                let mut fail = *self;
                fail.check_attempts += 1;
                fail.verify = VerifyState::Failed;
                if fail.check_attempts < 10 {
                    fail.phase = TreePhase::RepairQueued;
                    out.push(fail);
                } else {
                    out.push(fail.enter_settle_or_report(Outcome::Failed, true));
                }
                // Crash during Started check -> Unknown (never replays check!).
                let mut rec = *self;
                rec.verify = VerifyState::Unknown;
                out.push(rec.enter_settle_or_report(Outcome::Unknown, true));
            }
            TreePhase::RepairQueued => {
                let mut s = *self;
                s.phase = TreePhase::RepairRunning;
                out.push(s);
            }
            TreePhase::RepairRunning => {
                let mut fin = *self;
                fin.phase = TreePhase::CheckPending;
                fin.verify = VerifyState::Ready;
                out.push(fin);
                let fail = *self;
                out.push(fail.enter_settle_or_report(Outcome::Failed, true));
                let mut unk = *self;
                unk.root = Stage::Unknown;
                out.push(unk.enter_settle_or_report(Outcome::Unknown, true));
            }
            TreePhase::ReportReady => {
                let mut claim = *self;
                claim.phase = TreePhase::Reporting;
                claim.reporter_used = true;
                out.push(claim);
            }
            TreePhase::Reporting => {
                // Normal completion vs failure/crash fallback (`outcome` preserved).
                let mut done = *self;
                done.phase = TreePhase::Complete;
                done.can_continue = done.outcome != Outcome::Unknown;
                out.push(done);
            }
            TreePhase::Complete => {}
        }

        // User cancellation is a separate external transition, not an unconditional edge
        // from every state. Including it here would make the liveness check vacuous.
        out
    }

    fn cancelled(&self) -> Option<Self> {
        if self.phase == TreePhase::Complete {
            return None;
        }
        let has_started_op = matches!(self.phase, TreePhase::CheckStarted)
            || self.root == Stage::OpStarted
            || self.leaves.contains(&Stage::OpStarted);
        let can_cont_after_cancel = !self.multi_leaf || self.reporter_used;
        let mut c = *self;
        c.phase = TreePhase::Complete;
        // A terminal round may not leave Running/Queued stages behind. Started work is
        // unknown; the rest is cancelled.
        for stage in std::iter::once(&mut c.root).chain(c.leaves.iter_mut()) {
            *stage = match *stage {
                Stage::OpStarted => Stage::Unknown,
                stage if !stage.is_terminal() => Stage::Cancelled,
                stage => stage,
            };
        }
        if has_started_op {
            c.verify = VerifyState::Unknown;
            c.outcome = Outcome::Unknown;
            c.can_continue = false;
        } else {
            c.outcome = c.settle_outcome(Outcome::Cancelled);
            c.can_continue = c.outcome != Outcome::Unknown && can_cont_after_cancel;
        }
        Some(c)
    }
}

#[test]
fn formal_bfs_tree_check_repair_convergence_and_unknown_absorption() {
    let (states, terminals) = run_exhaustive_bfs(
        &TreeLifecycleState::roots(),
        TreeLifecycleState::successors,
        TreeLifecycleState::check_invariants,
        |s| s.phase == TreePhase::Complete,
    );
    println!("tree_bfs: ({states} states, {terminals} terminal)");
    assert!(
        states >= 450 && terminals >= 150,
        "unexpectedly tiny tree lifecycle model: states={states}, terminals={terminals}"
    );
}

#[test]
fn formal_tree_cancellation_is_fenced_and_settles_all_stages() {
    // Cancellation is an external action and may race any non-terminal phase. Every
    // resulting Complete state must have terminal stages and satisfy Unknown fencing.
    for phase in [
        TreePhase::RootWork,
        TreePhase::LeavesWork,
        TreePhase::CheckPending,
        TreePhase::CheckStarted,
        TreePhase::RepairQueued,
        TreePhase::RepairRunning,
        TreePhase::ReportReady,
        TreePhase::Reporting,
    ] {
        let mut state = TreeLifecycleState::roots()[1];
        state.phase = phase;
        state.root = Stage::Done;
        state.leaves = [Stage::Running, Stage::Queued];
        state.multi_leaf = true;
        state.outcome = Outcome::Active;
        if let Some(cancelled) = state.cancelled() {
            cancelled.check_invariants();
            assert_eq!(cancelled.phase, TreePhase::Complete);
            assert!(cancelled.root.is_terminal());
            assert!(cancelled.leaves.iter().all(|stage| stage.is_terminal()));
        }
    }
}

// ============================================================================
// 3. Scripted Invariant Checks on the Real SQLite Store (not a refinement proof)
// ============================================================================

struct ScratchDir(PathBuf);
impl ScratchDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("swarm-formal-{}", Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn open(&self) -> Store {
        Store::open(&self.0.join("swarm.db")).unwrap()
    }
}
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_store_invariants(db: &Connection, step: &str) {
    // 1. Every tree task references a valid tree_runs & root_policies row, and parents stay in the same (root, round) with smaller rowid.
    let bad_parent: i64 = db
        .query_row(
            "SELECT count(*) FROM tasks t WHERE t.parent IS NOT NULL AND NOT EXISTS(
                 SELECT 1 FROM tasks p WHERE p.id=t.parent AND p.root=t.root AND p.round<=t.round AND p.rowid < t.rowid
             )",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_parent, 0, "[{step}] Inv_AcyclicTree violated");

    let active_in_complete: i64 = db
        .query_row(
            "SELECT count(*) FROM tree_runs g JOIN tasks t ON t.root=g.root
             WHERE g.phase='complete' AND t.status IN ('queued','running','waiting','approval')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(active_in_complete, 0, "[{step}] Inv_CompleteStagesTerminal violated");

    let duplicate_member: i64 = db
        .query_row(
            "SELECT count(*) FROM (SELECT root,round,agent FROM tasks WHERE agent NOT LIKE 'repair-%'
             GROUP BY root,round,agent HAVING count(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(duplicate_member, 0, "[{step}] Inv_OneStagePerMember violated");

    let oversized_fanout: i64 = db
        .query_row(
            "SELECT count(*) FROM (SELECT parent FROM tasks WHERE parent IS NOT NULL AND round=(SELECT round FROM tasks p WHERE p.id=tasks.parent)
             GROUP BY parent HAVING count(*) > 64)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(oversized_fanout, 0, "[{step}] Inv_HandoffCap violated");

    // 2. Waiter status consistency: non-waiting tasks never retain wb_wait_* fields.
    let bad_waiter: i64 = db
        .query_row(
            "SELECT count(*) FROM tasks WHERE (wb_wait_key IS NOT NULL OR wb_wait_seq IS NOT NULL) AND status <> 'waiting'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_waiter, 0, "[{step}] Inv_WaiterStatusConsistency violated");

    // 3. Inv_WbDeadlockFreedom across every active root round.
    let deadlocked: i64 = db
        .query_row(
            "SELECT count(*) FROM tree_runs g WHERE g.phase='work'
             AND EXISTS(SELECT 1 FROM tasks w WHERE w.root=g.root AND w.round=g.round AND w.status='waiting' AND w.wb_wait_seq IS NOT NULL)
             AND NOT EXISTS(SELECT 1 FROM tasks a WHERE a.root=g.root AND a.round=g.round
                 AND (a.status IN ('queued','running','approval') OR (a.status='waiting' AND a.wb_wait_seq IS NULL)))",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(deadlocked, 0, "[{step}] Inv_WbDeadlockFreedom violated");

    // 4. Monotonic whiteboard versions & sequences.
    let bad_wb: i64 = db
        .query_row(
            "SELECT count(*) FROM whiteboard_entries w JOIN tree_runs g ON g.root=w.root
             WHERE w.version < 1 OR w.seq < 1 OR w.seq > g.wb_seq",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_wb, 0, "[{step}] Inv_WbLinearizable violated");

    // 5. Authorized verification gating: checked done root must have latest verify passing and <= 10 verify attempts.
    let bad_check: i64 = db
        .query_row(
            "SELECT count(*) FROM tree_runs g
             JOIN tasks r ON r.id=COALESCE(g.active_root, g.root)
             WHERE g.phase='complete' AND g.outcome='done' AND r.check_command IS NOT NULL
             AND COALESCE((SELECT ok FROM operations o WHERE o.task=r.id AND o.round=g.round AND json_extract(o.action,'$.op')='verify' ORDER BY o.rowid DESC LIMIT 1), 0) <> 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_check, 0, "[{step}] Inv_CheckAuthorization violated");

    let too_many_checks: i64 = db
        .query_row(
            "SELECT count(*) FROM (
                 SELECT o.task, o.round, count(*) AS cnt FROM operations o
                 WHERE json_extract(o.action,'$.op')='verify' GROUP BY o.task, o.round
             ) WHERE cnt > 10",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(too_many_checks, 0, "[{step}] Inv_AttemptCap violated");

    // 6. Unknown-outcome absorption: complete runs with outcome='unknown' must never expose continuation_node.
    let bad_unknown: i64 = db
        .query_row(
            "SELECT count(*) FROM tree_runs WHERE phase='complete' AND outcome='unknown' AND continuation_node IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_unknown, 0, "[{step}] Inv_UnknownAbsorbing violated");

    // 7. Summarizers (reporters) never own workspace operations.
    let bad_reporter_op: i64 = db
        .query_row("SELECT count(*) FROM operations o JOIN tasks t ON t.id=o.task WHERE t.kind='summarizer'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(bad_reporter_op, 0, "[{step}] Summarizer tool isolation violated");
}

#[test]
fn formal_sqlite_integration_scripted_invariants() {
    let dir = ScratchDir::new();
    let mut s = dir.open();

    // Scenario A: Checked 3-leaf collaborative round with concurrent waiters, CAS conflict, partial subtree cancel, check fail -> repair -> verify pass -> summarizer -> continue -> compaction -> delete.
    let root = s.submit_with_policy("default", "refinement test", Some("cargo test"), true).unwrap();
    assert_store_invariants(s.conn(), "submit root");

    s.claim().unwrap().unwrap();
    assert_store_invariants(s.conn(), "claim root");

    s.apply(
        &root,
        &json!({
            "op":"handoff",
            "text":"fan out 3",
            "children":[
                {"agent":"alpha","text":"write contract"},
                {"agent":"beta","text":"wait for contract"},
                {"agent":"gamma","text":"extra waiter"}
            ]
        }),
    )
    .unwrap();
    assert_store_invariants(s.conn(), "handoff 3 children");

    let t_a = s.claim().unwrap().unwrap();
    let t_b = s.claim().unwrap().unwrap();
    let t_c = s.claim().unwrap().unwrap();
    assert_store_invariants(s.conn(), "claim 3 leaves");

    // Beta and Gamma both park on wb_wait while Alpha remains running.
    s.apply(&t_b.task, &json!({"op":"wb_wait","key":"contract:"})).unwrap();
    assert_store_invariants(s.conn(), "beta wb_wait");
    s.apply(&t_c.task, &json!({"op":"wb_wait","key":"never:"})).unwrap();
    assert_store_invariants(s.conn(), "gamma wb_wait");

    // Alpha triggers CAS conflict first, then writes valid contract waking Beta.
    s.apply(
        &t_a.task,
        &json!({"op":"wb_write","key":"contract:v1","mode":"set","kind":"contract","content":"bad","expected_version":1}),
    )
    .unwrap();
    assert_store_invariants(s.conn(), "alpha cas conflict");
    s.claim().unwrap().unwrap();

    s.apply(
        &t_a.task,
        &json!({"op":"wb_write","key":"contract:v1","mode":"set","kind":"contract","content":"v1","expected_version":0}),
    )
    .unwrap();
    assert_store_invariants(s.conn(), "alpha wb_write wakes beta");

    // Alpha requests an edit in review mode, gets approved and executed, then finishes.
    s.claim().unwrap().unwrap();
    s.request_tool(&t_a.task, &json!({"op":"edit_file","path":"a.txt","old":"","new":"hi"})).unwrap();
    assert_store_invariants(s.conn(), "alpha request_tool review");
    let appr = s.task(&t_a.task).unwrap()["approval_id"].as_str().unwrap().to_owned();
    s.approve(&appr, true).unwrap();
    assert_store_invariants(s.conn(), "approve alpha tool");
    let job = s.claim_tool_job().unwrap().unwrap();
    s.tool_result(&job.task, Ok("applied")).unwrap();
    assert_store_invariants(s.conn(), "tool_result alpha");
    s.claim().unwrap().unwrap();
    s.apply(&t_a.task, &json!({"op":"finish","text":"alpha done"})).unwrap();
    assert_store_invariants(s.conn(), "alpha finish");

    // Woken Beta finishes; Gamma must auto-wake immediately with `no_other_writers`!
    s.claim().unwrap().unwrap();
    s.apply(&t_b.task, &json!({"op":"finish","text":"beta done"})).unwrap();
    assert_store_invariants(s.conn(), "beta finish wakes gamma");

    let gamma_resumed = s.claim().unwrap().unwrap();
    assert_eq!(gamma_resumed.task, t_c.task);
    assert!(gamma_resumed
        .messages
        .iter()
        .any(|m| m["content"].as_str().is_some_and(|c| c.contains("no_other_writers"))));
    s.apply(&t_c.task, &json!({"op":"finish","text":"gamma done"})).unwrap();
    assert_store_invariants(s.conn(), "gamma finish enters check");

    // Check attempt 1 fails -> repair stage -> check attempt 2 passes -> summarizer.
    let v1 = s.claim_tool_job().unwrap().unwrap();
    s.tool_result(&v1.task, Err("test failed once")).unwrap();
    assert_store_invariants(s.conn(), "verify 1 failed");

    let repair = s.claim().unwrap().unwrap();
    assert_eq!(repair.kind, TurnKind::Node);
    s.apply(&repair.task, &json!({"op":"finish","text":"repaired"})).unwrap();
    assert_store_invariants(s.conn(), "repair finish");

    let v2 = s.claim_tool_job().unwrap().unwrap();
    s.tool_result(&v2.task, Ok("all passed")).unwrap();
    assert_store_invariants(s.conn(), "verify 2 passed");

    let sum = s.claim().unwrap().unwrap();
    assert_eq!(sum.kind, TurnKind::Reporter);
    s.apply(&sum.task, &json!({"op":"finish","text":"round 1 report"})).unwrap();
    assert_store_invariants(s.conn(), "summarizer complete");

    // Continue from summarizer and clean delete.
    s.continue_tree(&sum.task, "round 2", None, false, &ModelSettings::default()).unwrap();
    assert_store_invariants(s.conn(), "continue_tree round 2");
    s.claim().unwrap().unwrap();
    s.apply(&sum.task, &json!({"op":"finish","text":"round 2 done"})).unwrap();
    assert_store_invariants(s.conn(), "round 2 complete");
    s.delete(&root).unwrap();
    assert_store_invariants(s.conn(), "delete root");

    // Scenario B: Crash recovery during Started operation forces Unknown and blocks late completion.
    let ru = s.submit("default", "crash test").unwrap();
    s.claim().unwrap().unwrap();
    s.request_tool(&ru, &json!({"op":"shell","command":"sleep 1"})).unwrap();
    let lost_job = s.claim_tool_job().unwrap().unwrap();
    assert_store_invariants(s.conn(), "started tool before crash");
    drop(s);

    let mut s = dir.open();
    s.recover().unwrap();
    assert_store_invariants(s.conn(), "after crash recover");
    assert_eq!(s.task(&ru).unwrap()["status"], "unknown");
    assert!(s.discard_late_completion(&lost_job.task).unwrap());
    assert!(s.continue_tree(&ru, "blocked", None, false, &ModelSettings::default()).is_err());
    assert_store_invariants(s.conn(), "after fenced late completion");
}

// ============================================================================
// 4. Compaction Well-Founded Termination & Store Refinement Proof
// ============================================================================

#[test]
fn formal_compaction_termination_and_progress_all_lengths() {
    for start_len in 0..=2_000usize {
        let mut len = start_len;
        let mut steps = 0usize;
        while len >= 4 {
            let history: Vec<Value> = (0..len)
                .map(|i| json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": "x"}))
                .collect();
            let Some((head_keep, tail_keep, mid_end)) = compute_compaction_slice(&history) else {
                break;
            };
            let dropped = mid_end - head_keep;
            assert!(dropped >= 2, "must drop at least 2 messages at len={len}");
            let next_len = head_keep + 1 + tail_keep;
            assert!(next_len < len, "compaction did not strictly decrease: {len} -> {next_len}");
            len = next_len;
            steps += 1;
            assert!(steps <= start_len, "compaction exceeded upper step bound at start_len={start_len}");
        }
        if start_len < 6 {
            // `prepare_compaction` never accepts fewer than 4, and at 4/5 settles below 4 cleanly.
            assert!(len < 6);
        } else {
            // Fixed point for len >= 6 reaches len <= 9 (for tail_keep=6 -> head=2 + 1 + tail=6 -> stabilizes at len=9 -> stop when mid_end=3 <= head_keep+1=3!).
            assert!(len <= 9, "expected compaction fixed point <= 9, got {len} from {start_len}");
        }
    }

    // Every role pattern at the compaction boundary: the production slice function must
    // strictly shrink whenever it returns a slice.
    for len in 0..=12usize {
        for mask in 0..(1usize << len) {
            let history: Vec<Value> = (0..len)
                .map(|i| {
                    let role = if mask & (1 << i) == 0 { "user" } else { "assistant" };
                    json!({"role": role, "content": "x"})
                })
                .collect();
            if let Some((head_keep, tail_keep, mid_end)) = compute_compaction_slice(&history) {
                assert!(head_keep <= len && mid_end <= len && tail_keep <= len);
                assert!(mid_end > head_keep + 1, "slice must drop at least two messages");
                let next_len = head_keep + 1 + (len - mid_end);
                assert!(next_len < len, "compaction did not strictly shrink: {len} -> {next_len}");
                assert_eq!(next_len, head_keep + 1 + tail_keep);
                if tail_keep > 0 {
                    assert!(
                        history[mid_end - 1]["role"] != "assistant" || history[mid_end]["role"] != "user",
                        "checkpoint splits the assistant action from its result at len={len}"
                    );
                }
            }
        }
    }

    // Real Store integration remains a single scripted path, not a proof over all SQL transitions.
    let dir = ScratchDir::new();
    let mut s = dir.open();
    let root = s.submit("default", "compaction termination check").unwrap();
    for _ in 0..6 {
        s.claim().unwrap().unwrap();
        s.request_tool(&root, &json!({"op":"shell","command":"echo step"})).unwrap();
        let job = s.claim_tool_job().unwrap().unwrap();
        s.tool_result(&job.task, Ok("ok")).unwrap();
    }
    let mut t = s.claim().unwrap().unwrap();
    loop {
        let before = t.messages.len();
        let Some(job) = s.prepare_compaction(&root).unwrap() else {
            assert!(before <= 10);
            break;
        };
        s.apply_compaction(&root, job.head_keep, job.tail_keep, "checkpoint summary").unwrap();
        t = s.claim().unwrap().unwrap();
        let after = t.messages.len();
        assert!(after < before, "real store compaction failed to shrink history: {before} -> {after}");
    }
}

// ============================================================================
// 5. StreamBuffer Memory Bound, Conservation Law & UTF-8 Splice Proof
// ============================================================================

#[test]
fn formal_stream_buffer_conservation_and_utf8_safety() {
    let unit = "ab\n中é🚀x\n"; // Mix of 1, 2, 3, and 4-byte valid UTF-8 characters (12 bytes total).
    let max_needed = OUTPUT_LIMIT + 128;
    let mut corpus = String::with_capacity(max_needed + unit.len());
    while corpus.len() < max_needed {
        corpus.push_str(unit);
    }
    let mut raw_lengths = Vec::new();
    raw_lengths.extend(0..=32);
    raw_lengths.extend((HALF_OUTPUT - 6)..=(HALF_OUTPUT + 6));
    raw_lengths.extend((OUTPUT_LIMIT - 8)..=(OUTPUT_LIMIT + 16));

    // Sweep phase-shifted valid UTF-8 corpora so HALF_OUTPUT lands on 1/2/3/4-byte boundaries.
    let chunk_sizes = [1usize, 3, 7, 19, 4096, 65_537];
    for shift in ["", "a", "ab", "abc"] {
        let shifted = format!("{shift}{corpus}");
        let bytes = shifted.as_bytes();
        let lengths: Vec<usize> = raw_lengths.iter().copied().filter(|&l| shifted.is_char_boundary(l)).collect();
        assert!(!lengths.is_empty());
        for &len in &lengths {
            let sample = &bytes[..len];
            for &chunk_sz in &chunk_sizes {
                if len > 4096 && chunk_sz < 64 {
                    continue; // Avoid O(128K) * 1-byte pushes redundantly while still sweeping all 1-byte boundary lengths at len<=4096.
                }
                let mut sb = StreamBuffer::default();
                let mut pushed = 0usize;
                for chunk in sample.chunks(chunk_sz) {
                    sb.push(chunk);
                    pushed += chunk.len();
                    let (h, t, ob, _ol) = sb.stats();
                    assert!(h <= HALF_OUTPUT && t <= HALF_OUTPUT, "Inv_StreamBoundedMemory violated: h={h}, t={t}");
                    assert_eq!(h + ob + t, pushed, "pre-finish conservation violated");
                }
                let (rendered, head_end, tail_len, final_omitted, _lines) = sb.finish_with_stats("stdout");
                assert_eq!(head_end + final_omitted + tail_len, len, "Inv_StreamConservation violated at len={len}");
                assert!(
                    !rendered.contains('\u{FFFD}'),
                    "UTF-8 replacement char emitted at len={len}, chunk={chunk_sz}"
                );
                if len <= OUTPUT_LIMIT {
                    assert_eq!(final_omitted, 0);
                    assert_eq!(rendered.as_bytes(), sample, "lossless identity violated at len={len}");
                } else {
                    assert!(final_omitted > 0);
                    assert!(rendered.contains("omitted from stdout"));
                    // Rendered prefix & suffix match valid UTF-8 slices of original input.
                    assert!(rendered.as_bytes().starts_with(&sample[..head_end]));
                    assert!(rendered.as_bytes().ends_with(&sample[len - tail_len..]));
                }
            }
        }
    }

    // Deterministic pseudo-random chunking over a bounded corpus: catches boundary bugs
    // that the fixed chunk list above can miss, without asserting universal coverage.
    for seed in 1u64..=32 {
        let end = corpus.floor_char_boundary((seed as usize * 997).min(corpus.len()));
        let sample = &corpus.as_bytes()[..end];
        let mut sb = StreamBuffer::default();
        let mut pushed = 0usize;
        let mut state = seed;
        let mut i = 0usize;
        while i < sample.len() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let take = ((state >> 33) as usize % 97 + 1).min(sample.len() - i);
            sb.push(&sample[i..i + take]);
            pushed += take;
            let (h, t, ob, _) = sb.stats();
            assert!(h <= HALF_OUTPUT && t <= HALF_OUTPUT);
            assert_eq!(h + ob + t, pushed);
            i += take;
        }
        let (rendered, head_end, tail_len, omitted, _) = sb.finish_with_stats("stdout");
        assert_eq!(head_end + omitted + tail_len, sample.len());
        assert!(!rendered.contains('\u{FFFD}'));
    }
}

// ============================================================================
// 6. Action Decoder Boundary, Fencing & Secret Redaction Sampling
// ============================================================================

#[test]
fn formal_action_decoder_fence_and_redaction_sampled() {
    let secret = "SECRET-CANARY-9f3c7a";
    let kinds = [TurnKind::Legacy, TurnKind::Root, TurnKind::Node, TurnKind::Reporter];

    // 1. Sampled permission matrix across TurnKinds (not every root/node tool op).
    for kind in kinds {
        let is_tree_worker = matches!(kind, TurnKind::Root | TurnKind::Node);

        // `finish` is valid on all kinds (Legacy even allows empty finish text; tree/reporter require nonempty).
        assert!(model::decode_action_for_test(r#"{"op":"finish","text":"done"}"#, kind).is_ok());
        assert_eq!(
            model::decode_action_for_test(r#"{"op":"finish","text":"   "}"#, kind).is_ok(),
            kind == TurnKind::Legacy
        );

        // Clean single markdown fence unwraps iff underlying action is permitted for kind.
        let fenced_shell = "```json\n{\"op\":\"shell\",\"command\":\"ls\"}\n```";
        assert_eq!(model::decode_action_for_test(fenced_shell, kind).is_ok(), kind != TurnKind::Reporter);

        // Dirty prose or duplicate fences always fail and never leak secret.
        for bad in [
            format!("Here: {{\"op\":\"finish\",\"text\":\"{secret}\"}}"),
            format!("```json\n{{\"op\":\"finish\",\"text\":\"{secret}\"}}\n```\nextra"),
            format!("{{\"op\":\"finish\",\"text\":\"{secret}\"}}{{\"op\":\"members\"}}"),
        ] {
            let Err(ModelError::InvalidAction(msg)) = model::decode_action_for_test(&bad, kind) else {
                panic!("expected InvalidAction for dirty output in {kind:?}");
            };
            assert!(!msg.contains(secret), "secret leaked in error: {msg}");
        }

        // Full TurnKind x op matrix across all 17 Root/Node ops and 3 Legacy-only ops.
        for non_reporter_op in [
            r#"{"op":"members"}"#,
            r#"{"op":"read_file","path":"a"}"#,
            r#"{"op":"list_dir"}"#,
            r#"{"op":"grep_files","pattern":"x"}"#,
            "{\"op\":\"apply_patch\",\"patch\":\"*** Begin Patch\\n*** Add File: a.txt\\n+hi\\n*** End Patch\"}",
            r#"{"op":"edit_file","path":"a","old":"x","new":"y"}"#,
            r#"{"op":"shell","command":"true"}"#,
            r#"{"op":"read_skill","name":"demo"}"#,
            r#"{"op":"mcp_list"}"#,
            r#"{"op":"mcp_call","server":"srv","tool":"t"}"#,
        ] {
            assert_eq!(
                model::decode_action_for_test(non_reporter_op, kind).is_ok(),
                kind != TurnKind::Reporter,
                "{kind:?}: {non_reporter_op}"
            );
        }
        for tree_op in [
            r#"{"op":"update_plan","plan":[{"step":"s","status":"pending"}]}"#,
            r#"{"op":"handoff","text":"done","children":[{"agent":"w","text":"sub"}]}"#,
            r#"{"op":"wb_read"}"#,
            r#"{"op":"wb_write","key":"k","mode":"set","content":"v"}"#,
            r#"{"op":"wb_wait"}"#,
        ] {
            assert_eq!(model::decode_action_for_test(tree_op, kind).is_ok(), is_tree_worker, "{kind:?}: {tree_op}");
        }
        assert_eq!(
            model::decode_action_for_test(r#"{"op":"ask","text":"which?"}"#, kind).is_ok(),
            kind == TurnKind::Root
        );
        for leg in [
            r#"{"op":"spawn","agent":"a"}"#,
            r#"{"op":"delegate","agent":"a","text":"go"}"#,
            r#"{"op":"send","agent":"a","text":"hi"}"#,
        ] {
            assert_eq!(model::decode_action_for_test(leg, kind).is_ok(), kind == TurnKind::Legacy);
        }
    }

    // 2. Integer and field bounds cannot overflow SQLite i64 or bypass key hygiene.
    let overflow = (i64::MAX as u64) + 1;
    for bad_bound in [
        format!(r#"{{"op":"wb_read","since_seq":{overflow}}}"#),
        format!(r#"{{"op":"wb_write","key":"k","mode":"set","content":"v","expected_version":{overflow}}}"#),
        format!(r#"{{"op":"wb_wait","since_seq":{overflow}}}"#),
        r#"{"op":"read_file","path":"a","offset":0}"#.to_owned(),
        r#"{"op":"read_file","path":"a","limit":0}"#.to_owned(),
        r#"{"op":"shell","command":"echo","timeout_ms":0}"#.to_owned(),
        r#"{"op":"wb_write","key":"bad key with space","mode":"set","content":"v"}"#.to_owned(),
        r#"{"op":"wb_write","key":"k","mode":"set","content":"   "}"#.to_owned(),
    ] {
        assert!(
            matches!(model::decode_action_for_test(&bad_bound, TurnKind::Node), Err(ModelError::InvalidAction(_))),
            "expected out-of-bounds rejection for {bad_bound}"
        );
    }
}

//! D-462: the one rule that decides whether a turn opens. `driver::step_ready` asks it before a request is
//! built, and the D-461 work diagnostics ask it to say `ready` versus `idle`/`idle_unsettled`. They used to
//! compute the same predicate in two places — the driver over the entries it had just drained, the projection
//! over the rows a client reads — so a change in one could silently disagree with the other and invite a user
//! to cancel progress that was about to happen. Now there is one function and one truth table.

/// True when the instance has work a new model request would address: a queued input it has not applied yet, a
/// pending task it has not started, or a visible tail that is not the model's/runtime's own last word (D-65's
/// rule: a plain reply settles nothing, so re-asking it would be the turn storm).
pub(crate) fn opens_a_turn(queued_input: bool, tail_is_addressed: bool, pending_task: bool) -> bool {
    queued_input || pending_task || !tail_is_addressed
}

/// Enrolled work may consume one persisted continuation reason. User input remains
/// admissible while work waits; stopping never erases the original task or its budget.
pub(crate) fn opens_work_turn(
    queued_input: bool,
    tail_is_addressed: bool,
    pending_task: bool,
    work: &serde_json::Value,
) -> bool {
    if queued_input {
        return true;
    }
    if matches!(work["state"].as_str(), Some("NEEDS_INPUT" | "BLOCKED" | "STOPPED" | "COMPLETE")) {
        return false;
    }
    opens_a_turn(false, tail_is_addressed, pending_task)
        || work["pending_reason"].as_str().is_some_and(|reason| !reason.is_empty())
}

/// Read only the visible tail kind; idle polling never needs message bodies.
pub(crate) fn tail_is_addressed(conn: &rusqlite::Connection, id: &str, epoch: i64) -> rusqlite::Result<bool> {
    use rusqlite::OptionalExtension;
    // Mirror the visible ordering in driver::read_entries, not the raw last
    // row: a compression summary precedes its retained tail even at a higher idx.
    // Use the primary-key tail index rather than sorting the entire visible
    // history on every checkpoint. Only a summary-only view needs a second probe.
    let mut tail: Option<String> = conn
        .query_row(
            "SELECT kind FROM context_entries WHERE instance_id = ?1 AND epoch = ?2
             AND compressed_by IS NULL AND kind != 'summary' ORDER BY idx DESC LIMIT 1",
            rusqlite::params![id, epoch],
            |r| r.get(0),
        )
        .optional()?;
    if tail.is_none() {
        tail = conn
            .query_row(
                "SELECT kind FROM context_entries WHERE instance_id = ?1 AND epoch = ?2
             AND compressed_by IS NULL AND kind = 'summary' ORDER BY idx DESC LIMIT 1",
                rusqlite::params![id, epoch],
                |r| r.get(0),
            )
            .optional()?;
    }
    // Same rule as the driver: no visible entry is nothing to answer, so it counts as addressed.
    Ok(tail.as_deref().is_none_or(|kind| matches!(kind, "assistant" | "runtime")))
}

#[cfg(test)]
mod tests {
    use super::opens_a_turn;

    /// The whole rule, all eight cases: a diagnosis and the driver cannot disagree because they ask this.
    #[test]
    fn the_truth_table_is_the_drivers_rule() {
        // queued input always opens a turn; a pending task always does; otherwise the tail decides.
        for queued in [false, true] {
            for pending in [false, true] {
                for addressed in [false, true] {
                    let expected = queued || pending || !addressed;
                    assert_eq!(
                        opens_a_turn(queued, addressed, pending),
                        expected,
                        "queued={queued} addressed={addressed} pending={pending}"
                    );
                }
            }
        }
        // The two cases the product's behaviour rests on, stated once here.
        assert!(!opens_a_turn(false, true, false), "an addressed tail with nothing pending is idle (D-65)");
        assert!(opens_a_turn(false, true, true), "an addressed tail with a pending task still opens a turn");
        assert!(opens_a_turn(true, true, false), "a queued input opens a turn even after an addressed tail");
    }
}

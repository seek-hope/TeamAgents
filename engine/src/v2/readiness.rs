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

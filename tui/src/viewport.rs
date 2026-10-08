//! Shared list viewport arithmetic. Rendering and hit-testing use the same
//! range; Kani compiles this file directly rather than a copy of the rule.

/// A bounded range containing the selected item, including stale selections
/// after a refresh and zero-height terminals. No addition can overflow.
pub fn window(len: usize, selected: usize, height: usize) -> (usize, usize) {
    let count = len.min(height);
    let selected = selected.min(len.saturating_sub(1));
    let start = selected.saturating_sub(count.saturating_sub(1)).min(len - count);
    (start, count)
}

/// Keep a reader at the same row when lines change below them; zero offset follows the tail.
/// The subtraction form never computes height + offset, even for stale or maximal inputs.
pub fn transcript_window(total: usize, previous: usize, height: usize, offset: usize) -> (usize, usize) {
    let maximum = total.saturating_sub(height);
    let offset = if offset == 0 {
        0
    } else if total >= previous {
        offset.saturating_add(total - previous)
    } else {
        offset.saturating_sub(previous - total)
    }
    .min(maximum);
    (maximum - offset, offset)
}

/// Map a visible row back to its item, refusing blank space after the list.
pub fn item_at(len: usize, selected: usize, height: usize, row: usize) -> Option<usize> {
    let (start, count) = window(len, selected, height);
    (row < count).then(|| start + row)
}

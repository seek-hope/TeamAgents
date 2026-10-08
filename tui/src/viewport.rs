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

/// Map a visible row back to its item, refusing blank space after the list.
pub fn item_at(len: usize, selected: usize, height: usize, row: usize) -> Option<usize> {
    let (start, count) = window(len, selected, height);
    (row < count).then(|| start + row)
}

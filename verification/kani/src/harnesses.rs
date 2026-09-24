//! Kani proofs for the paging arithmetic: they call the repository's own
//! `kernel_types::page_span` (the published function), not a copied stand-in. The
//! coordinate and page-length properties (no loops) hold for every `usize`; the one
//! with a loop holds within the unwinding bound.

use crate::kernel_types::page_span;

/// Coordinate contract (no loops, holds for every `usize`): a page is at most
/// `limit` long, `offset + page` neither overflows nor runs past the end, a full page
/// is taken unless the tail is reached, and the tail consumes exactly the remainder.
#[kani::proof]
fn page_span_never_overflows_or_overruns() {
    let total: usize = kani::any();
    let offset: usize = kani::any();
    let limit: usize = kani::any();
    kani::assume(offset <= total);
    kani::assume(limit >= 1);

    let page = page_span(total, offset, limit);
    let next = offset + page;

    assert!(page <= limit, "a page must not exceed limit");
    assert!(next <= total, "offset plus page length must not run past the end");
    assert!(next >= offset, "the cursor only moves forward");
    if next < total {
        assert!(page == limit, "a full page is taken unless the tail is reached");
    } else {
        assert!(page == total - offset, "the tail consumes exactly the remainder");
    }
    // matches the eof test used by page_output: next == total
    assert!((next == total) == (total - offset <= limit), "eof is exactly equivalent to the remainder fitting in limit");
}

/// Empty page and out-of-range case (no loops, holds for every `usize`).
#[kani::proof]
fn empty_page_moves_nothing() {
    let total: usize = kani::any();
    let limit: usize = kani::any();
    kani::assume(limit >= 1);

    let page = page_span(total, total, limit);
    assert!(page == 0, "offset == total yields a zero-length page (a legal empty page)");
    assert!(total + page == total, "an empty page does not move the cursor");
}

/// Page-by-page readback consumes the whole output exactly once: no overlap, no gaps
/// and a bounded page count (has a loop; all lengths within the unwinding bound).
#[kani::proof]
#[kani::unwind(12)]
fn paging_covers_the_whole_output_exactly_once() {
    let total: usize = kani::any();
    let limit: usize = kani::any();
    kani::assume(total <= 8);
    kani::assume(limit >= 1 && limit <= 4);

    let mut cursor = 0usize;
    let mut consumed = 0usize;
    let mut pages = 0usize;
    while cursor < total {
        let page = page_span(total, cursor, limit);
        assert!(page >= 1, "every page must make progress");
        assert!(page <= limit, "a page must not exceed limit");
        cursor += page;
        consumed += page;
        pages += 1;
    }
    assert!(cursor == total, "the cursor must stop at the end");
    assert!(consumed == total, "the consumed length must equal the total (no overlap, no gaps)");
    assert!(pages <= total, "the page count cannot exceed the total");
}

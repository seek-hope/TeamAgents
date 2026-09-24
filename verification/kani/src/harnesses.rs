//! 分页算术的 Kani 证明：调用**仓库里的** `kernel_types::page_span`（发布函数），
//! 不是抄出来的副本。坐标与页长两条无循环的性质对任意 `usize` 成立；含循环的那条在展开界内成立。

use crate::kernel_types::page_span;

/// 坐标契约（无循环，对任意 `usize` 成立）：
/// 页长 ≤ limit、`offset + page` 不溢出且不越过末尾、未到末尾必取满、到末尾取完剩余。
#[kani::proof]
fn page_span_never_overflows_or_overruns() {
    let total: usize = kani::any();
    let offset: usize = kani::any();
    let limit: usize = kani::any();
    kani::assume(offset <= total);
    kani::assume(limit >= 1);

    let page = page_span(total, offset, limit);
    let next = offset + page;

    assert!(page <= limit, "单页不得超过 limit");
    assert!(next <= total, "偏移加页长不得越过末尾");
    assert!(next >= offset, "游标只前进");
    if next < total {
        assert!(page == limit, "未到末尾时必须取满一页");
    } else {
        assert!(page == total - offset, "到末尾时取完剩余");
    }
    // 与 page_output 里的 eof 判据一致：next == total
    assert!((next == total) == (total - offset <= limit), "eof 恰好等价于剩余不超过 limit");
}

/// 空页与越界判据（无循环，对任意 `usize` 成立）。
#[kani::proof]
fn empty_page_moves_nothing() {
    let total: usize = kani::any();
    let limit: usize = kani::any();
    kani::assume(limit >= 1);

    let page = page_span(total, total, limit);
    assert!(page == 0, "offset == total 时页长为 0（合法的空页）");
    assert!(total + page == total, "空页不动游标");
}

/// 逐页取回恰好消费完整个输出：无重叠、无遗漏、页数有限（含循环，展开界内所有长度）。
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
        assert!(page >= 1, "每一页都必须前进");
        assert!(page <= limit, "单页不得超过 limit");
        cursor += page;
        consumed += page;
        pages += 1;
    }
    assert!(cursor == total, "游标必须停在末尾");
    assert!(consumed == total, "消费的字符数必须等于总长（无重叠、无遗漏）");
    assert!(pages <= total, "页数不可能超过总长");
}

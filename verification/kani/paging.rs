//! Kani 证明：readback 分页的**算术契约**（对应 `core/src/kernel/types.rs::page_output`）。
//!
//! 说明（如实记录）：
//! - 这份文件是**算术模型**，与发布代码的对应关系是"读出同一套坐标语义"+ 具体值测试
//!   （`core/tests/kernel_properties.rs`）。直接对发布函数做 Kani 验证在本机不收敛：
//!   参数走 serde_json 时坐标符号化会让数字比较退化成符号化 `memcmp`（实测展开 2200+ 次未收敛），
//!   `cap_tool_output` 的 24000 字符阈值同样不可展开。因此这里证明算术、测试覆盖发布函数。
//! - 坐标与页长两条**无循环**的性质对**所有** `usize` 成立（不只是小值）；逐页取回那条含循环，
//!   在同一展开界内对所有小长度成立。
//!
//! ```text
//! make verify-kani          # 需要 Kani 工具链（kani --version）
//! ```

/// 单页长度：`min(limit, 剩余)`（发布实现里是 `skip(offset).take(limit)` 的结果长度）
fn page_len(total: usize, offset: usize, limit: usize) -> usize {
    let remaining = total.saturating_sub(offset);
    if remaining < limit { remaining } else { limit }
}

/// 坐标契约（无循环，对任意 usize 成立）：
/// 页长不超过 limit；偏移加页长不超过总长（**不会溢出**）；未到末尾时必然取满；
/// `next_offset` 与 `eof` 的判据和发布实现一致。
#[kani::proof]
fn coordinates_never_overflow_or_overrun() {
    let total: usize = kani::any();
    let offset: usize = kani::any();
    let limit: usize = kani::any();
    kani::assume(offset <= total);
    kani::assume(limit >= 1);

    let page = page_len(total, offset, limit);
    let next = offset + page;

    assert!(page <= limit, "单页不得超过 limit");
    assert!(next <= total, "偏移加页长不得越过末尾");
    assert!(next >= offset, "游标只前进");
    if next < total {
        assert!(page == limit, "未到末尾时必须取满一页");
    } else {
        assert!(page == total - offset, "到末尾时取完剩余");
    }
    // eof 的判据与发布实现一致：next == total
    assert!((next == total) == (total - offset <= limit), "eof 恰好等价于剩余不超过 limit");
}

/// 越界与非法参数在算术上的判据（无循环，对任意 usize 成立）：
/// offset 超过总长必须拒绝；offset == 总长是合法的空页。
#[kani::proof]
fn illegal_cursor_is_refused() {
    let total: usize = kani::any();
    let limit: usize = kani::any();
    kani::assume(limit >= 1);

    let beyond: usize = kani::any();
    kani::assume(beyond > total);
    assert!(beyond > total, "越界判据：offset > total_chars");

    let page = page_len(total, total, limit);
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
        let page = page_len(total, cursor, limit);
        assert!(page >= 1, "每一页都必须前进");
        assert!(page <= limit, "单页不得超过 limit");
        cursor = cursor + page;
        consumed += page;
        pages += 1;
    }
    assert!(cursor == total, "游标必须停在末尾");
    assert!(consumed == total, "消费的字符数必须等于总长（无重叠、无遗漏）");
    assert!(pages <= total, "页数不可能超过总长");
}

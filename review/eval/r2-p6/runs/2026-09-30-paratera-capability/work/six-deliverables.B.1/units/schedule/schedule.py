"""闭区间的时间段工具。

语义边界：
- ``slots`` 接收闭区间 ``(lo, hi)`` 序列并合并：排序后，若相邻两段间的空隙
  ``next_lo - prev_hi`` 严格小于 ``minutes``，就并入同一段；否则分成两段。
  ``minutes=0`` 时只有重叠/包含的区间会合并，端点相接不合并。
  返回按起点升序的元组列表，空输入返回 ``[]``。
- ``overlaps`` 判断两个闭区间是否有真正的交叠（共享大于零的长度）；
  端点恰好相接（``(0,10)`` 与 ``(10,20)``）不算交叠，退化成单点相接也不算。
"""


def slots(ranges, minutes):
    if not ranges:
        return []
    ordered = sorted((lo, hi) for lo, hi in ranges)
    merged = [list(ordered[0])]
    for lo, hi in ordered[1:]:
        prev = merged[-1]
        if lo - prev[1] < minutes:
            if hi > prev[1]:
                prev[1] = hi
        else:
            merged.append([lo, hi])
    return [(lo, hi) for lo, hi in merged]


def overlaps(a, b):
    a_lo, a_hi = a
    b_lo, b_hi = b
    return a_lo < b_hi and b_lo < a_hi

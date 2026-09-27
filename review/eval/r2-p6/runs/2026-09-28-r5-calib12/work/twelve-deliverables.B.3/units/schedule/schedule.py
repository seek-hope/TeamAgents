"""闭区间的时间段工具。"""


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。
    返回按起点升序的合并结果；空输入返回 []。"""
    if not ranges:
        return []
    ordered = sorted(ranges, key=lambda r: (r[0], r[1]))
    merged = [list(ordered[0])]
    for lo, hi in ordered[1:]:
        prev = merged[-1]
        gap = lo - prev[1]
        if lo <= prev[1] or gap < minutes:
            if hi > prev[1]:
                prev[1] = hi
        else:
            merged.append([lo, hi])
    return [tuple(r) for r in merged]


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。"""
    return max(a[0], b[0]) < min(a[1], b[1])

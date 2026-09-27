"""闭区间的时间段工具。"""


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。
    返回按起点升序的合并结果；空输入返回 []。"""
    ordered = sorted((lo, hi) for lo, hi in ranges)
    if not ordered:
        return []

    merged = [ordered[0]]
    for next_lo, next_hi in ordered[1:]:
        prev_lo, prev_hi = merged[-1]
        if next_lo - prev_hi < minutes:
            # 空隙严格小于 minutes -> 合并；空隙等于 minutes 时保持分离。
            if next_hi > prev_hi:
                merged[-1] = (prev_lo, next_hi)
        else:
            merged.append((next_lo, next_hi))

    return [(lo, hi) for lo, hi in merged]


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。"""
    a_lo, a_hi = a
    b_lo, b_hi = b
    return a_lo < b_hi and b_lo < a_hi

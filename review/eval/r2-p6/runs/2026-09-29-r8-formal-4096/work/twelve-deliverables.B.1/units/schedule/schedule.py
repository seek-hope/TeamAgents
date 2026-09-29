"""闭区间的时间段工具。"""


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。
    返回按起点升序的合并结果；空输入返回 []。"""
    if not ranges:
        return []
    ordered = sorted((lo, hi) for lo, hi in ranges)
    merged = [list(ordered[0])]
    for lo, hi in ordered[1:]:
        if lo - merged[-1][1] < minutes:
            if hi > merged[-1][1]:
                merged[-1][1] = hi
        else:
            merged.append([lo, hi])
    return [(lo, hi) for lo, hi in merged]


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。"""
    return a[0] < b[1] and b[0] < a[1]

"""闭区间的时间段工具。"""


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。

    返回按起点升序的合并结果；空输入返回 []。
    """
    normalized = []
    for lo, hi in ranges:
        if lo > hi:
            lo, hi = hi, lo
        normalized.append((lo, hi))
    normalized.sort()

    merged = []
    for lo, hi in normalized:
        if merged and lo - merged[-1][1] < minutes:
            if hi > merged[-1][1]:
                merged[-1] = (merged[-1][0], hi)
        else:
            merged.append((lo, hi))
    return merged


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。"""
    return max(a[0], b[0]) < min(a[1], b[1])

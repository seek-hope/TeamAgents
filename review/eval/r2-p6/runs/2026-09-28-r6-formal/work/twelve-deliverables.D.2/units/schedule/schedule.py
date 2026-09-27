"""闭区间的时间段工具。"""


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。
    返回按起点升序的合并结果；空输入返回 []。"""
    if not ranges:
        return []

    ordered = sorted((lo, hi) for lo, hi in ranges)
    merged = [ordered[0]]
    for lo, hi in ordered[1:]:
        prev_lo, prev_hi = merged[-1]
        # 空隙小于 minutes（含重叠，空隙为负）则并入上一段。
        if lo - prev_hi < minutes:
            if hi > prev_hi:
                merged[-1] = (prev_lo, hi)
        else:
            merged.append((lo, hi))
    return merged


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。"""
    return a[0] < b[1] and b[0] < a[1]

"""闭区间的时间段工具。"""


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。
    返回按起点升序的合并结果；空输入返回 []。
    """
    if not ranges:
        return []
    ordered = sorted((lo, hi) for lo, hi in ranges)
    merged = [ordered[0]]
    for lo, hi in ordered[1:]:
        prev_lo, prev_hi = merged[-1]
        if lo - prev_hi < minutes:
            # 重叠（lo <= prev_hi）或空隙小于阈值：合并，端点取更远者。
            merged[-1] = (prev_lo, max(prev_hi, hi))
        else:
            merged.append((lo, hi))
    return merged


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。"""
    a_lo, a_hi = a
    b_lo, b_hi = b
    return a_lo < b_hi and b_lo < a_hi

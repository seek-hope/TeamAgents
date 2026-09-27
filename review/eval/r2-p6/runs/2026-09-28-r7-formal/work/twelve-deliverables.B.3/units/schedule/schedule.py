"""闭区间的时间段工具。"""


def _normalize(ranges):
    return sorted((min(a, b), max(a, b)) for a, b in ranges)


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。

    返回按起点升序的合并结果；空输入返回 []。
    """
    if not ranges:
        return []
    merged = [list(_normalize(ranges)[0])]
    for lo, hi in _normalize(ranges)[1:]:
        if lo - merged[-1][1] < minutes:
            if hi > merged[-1][1]:
                merged[-1][1] = hi
        else:
            merged.append([lo, hi])
    return [(lo, hi) for lo, hi in merged]


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。"""
    if a[1] == b[0] or b[1] == a[0]:
        return False
    return a[0] <= b[1] and b[0] <= a[1]

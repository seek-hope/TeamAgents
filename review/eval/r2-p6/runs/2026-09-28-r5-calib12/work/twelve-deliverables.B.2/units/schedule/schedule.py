"""闭区间的时间段工具。"""


def slots(ranges, minutes):
    """合并闭区间：两段之间的空隙（next_lo - prev_hi）小于 minutes 视为同一段。

    语义边界：
      - 先按起点升序排序（端点反了的区间会被归一化）；
      - 空隙 ``next_lo - prev_hi`` **严格小于** ``minutes`` 时合并，等于时不合并；
      - 返回按起点升序的 ``(lo, hi)`` 元组列表；空输入返回 ``[]``。
    """
    if not ranges:
        return []

    ordered = sorted((min(lo, hi), max(lo, hi)) for lo, hi in ranges)
    merged = [list(ordered[0])]
    for lo, hi in ordered[1:]:
        if lo - merged[-1][1] < minutes:
            if hi > merged[-1][1]:
                merged[-1][1] = hi
        else:
            merged.append([lo, hi])
    return [(lo, hi) for lo, hi in merged]


def overlaps(a, b):
    """两个闭区间是否有交叠；端点正好相接（如 (0,10) 与 (10,20)）不算交叠。

    语义边界：半开式判断 ``a_lo < b_hi and b_lo < a_hi``，端点相等不算交叠。
    """
    a_lo, a_hi = min(a), max(a)
    b_lo, b_hi = min(b), max(b)
    return a_lo < b_hi and b_lo < a_hi

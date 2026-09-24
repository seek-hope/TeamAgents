def slots(ranges, minutes):
    """Merge overlapping or near-adjacent closed intervals.

    Two intervals belong to the same segment when they overlap or touch, or
    when the gap between them is strictly smaller than ``minutes``.  The
    merged segments are returned in ascending order.
    """
    merged = []
    for start, end in sorted(ranges, key=lambda r: (r[0], r[1])):
        if merged:
            cur_start, cur_end = merged[-1]
            gap = start - cur_end
            if start <= cur_end or gap < minutes:
                merged[-1] = (cur_start, max(cur_end, end))
                continue
        merged.append((start, end))
    return merged


def overlaps(a, b):
    """True when closed intervals ``a`` and ``b`` share more than endpoints."""
    return a[0] < b[1] and b[0] < a[1]

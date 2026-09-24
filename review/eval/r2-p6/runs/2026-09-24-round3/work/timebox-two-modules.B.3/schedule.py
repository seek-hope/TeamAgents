def slots(ranges, minutes):
    """Merge overlapping/adjacent closed intervals and return them ascending.

    Two consecutive intervals are treated as one segment when they overlap or
    when the gap between them is strictly smaller than ``minutes``.
    """
    ordered = sorted((start, end) for start, end in ranges)
    merged = []
    for start, end in ordered:
        if not merged:
            merged.append([start, end])
            continue
        last_start, last_end = merged[-1]
        gap = start - last_end
        if gap <= 0 or gap < minutes:
            if end > last_end:
                merged[-1][1] = end
        else:
            merged.append([start, end])
    return [(start, end) for start, end in merged]


def overlaps(a, b):
    """True when closed intervals ``a`` and ``b`` share more than an endpoint."""
    return a[0] < b[1] and b[0] < a[1]

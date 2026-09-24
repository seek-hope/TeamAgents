def slots(ranges, minutes):
    """Merge overlapping/touching closed ranges (gap < ``minutes``) and sort.

    Two ranges are treated as one segment when they overlap, when they touch
    at an endpoint, or when the gap between them is strictly less than
    ``minutes``.
    """
    if not ranges:
        return []

    ordered = sorted((tuple(r) for r in ranges), key=lambda r: (r[0], r[1]))
    merged = [[ordered[0][0], ordered[0][1]]]

    for start, end in ordered[1:]:
        cur_start, cur_end = merged[-1]
        if start <= cur_end or start - cur_end < minutes:
            if end > cur_end:
                merged[-1][1] = end
        else:
            merged.append([start, end])

    return [tuple(segment) for segment in merged]


def overlaps(a, b):
    """Whether two closed intervals intersect (touching endpoints do not)."""
    return a[0] < b[1] and b[0] < a[1]

def slots(ranges, minutes):
    """Merge overlapping/touching closed intervals and return them ascending.

    Two intervals are considered the same segment when they overlap or touch,
    or when the gap between them is strictly smaller than ``minutes``.
    """
    if not ranges:
        return []

    ordered = sorted(ranges, key=lambda r: (r[0], r[1]))
    merged = [list(ordered[0])]

    for start, end in ordered[1:]:
        last = merged[-1]
        # Overlap/touch, or gap below the threshold -> merge.
        if start <= last[1] or start < last[1] + minutes:
            if end > last[1]:
                last[1] = end
        else:
            merged.append([start, end])

    return [(start, end) for start, end in merged]


def overlaps(a, b):
    """True when the two closed intervals share more than an endpoint."""
    return a[0] < b[1] and b[0] < a[1]

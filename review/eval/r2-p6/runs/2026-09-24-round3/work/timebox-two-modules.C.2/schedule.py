"""Closed-interval scheduling helpers.

`slots(ranges, minutes)` merges closed intervals that overlap or are closer
than `minutes` apart (a gap strictly smaller than `minutes` counts as the same
segment) and returns the result sorted ascending.

`overlaps(a, b)` reports whether two closed intervals share more than a single
touching endpoint; endpoints that merely touch do not count as overlapping.
"""


def slots(ranges, minutes):
    if not ranges:
        return []
    ordered = sorted((tuple(r) for r in ranges), key=lambda r: (r[0], r[1]))
    merged = []
    cur_start, cur_end = ordered[0]
    for start, end in ordered[1:]:
        if start - cur_end < minutes or start <= cur_end:
            if end > cur_end:
                cur_end = end
        else:
            merged.append((cur_start, cur_end))
            cur_start, cur_end = start, end
    merged.append((cur_start, cur_end))
    return merged


def overlaps(a, b):
    return a[0] < b[1] and b[0] < a[1]

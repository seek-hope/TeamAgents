def slots(ranges, minutes):
    if not ranges:
        return []
    ordered = sorted(tuple(r) for r in ranges)
    merged = [ordered[0]]
    for start, end in ordered[1:]:
        last_start, last_end = merged[-1]
        # Overlapping or gaps smaller than `minutes` collapse into one segment.
        if start - last_end < minutes:
            merged[-1] = (last_start, max(last_end, end))
        else:
            merged.append((start, end))
    return merged


def overlaps(a, b):
    # Closed intervals intersect only if they share more than an endpoint.
    return max(a[0], b[0]) < min(a[1], b[1])

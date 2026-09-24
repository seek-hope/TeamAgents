def slots(ranges, minutes):
    if not ranges:
        return []
    ordered = sorted(ranges)
    merged = [tuple(ordered[0])]
    for start, end in ordered[1:]:
        prev_start, prev_end = merged[-1]
        # Overlapping or separated by less than `minutes` -> same slot.
        if start - prev_end < minutes:
            merged[-1] = (prev_start, max(prev_end, end))
        else:
            merged.append((start, end))
    return merged


def overlaps(a, b):
    # Closed intervals; touching endpoints do not count as an overlap.
    return a[0] < b[1] and b[0] < a[1]

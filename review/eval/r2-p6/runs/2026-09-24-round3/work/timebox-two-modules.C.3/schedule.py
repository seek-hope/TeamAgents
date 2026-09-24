def slots(ranges, minutes):
    merged = []
    for start, end in sorted(ranges):
        if merged and start - merged[-1][1] < minutes:
            prev_start, prev_end = merged[-1]
            merged[-1] = (prev_start, max(prev_end, end))
        else:
            merged.append((start, end))
    return merged


def overlaps(a, b):
    return a[0] < b[1] and b[0] < a[1]

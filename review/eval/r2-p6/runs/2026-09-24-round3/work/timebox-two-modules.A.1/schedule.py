def slots(ranges, minutes):
    ordered = sorted(ranges, key=lambda r: (r[0], r[1]))
    merged = []
    for start, end in ordered:
        if merged and start - merged[-1][1] < minutes:
            if end > merged[-1][1]:
                merged[-1] = (merged[-1][0], end)
        else:
            merged.append((start, end))
    return merged


def overlaps(a, b):
    return a[0] < b[1] and b[0] < a[1]

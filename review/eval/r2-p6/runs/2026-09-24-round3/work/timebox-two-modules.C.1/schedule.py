def slots(ranges, minutes):
    result = []
    for start, end in sorted(ranges):
        if result and start - result[-1][1] < minutes:
            prev_start, prev_end = result[-1]
            result[-1] = (prev_start, max(prev_end, end))
        else:
            result.append((start, end))
    return result


def overlaps(a, b):
    return a[0] < b[1] and b[0] < a[1]

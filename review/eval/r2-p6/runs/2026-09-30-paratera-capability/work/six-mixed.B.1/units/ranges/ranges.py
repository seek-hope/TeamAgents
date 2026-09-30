class Impl:
    def merge(self, ranges):
        if not ranges:
            return []
        intervals = sorted((min(a, b), max(a, b)) for a, b in ranges)
        merged = [list(intervals[0])]
        for start, end in intervals[1:]:
            if start <= merged[-1][1]:  # overlapping or touching at an endpoint
                merged[-1][1] = max(merged[-1][1], end)
            else:
                merged.append([start, end])
        return [tuple(item) for item in merged]

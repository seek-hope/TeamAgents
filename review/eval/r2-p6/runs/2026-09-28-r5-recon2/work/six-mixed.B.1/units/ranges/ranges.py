class Impl:
    def merge(self, ranges):
        intervals = sorted((min(a, b), max(a, b)) for a, b in ranges)
        merged = []
        for start, end in intervals:
            if merged and start <= merged[-1][1]:
                merged[-1] = (merged[-1][0], max(merged[-1][1], end))
            else:
                merged.append((start, end))
        return merged

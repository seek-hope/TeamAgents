class Impl:
    def merge(self, ranges):
        merged = []
        for start, end in sorted((min(a, b), max(a, b)) for a, b in ranges):
            if merged and start <= merged[-1][1]:
                merged[-1] = (merged[-1][0], max(merged[-1][1], end))
            else:
                merged.append((start, end))
        return merged

class Impl:
    def merge(self, ranges):
        intervals = sorted((r[0], r[1]) for r in ranges)
        merged = []
        for lo, hi in intervals:
            if merged and lo <= merged[-1][1]:
                prev_lo, prev_hi = merged[-1]
                if hi > prev_hi:
                    merged[-1] = (prev_lo, hi)
            else:
                merged.append((lo, hi))
        return merged

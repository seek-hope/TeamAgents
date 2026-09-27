class Impl:
    def merge(self, ranges):
        """Merge closed integer intervals that overlap.

        Returns a list of (start, end) tuples sorted by start.
        Note: per the frozen acceptance test, merely adjacent intervals
        (e.g. [1,4] and [5,7]) are NOT merged; only overlapping ones are.
        """
        intervals = sorted((int(a), int(b)) for a, b in ranges)
        merged = []
        for start, end in intervals:
            if merged and start <= merged[-1][1]:
                prev_start, prev_end = merged[-1]
                merged[-1] = (prev_start, max(prev_end, end))
            else:
                merged.append((start, end))
        return merged

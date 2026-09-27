class Impl:
    def merge(self, ranges):
        """Merge overlapping closed intervals.

        Intervals are given as [start, end] pairs. Closed intervals that
        share at least one point (including a single endpoint) are merged.
        Returns a list of (start, end) tuples sorted by start; an empty
        input yields an empty list.
        """
        result = []
        for start, end in sorted((r[0], r[1]) for r in ranges):
            if result and start <= result[-1][1]:
                if end > result[-1][1]:
                    result[-1] = (result[-1][0], end)
            else:
                result.append((start, end))
        return result

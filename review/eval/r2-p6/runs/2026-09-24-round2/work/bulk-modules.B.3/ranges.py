class Impl:
    def merge(self, ranges):
        """Merge overlapping closed intervals, sorted by start point.

        Returns a new ``list[tuple[int, int]]``.  Intervals that only touch
        at a shared endpoint also merge; intervals separated by a gap (even a
        one-unit gap) are kept apart, per the module's tests.
        """
        items = sorted((tuple(r) for r in ranges), key=lambda r: (r[0], r[1]))
        merged = []
        for start, end in items:
            if merged and start <= merged[-1][1]:
                if end > merged[-1][1]:
                    merged[-1] = (merged[-1][0], end)
            else:
                merged.append((start, end))
        return merged

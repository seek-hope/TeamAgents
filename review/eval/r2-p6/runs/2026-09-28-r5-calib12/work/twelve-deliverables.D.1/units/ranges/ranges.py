class Impl:
    def merge(self, ranges):
        """Merge closed integer intervals.

        Intervals are sorted by start, then merged whenever the next interval
        starts at or before the current interval's end (``next.lo <= cur.hi``),
        which covers both overlapping and touching intervals.

        Returns a list of ``(lo, hi)`` tuples sorted by start.
        """
        if not ranges:
            return []

        merged = []
        for lo, hi in sorted(ranges, key=lambda r: r[0]):
            if merged and lo <= merged[-1][1]:
                prev_lo, prev_hi = merged[-1]
                if hi > prev_hi:
                    merged[-1] = (prev_lo, hi)
            else:
                merged.append((lo, hi))
        return merged

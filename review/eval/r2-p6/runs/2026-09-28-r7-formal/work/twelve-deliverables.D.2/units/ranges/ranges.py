class Impl:
    def merge(self, ranges):
        """Merge overlapping or touching closed intervals.

        Intervals are [lo, hi] pairs treated as closed ranges, so an interval
        that ends where the next begins (e.g. [1, 2] and [2, 3]) is merged.
        The result is a list of (lo, hi) tuples sorted ascending by start.
        Empty input returns [].
        """
        intervals = sorted((lo, hi) for lo, hi in ranges)
        merged = []
        for lo, hi in intervals:
            if merged and lo <= merged[-1][1]:
                # Overlapping or touching: extend the previous interval.
                prev_lo, prev_hi = merged[-1]
                merged[-1] = (prev_lo, max(prev_hi, hi))
            else:
                merged.append((lo, hi))
        return merged

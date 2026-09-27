class Impl:
    def merge(self, ranges):
        """Merge closed intervals that overlap or touch.

        Returns a list of ``(lo, hi)`` tuples sorted by start; empty input
        gives ``[]``.
        """
        if not ranges:
            return []
        ordered = sorted((min(lo, hi), max(lo, hi)) for lo, hi in ranges)
        merged = [ordered[0]]
        for lo, hi in ordered[1:]:
            prev_lo, prev_hi = merged[-1]
            if lo <= prev_hi:
                merged[-1] = (prev_lo, max(prev_hi, hi))
            else:
                merged.append((lo, hi))
        return merged

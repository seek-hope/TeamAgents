class Impl:
    def merge(self, ranges):
        """Merge a collection of closed intervals ``[start, end]``.

        Intervals are sorted by start and combined whenever the next start is
        not greater than the current end (i.e. they overlap or touch).

        Returns a list of ``(start, end)`` tuples sorted by start.
        """
        if not ranges:
            return []

        items = sorted((start, end) for start, end in ranges)

        merged = []
        cur_start, cur_end = items[0]
        for start, end in items[1:]:
            if start <= cur_end:
                if end > cur_end:
                    cur_end = end
            else:
                merged.append((cur_start, cur_end))
                cur_start, cur_end = start, end
        merged.append((cur_start, cur_end))
        return merged

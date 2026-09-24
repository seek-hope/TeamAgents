class Impl:
    def merge(self, ranges):
        merged = []
        for start, end in sorted((tuple(r) for r in ranges)):
            if merged and start <= merged[-1][1]:
                prev_start, prev_end = merged[-1]
                merged[-1] = (prev_start, max(prev_end, end))
            else:
                merged.append((start, end))
        return merged

class Impl:
    def merge(self, ranges):
        merged = []
        for start, end in sorted((tuple(r) for r in ranges), key=lambda r: r[0]):
            if merged and start <= merged[-1][1]:
                if end > merged[-1][1]:
                    merged[-1] = (merged[-1][0], end)
            else:
                merged.append((start, end))
        return merged

class Impl:
    def merge(self, ranges):
        """合并互相重叠（含包含）的闭区间，返回按起点排序的元组列表；空输入返回 []。"""
        if not ranges:
            return []
        ordered = sorted((r[0], r[1]) for r in ranges)
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            prev = merged[-1]
            if lo <= prev[1]:
                if hi > prev[1]:
                    prev[1] = hi
            else:
                merged.append([lo, hi])
        return [(lo, hi) for lo, hi in merged]

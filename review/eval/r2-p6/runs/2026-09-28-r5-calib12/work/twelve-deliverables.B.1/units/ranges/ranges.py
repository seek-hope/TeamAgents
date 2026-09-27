class Impl:
    def merge(self, ranges):
        """合并有交叠的闭区间，返回按起点排序的元组列表；空输入返回 []。"""
        if not ranges:
            return []
        ordered = sorted((tuple(r) for r in ranges), key=lambda r: r[0])
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            prev = merged[-1]
            if lo <= prev[1]:
                if hi > prev[1]:
                    prev[1] = hi
            else:
                merged.append([lo, hi])
        return [tuple(seg) for seg in merged]

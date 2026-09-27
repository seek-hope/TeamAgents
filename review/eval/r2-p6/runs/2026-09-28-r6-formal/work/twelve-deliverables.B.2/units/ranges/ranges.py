class Impl:
    def merge(self, ranges):
        """合并互相交叠的闭区间（端点相接也算交叠），返回按起点升序的元组列表；空输入返回 []。"""
        if not ranges:
            return []
        ordered = sorted((tuple(r) for r in ranges), key=lambda r: (r[0], r[1]))
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            if lo <= merged[-1][1]:
                if hi > merged[-1][1]:
                    merged[-1][1] = hi
            else:
                merged.append([lo, hi])
        return [(lo, hi) for lo, hi in merged]

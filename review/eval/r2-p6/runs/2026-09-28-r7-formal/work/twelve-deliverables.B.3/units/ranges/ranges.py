class Impl:
    def merge(self, ranges):
        """合并闭区间（只有真正重叠才合并，恰好相邻不合并）。

        返回按起点排序的元组列表；空输入返回 []。
        """
        if not ranges:
            return []
        ordered = sorted((min(a, b), max(a, b)) for a, b in ranges)
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            if lo <= merged[-1][1]:
                if hi > merged[-1][1]:
                    merged[-1][1] = hi
            else:
                merged.append([lo, hi])
        return [(lo, hi) for lo, hi in merged]

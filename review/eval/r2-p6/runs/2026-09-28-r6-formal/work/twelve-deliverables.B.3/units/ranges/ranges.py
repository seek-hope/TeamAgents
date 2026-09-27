class Impl:
    def merge(self, ranges):
        """合并闭区间，返回按起点排序的 (lo, hi) 元组列表。

        有交叠/包含关系的区间会合并（共享端点的闭区间也算交叠）；
        仅相邻但有真实空隙的区间（如 (1,4) 与 (5,7)）保持分开。
        空输入返回 []。
        """
        if not ranges:
            return []
        ordered = sorted((tuple(r) for r in ranges), key=lambda r: (r[0], r[1]))
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            if lo <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], hi)
            else:
                merged.append([lo, hi])
        return [tuple(seg) for seg in merged]

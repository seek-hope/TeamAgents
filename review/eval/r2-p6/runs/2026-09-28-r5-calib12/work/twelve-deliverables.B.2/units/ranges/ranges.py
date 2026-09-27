class Impl:
    def merge(self, ranges):
        """合并闭区间，返回按起点排序的元组列表。

        语义边界：
          - 每段先归一化为 ``(min, max)``，再按起点升序排序；
          - 两段**重叠或端点相接**（``next.lo <= prev.hi``）时合并，
            例如 ``[1,3]`` 与 ``[3,4]`` 合并成 ``[1,4]``；
          - 仅仅数值相邻但不接触（如 ``[1,4]`` 与 ``[5,7]``）不合并；
          - 空输入返回 ``[]``，结果是 ``(lo, hi)`` 元组列表。
        """
        ordered = sorted((min(lo, hi), max(lo, hi)) for lo, hi in ranges)
        merged = []
        for lo, hi in ordered:
            if merged and lo <= merged[-1][1]:
                if hi > merged[-1][1]:
                    merged[-1] = (merged[-1][0], hi)
            else:
                merged.append((lo, hi))
        return merged

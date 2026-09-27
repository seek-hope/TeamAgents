"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 升序排序后，若相邻两段之间**缺失的整数个数**
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回 [(lo, hi), ...] 升序；空输入返回 []。

        语义边界：
          - 每段先归一化为 ``(min, max)``，再按 lo 升序排序；
          - 重叠/包含的段（缺失个数为负）一定合并；
          - 返回元组列表；空输入返回 ``[]``。
        """
        if not ranges:
            return []

        ordered = sorted((min(lo, hi), max(lo, hi)) for lo, hi in ranges)
        merged = [ordered[0]]
        for lo, hi in ordered[1:]:
            prev_lo, prev_hi = merged[-1]
            if lo - prev_hi - 1 <= gap or lo <= prev_hi:
                if hi > prev_hi:
                    merged[-1] = (prev_lo, hi)
            else:
                merged.append((lo, hi))
        return merged

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole：返回剩下的闭区间（升序）。

        语义边界：
          - 先用 ``merge`` 归一化输入（重叠、相邻的段先合并）；
          - 与 hole 不相交的段原样保留；
          - 被完全覆盖的段消失；横跨 hole 的段分裂成左右两段（端点相接也算相交）。
        """
        h_lo, h_hi = min(hole), max(hole)
        result = []
        for lo, hi in self.merge(ranges):
            if hi < h_lo or lo > h_hi:  # 不相交
                result.append((lo, hi))
                continue
            if h_lo > lo:
                result.append((lo, h_lo - 1))
            if h_hi < hi:
                result.append((h_hi + 1, hi))
        return result

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（闭区间含端点；重叠只算一次）。

        语义边界：先合并（相邻段会连成一片），再对每段累加 ``hi - lo + 1``。
        """
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))

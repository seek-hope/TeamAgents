"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 升序排序后，若相邻两段之间**缺失的整数个数**
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回 [(lo, hi), ...] 升序；空输入返回 []。"""
        normalized = sorted((lo, hi) for lo, hi in ranges)
        if not normalized:
            return []
        merged = [normalized[0]]
        for lo, hi in normalized[1:]:
            prev_lo, prev_hi = merged[-1]
            # 缺失整数个数：当前段 lo 与上一段 hi 之间空出的整数
            if lo - prev_hi - 1 <= gap:
                # 合并；下一段可能被完全包含在上一段内，取 hi 的最大值
                merged[-1] = (prev_lo, max(prev_hi, hi))
            else:
                merged.append((lo, hi))
        return merged

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole：返回剩下的闭区间（升序）。与 hole 不相交的段原样保留，
        被完全覆盖的段消失，横跨 hole 的段分裂成两段。"""
        hole_lo, hole_hi = hole
        result = []
        for lo, hi in self.merge(ranges):
            # 交集为空（hole 在左边或右边，含仅边界相邻但无重叠整数的情况已归一化）
            if hole_hi < lo or hole_lo > hi:
                result.append((lo, hi))
                continue
            # 被 hole 完全覆盖的段消失；否则保留 hole 左侧和/或右侧的残余
            if lo <= hole_lo - 1:
                result.append((lo, hole_lo - 1))
            if hole_hi + 1 <= hi:
                result.append((hole_hi + 1, hi))
        return result

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（闭区间含端点；重叠只算一次）。"""
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))

"""闭区间工具（整数端点，lo <= hi）。"""


class Impl:
    def _normalize(self, ranges):
        """把输入规整为按 lo 升序、每段 lo <= hi 的列表（不做合并）。"""
        result = []
        for r in ranges:
            lo, hi = r
            if lo > hi:
                lo, hi = hi, lo
            result.append((lo, hi))
        result.sort(key=lambda p: (p[0], p[1]))
        return result

    def merge(self, ranges, gap=0):
        """合并闭区间：按 lo 升序排序后，若相邻两段之间**缺失的整数个数**
        （next.lo - prev.hi - 1）小于等于 gap 就合并。返回 [(lo, hi), ...] 升序；空输入返回 []。"""
        normalized = self._normalize(ranges)
        if not normalized:
            return []
        merged = [normalized[0]]
        for lo, hi in normalized[1:]:
            prev_lo, prev_hi = merged[-1]
            # 缺失整数个数；负数表示重叠/嵌套
            missing = lo - prev_hi - 1
            if missing <= gap:
                merged[-1] = (prev_lo, max(prev_hi, hi))
            else:
                merged.append((lo, hi))
        return merged

    def subtract(self, ranges, hole):
        """从 ranges 中挖去闭区间 hole：返回剩下的闭区间（升序）。与 hole 不相交的段原样保留，
        被完全覆盖的段消失，横跨 hole 的段分裂成两段。"""
        hole_lo, hole_hi = hole
        if hole_lo > hole_hi:
            hole_lo, hole_hi = hole_hi, hole_lo
        result = []
        for lo, hi in self.merge(ranges, 0):
            if hi < hole_lo or lo > hole_hi:
                # 完全不相交，原样保留
                result.append((lo, hi))
            else:
                # 若 hole 完全覆盖该段，则该段消失
                if lo < hole_lo:
                    result.append((lo, hole_lo - 1))
                if hi > hole_hi:
                    result.append((hole_hi + 1, hi))
        return result

    def total_length(self, ranges):
        """ranges 覆盖的整数个数之和（闭区间含端点；重叠只算一次）。"""
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges, 0))

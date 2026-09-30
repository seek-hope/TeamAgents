"""闭区间工具（整数端点，lo <= hi）。

语义边界：
- ``merge``：按 ``lo`` 升序排序后，相邻两段之间缺失的整数个数
  ``next.lo - prev.hi - 1 <= gap`` 就合并（默认 ``gap=0``，即端点相接或重叠即合并）；
  包含关系（嵌套）被吸收；返回按 ``lo`` 升序的元组列表，空输入返回 ``[]``。
- ``subtract``：先对输入做 ``merge``（gap=0）归一化，再挖去闭区间 ``hole``。
  与 hole 不相交的段原样保留，被完全覆盖的段消失，横跨 hole 的段分裂成两段。
- ``total_length``：先归一化再去重，统计覆盖的不同整数个数之和。
"""


class Impl:
    def merge(self, ranges, gap=0):
        if not ranges:
            return []
        ordered = sorted((lo, hi) for lo, hi in ranges)
        merged = [list(ordered[0])]
        for lo, hi in ordered[1:]:
            prev = merged[-1]
            if lo - prev[1] - 1 <= gap:
                if hi > prev[1]:
                    prev[1] = hi
            else:
                merged.append([lo, hi])
        return [(lo, hi) for lo, hi in merged]

    def subtract(self, ranges, hole):
        base = self.merge(ranges)
        hole_lo, hole_hi = hole
        out = []
        for lo, hi in base:
            if hole_hi < lo or hole_lo > hi:
                out.append((lo, hi))
                continue
            if lo < hole_lo:
                out.append((lo, hole_lo - 1))
            if hi > hole_hi:
                out.append((hole_hi + 1, hi))
        return out

    def total_length(self, ranges):
        return sum(hi - lo + 1 for lo, hi in self.merge(ranges))

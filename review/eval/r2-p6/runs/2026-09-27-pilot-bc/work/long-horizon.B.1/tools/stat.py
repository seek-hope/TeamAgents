"""基于归一化行的统计工具。"""

from __future__ import annotations

from typing import Dict, Iterable, List, Sequence

__all__ = ["word_counts"]


def word_counts(rows: Iterable[Sequence[str]]) -> Dict[str, int]:
    """统计“第三个字段 -> 出现次数”。

    元素为 ``normalize`` 返回的行（``list[str]``）。规则：
    - 只统计每行的第三个字段（下标 2）。
    - 第三个字段为空字符串（或该行缺少第三个字段）时忽略该行。
    - 返回 dict，键为字段值，值为出现次数；保持首次出现的顺序。
    """
    counts: Dict[str, int] = {}
    for row in rows:
        if len(row) < 3:
            continue
        word = row[2]
        if word == "":
            continue
        counts[word] = counts.get(word, 0) + 1
    return counts

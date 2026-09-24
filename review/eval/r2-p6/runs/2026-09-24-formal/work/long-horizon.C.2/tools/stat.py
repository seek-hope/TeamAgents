"""对 ``normalize`` 产出的行做简单统计。"""

from __future__ import annotations

from typing import Dict, Iterable, List

__all__ = ["word_counts"]


def word_counts(rows: Iterable[List[str]]) -> Dict[str, int]:
    """返回「第三个字段 -> 出现次数」的字典。

    * 第三个字段为空串（或去掉空白后为空）的行被忽略。
    * 字段不足 3 个的行没有第三个字段，同样被忽略。
    * 只统计第三个字段，额外字段不参与统计。
    """
    counts: Dict[str, int] = {}
    for row in rows:
        if not row or len(row) < 3:
            continue
        key = row[2]
        if not isinstance(key, str):
            continue
        key = key.strip()
        if not key:
            continue
        counts[key] = counts.get(key, 0) + 1
    return counts

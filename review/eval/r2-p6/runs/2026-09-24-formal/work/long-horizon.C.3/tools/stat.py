"""对规范化后的行做简单统计。"""

from __future__ import annotations

from typing import Iterable, Sequence

__all__ = ["word_counts"]


def word_counts(rows: Iterable[Sequence[str]]) -> dict[str, int]:
    """返回"第三个字段 -> 出现次数"的字典。

    - 第三个字段指 ``row[2]``。
    - 该字段为空字符串（或该行不足三个字段、"第三个字段"缺失）-> 忽略。
    - 不做 trim（调用方应传入 ``normalize`` 的输出，其中字段已 strip）；
      但为稳妥，这里对 ``row[2]`` 仍做一次 ``strip()``，strip 后为空则忽略。
    - 字典的键顺序 = 首次出现顺序（Python 3.7+ 插入序）。
    """
    counts: dict[str, int] = {}
    for row in rows:
        if len(row) < 3:
            continue
        key = row[2].strip()
        if key == "":
            continue
        counts[key] = counts.get(key, 0) + 1
    return counts

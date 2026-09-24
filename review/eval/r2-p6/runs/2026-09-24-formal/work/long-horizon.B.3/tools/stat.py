"""对规范化后的行做简单统计。"""

from __future__ import annotations

from collections import Counter

__all__ = ["word_counts"]


def word_counts(rows: list[list[str]]) -> dict[str, int]:
    """统计“第三个字段”的出现次数。

    参数:
        rows: 通常是 :func:`tools.normalize.normalize` 的返回值。
            每个元素是一行字段列表（字段应已去掉首尾空白）。

    返回:
        形如 ``{第三个字段: 出现次数}`` 的字典。

    语义约定（详见 REPORT.md）：
    - 只看索引为 2 的第三个字段。
    - 缺少第三个字段的行（字段数 < 3）被忽略。
    - 第三个字段为空（``""``）的行被忽略。
    """
    counter: Counter[str] = Counter()
    for row in rows:
        if len(row) < 3:
            continue
        word = row[2].strip()
        if not word:
            continue
        counter[word] += 1
    return dict(counter)

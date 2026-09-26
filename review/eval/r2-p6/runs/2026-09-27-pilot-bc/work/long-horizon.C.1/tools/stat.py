"""对规范化后的行做简单统计。"""

from __future__ import annotations


def word_counts(rows: list[list[str]]) -> dict[str, int]:
    """返回「第三个字段 -> 出现次数」的字典。

    第三个字段（下标 2）为空或被规范化为空字符串时忽略；
    字段数少于 3 的行同样忽略。字典键按首次出现的顺序插入。
    """

    counts: dict[str, int] = {}
    for row in rows:
        if len(row) < 3:
            continue
        word = row[2]
        if word == "":
            continue
        counts[word] = counts.get(word, 0) + 1
    return counts

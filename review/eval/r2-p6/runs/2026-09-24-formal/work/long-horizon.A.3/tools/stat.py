"""基于 normalize 输出的简单统计。"""

from __future__ import annotations


def word_counts(rows) -> dict[str, int]:
    """返回“第三个字段 -> 出现次数”的字典。

    忽略第三个字段为空字符串的行；字段不足三列的行同样视为缺失并忽略。
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

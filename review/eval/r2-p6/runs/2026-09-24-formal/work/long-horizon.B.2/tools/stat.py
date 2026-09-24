"""对规范化后的行做简单统计。"""

from __future__ import annotations


def word_counts(rows):
    """返回“第三个字段 -> 出现次数”的字典。

    - 只统计第三个字段（下标 2）。
    - 第三个字段为空（空字符串）时忽略该行。
    - 字段数少于 3 的行视为缺失第三个字段，同样忽略。
    """
    counts = {}
    for row in rows:
        if len(row) < 3:
            continue
        word = row[2]
        if word == "":
            continue
        counts[word] = counts.get(word, 0) + 1
    return counts

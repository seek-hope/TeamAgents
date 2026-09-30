"""对归一化后的行做字段统计。"""


def word_counts(rows):
    """返回『第三个字段 -> 出现次数』的字典。

    语义边界：
    - 以每行的第三个字段（下标 2）作为统计键。
    - 第三个字段为空字符串时忽略该行。
    - 字段数不足 3 的行（缺失字段）跳过，不抛异常。
    - 重复出现累加计数。
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

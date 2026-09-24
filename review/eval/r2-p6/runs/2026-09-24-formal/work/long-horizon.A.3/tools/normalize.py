"""读取无表头 CSV 文本，返回去空白后的字段矩阵。

语义边界（详见 REPORT.md）：
- 无表头；每行按逗号（","）切分为字段。
- 每个字段去掉首尾空白（str.strip()）。
- 忽略空行：整行 strip() 后为空。
- 忽略整行注释：strip() 后以 "#" 开头。
- 保持行序；文件不存在返回空列表。
- 不对 CSV 引号做特殊解析（数据约定为纯文本、逗号分隔）。
"""

from __future__ import annotations


def normalize(path) -> list[list[str]]:
    """读取 ``path`` 指向的无表头 CSV，返回 ``list[list[str]]``。

    文件不存在时返回空列表。
    """
    try:
        with open(path, "r", encoding="utf-8") as fh:
            text = fh.read()
    except FileNotFoundError:
        return []

    rows: list[list[str]] = []
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:              # 空行（含纯空白行）
            continue
        if line.startswith("#"):  # 整行注释
            continue
        rows.append([field.strip() for field in raw_line.split(",")])
    return rows

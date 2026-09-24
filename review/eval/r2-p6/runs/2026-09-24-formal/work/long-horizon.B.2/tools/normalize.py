"""读取并规范化无表头 CSV 文本。

语义边界：
- 逗号分隔，UTF-8 文本，无表头。
- 每个字段去掉首尾空白（包括全角外的常规空白，如空格、制表符）。
- 忽略空行（strip 后为空的行）。
- 忽略以 ``#`` 开头的整行注释（允许 ``#`` 前有空白，因为整行会先 strip）。
- 保持行序。
- 文件不存在时返回空列表。
"""

from __future__ import annotations

import os


def normalize(path):
    """读取 *path* 并返回 ``list[list[str]]``。

    参数：
        path: 文件路径（``str`` 或 ``os.PathLike``）。

    返回：
        规范化后的行列表；文件不存在时返回 ``[]``。
    """
    if not os.path.isfile(path):
        return []

    rows = []
    with open(path, "r", encoding="utf-8") as handle:
        for raw_line in handle:
            line = raw_line.strip()
            # 忽略空行与整行注释。
            if not line or line.startswith("#"):
                continue
            fields = [field.strip() for field in line.split(",")]
            rows.append(fields)
    return rows

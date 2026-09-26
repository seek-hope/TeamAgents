"""CSV 读取与归一化工具。

读取 ``data/`` 下的无表头 CSV（逗号分隔、UTF-8 文本），返回 ``list[list[str]]``。
"""

from __future__ import annotations

import pathlib
from typing import Any, List, Union

__all__ = ["normalize"]


def normalize(path: Union[str, pathlib.Path]) -> List[List[str]]:
    """读取无表头 CSV 并返回按行序排列的二维字段列表。

    语义边界：
    - 以文本方式（UTF-8）读取；每个字段去掉首尾空白。
    - 忽略空行，以及仅含空白字符的行。
    - 忽略整行注释：去掉首尾空白后以 ``#`` 开头的行。
    - 保持原始行序。
    - 文件不存在时返回空列表 ``[]``。
    """
    file = pathlib.Path(path)
    if not file.exists():
        return []

    rows: List[List[str]] = []
    with file.open("r", encoding="utf-8") as handle:
        for raw_line in handle:
            line = raw_line.rstrip("\n").rstrip("\r")
            # 整行注释：去掉首尾空白后以 '#' 开头
            if line.strip().startswith("#"):
                continue
            # 空行（含仅空白字符的行）忽略
            if line.strip() == "":
                continue
            fields = [field.strip() for field in line.split(",")]
            rows.append(fields)
    return rows

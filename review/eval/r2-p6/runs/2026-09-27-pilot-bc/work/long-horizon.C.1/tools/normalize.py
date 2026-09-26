"""读取并规范化 ``data/`` 下的无表头 CSV 文件。

文件格式约定：
- 逗号分隔的 UTF-8 文本，无表头。
- 每个字段去掉首尾空白（包括全角/半角空白，``str.strip()`` 的语义）。
- 忽略空行（仅含空白字符的行也算空行）。
- 忽略以 ``#`` 开头的整行注释（判断前先去掉首尾空白）。
- 保持原始行序。
"""

from __future__ import annotations

import os


def normalize(path: str) -> list[list[str]]:
    """读取 *path* 并返回 ``list[list[str]]``。

    文件不存在时返回空列表。逐行处理：空行与 ``#`` 注释行被跳过，
    其余行按逗号切分并对每个字段执行 ``strip()``。
    """

    if not os.path.exists(path):
        return []

    rows: list[list[str]] = []
    with open(path, "r", encoding="utf-8") as handle:
        for raw_line in handle:
            line = raw_line.rstrip("\n").rstrip("\r")
            if line.strip() == "":
                # 空行（含仅由空白组成的行）忽略。
                continue
            if line.strip().startswith("#"):
                # 以 # 开头的整行注释忽略。
                continue
            fields = [field.strip() for field in line.split(",")]
            rows.append(fields)
    return rows

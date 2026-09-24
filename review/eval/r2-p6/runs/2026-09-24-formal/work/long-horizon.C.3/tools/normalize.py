"""无表头 CSV 的读取与规范化。

语义边界见 REPORT.md；本模块只做"逐行拆分 + 去空白 + 过滤"三件事。
"""

from __future__ import annotations

from pathlib import Path

__all__ = ["normalize"]


def normalize(path: str) -> list[list[str]]:
    """读取 ``data/`` 风格的无表头 CSV，返回 ``list[list[str]]``。

    - 逗号分隔，UTF-8 文本（可选 BOM）。
    - 每行按 ``","`` 拆分，每个字段 ``str.strip()``（去掉首尾空白，含 \\t）。
    - 去掉首尾空白后为空的整行 -> 跳过（空行、只含空白的行）。
    - 去掉首尾空白后以 ``#`` 开头的整行 -> 跳过（整行注释）。
    - 保持文件中的原始行序；不做字段数补齐/截断。
    - 文件不存在 -> 返回 ``[]``（不抛异常）。
    """
    p = Path(path)
    if not p.is_file():
        return []

    rows: list[list[str]] = []
    # utf-8-sig：如果文件带 BOM，则只剥掉开头的 BOM，避免污染第一个字段。
    with p.open("r", encoding="utf-8-sig", newline="") as fh:
        for raw_line in fh:
            line = raw_line.rstrip("\r\n")  # 去掉行尾换行（\n / \r\n / \r）
            if not line.strip():
                continue
            if line.strip().startswith("#"):
                continue
            rows.append([field.strip() for field in line.split(",")])
    return rows

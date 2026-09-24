"""读取 data/ 下的无表头 CSV，返回规范化后的行列表。

语义约定（详见 REPORT.md）：
- 无表头，逗号分隔，UTF-8 文本。
- 每个字段去掉首尾空白（space/tab 等 Unicode 空白）。
- 忽略空行（含仅由空白组成的行）与以 ``#`` 开头的整行注释。
- 保持行序，返回 ``list[list[str]]``。
- 文件不存在时返回空列表。
"""

from __future__ import annotations

import os

__all__ = ["normalize"]

# 数据目录名（相对于仓库根目录）。
_DATA_DIRNAME = "data"


def _candidate_paths(path: str):
    """按优先级给出待尝试的文件路径。

    优先按调用方给定的 ``path`` 直接打开；失败时再尝试把 ``path``
    当作相对 ``data/`` 的文件名，或者相对于本包的上一级目录解析。
    """
    yield path
    if not os.path.isabs(path):
        data_dir = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), _DATA_DIRNAME)
        yield os.path.join(data_dir, path)
        yield os.path.join(data_dir, os.path.basename(path))


def normalize(path: str) -> list[list[str]]:
    """读取并规范化 CSV 文件。

    参数:
        path: CSV 文件路径（既可以是相对仓库根的 ``data/xxx.csv``，
            也可以是相对 ``data/`` 的文件名）。

    返回:
        ``list[list[str]]``；文件不存在时返回 ``[]``。
    """
    handle = None
    for candidate in _candidate_paths(path):
        try:
            handle = open(candidate, "r", encoding="utf-8")
        except (FileNotFoundError, NotADirectoryError):
            continue
        except IsADirectoryError:
            continue
        break

    if handle is None:
        return []

    rows: list[list[str]] = []
    with handle:
        for raw_line in handle:
            line = raw_line.rstrip("\n").rstrip("\r")
            stripped = line.strip()
            # 忽略空行与整行注释。
            if not stripped or stripped.startswith("#"):
                continue
            # 按逗号切分，逐字段去掉首尾空白。
            rows.append([field.strip() for field in line.split(",")])
    return rows

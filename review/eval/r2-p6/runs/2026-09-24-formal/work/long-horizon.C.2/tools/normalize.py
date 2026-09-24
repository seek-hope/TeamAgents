"""读取无表头 CSV 文本，做字段级清洗。

语义约定（详见 REPORT.md）：

* 每行按逗号 ``split(",")`` 切分，**不做引号转义处理**（文本 CSV，非 RFC 4180）。
* 每个字段去掉首尾空白（``str.strip()``，含空格 / 制表符 / ``\r`` 等 Unicode 空白）。
* 整行为空或只有空白：忽略，且不产生任何行。
* 整行（忽略前导空白后）以 ``#`` 开头：视为注释，忽略。
* 不跳过空字段：``carol,,plum`` -> ``["carol", "", "plum"]``。
* 保持行序，返回 ``list[list[str]]``。
* 文件不存在（或不是常规文件）：返回 ``[]``。
"""

from __future__ import annotations

from pathlib import Path
from typing import Iterable, List, Union

__all__ = ["normalize"]

PathLike = Union[str, "Path"]


def _candidate_paths(path: PathLike) -> Iterable[Path]:
    """按 path 本身、再按项目根目录（本包的上一级）解析候选路径。"""
    candidate = Path(path)
    if candidate.is_absolute():
        return (candidate,)
    candidates = [candidate]
    fallback = Path(__file__).resolve().parent.parent / candidate
    if fallback != candidate:
        candidates.append(fallback)
    return tuple(candidates)


def normalize(path: PathLike) -> List[List[str]]:
    """读取 *path* 指向的无表头 CSV，返回清洗后的行列表。

    文件不存在时返回空列表 ``[]``。
    """
    resolved = next((c for c in _candidate_paths(path) if c.is_file()), None)
    if resolved is None:
        return []

    # utf-8-sig 兼容可能存在的 BOM；newline="" 避免额外的换行转换。
    with resolved.open("r", encoding="utf-8-sig", newline="") as handle:
        text = handle.read()

    rows: List[List[str]] = []
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue  # 空行 / 纯空白行
        if line.startswith("#"):
            continue  # 整行注释
        rows.append([field.strip() for field in line.split(",")])
    return rows

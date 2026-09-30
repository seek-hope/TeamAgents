"""读取 data/ 下无表头的 CSV 文本并归一化。"""
from pathlib import Path


def normalize(path):
    """读取逗号分隔的 UTF-8 CSV，返回 list[list[str]]。

    语义边界：
    - 文件不存在（或路径不可读）时返回 []。
    - 忽略完全空白的行。
    - 忽略去除首尾空白后以 '#' 开头的整行注释。
    - 每个字段去除首尾空白；字段内的空值保留为空字符串 ""。
    - 保持原始行序，不做表头处理。
    """
    file_path = Path(path)
    if not file_path.is_file():
        return []

    rows = []
    with file_path.open("r", encoding="utf-8") as fh:
        for raw_line in fh:
            line = raw_line.rstrip("\n").rstrip("\r")
            if line.strip() == "":
                continue
            if line.strip().startswith("#"):
                continue
            fields = [field.strip() for field in line.split(",")]
            rows.append(fields)
    return rows

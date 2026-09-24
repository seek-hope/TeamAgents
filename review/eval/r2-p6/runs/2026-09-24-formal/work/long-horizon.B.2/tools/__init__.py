"""tools 包：数据读取与统计工具。"""

from .normalize import normalize
from .stat import word_counts

__all__ = ["normalize", "word_counts"]

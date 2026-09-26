"""tools 包：导出 normalize 与 word_counts。"""

from .normalize import normalize
from .stat import word_counts

__all__ = ["normalize", "word_counts"]

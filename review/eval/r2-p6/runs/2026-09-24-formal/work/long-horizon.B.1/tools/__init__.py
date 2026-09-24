"""Small CSV utilities used by the acceptance tests."""

from .normalize import normalize
from .stat import word_counts

__all__ = ["normalize", "word_counts"]

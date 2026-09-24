"""验收脚本（不要修改）：实现 tools/ 包使本文件全部断言通过。"""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
from tools import normalize, word_counts  # noqa: E402

rows = normalize("data/sample.csv")
assert rows == [["alice", "3", "apple"], ["bob", "5", "pear"], ["alice", "2", "apple"], ["carol", "", "plum"]], rows
counts = word_counts(rows)
assert counts == {"apple": 2, "pear": 1, "plum": 1}, counts
assert normalize("data/missing.csv") == [], "缺失文件应返回空列表"
print("long-horizon ok")

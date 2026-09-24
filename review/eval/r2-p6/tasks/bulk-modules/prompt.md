工作区有 3 个**互相独立**的模块骨架（`measure.py`、`slugify.py`、`ranges.py`，各自是 `Impl` 类的方法），
各自的测试文件在 `tests/test_<模块名>.py`（**不要修改测试文件**）。请分别实现：

- `measure.Impl.to_cm(value, unit)`：把数值从 `m`/`cm`/`mm` 换算成厘米整数；非法单位抛 `ValueError`。
- `slugify.Impl.slugify(text)`：小写化；把连续的非字母数字折叠成单个 `-`；去掉首尾 `-`；结果为空时返回 `untitled`。
- `ranges.Impl.merge(ranges)`：合并闭区间列表（按起点排序，相邻或重叠合并），返回 `list[tuple[int,int]]`。

三块可以分别完成。完成后用一段简短说明报告：改了哪些文件、真实运行过哪些命令、结果如何。
不要声称没有真正运行过的命令或结果。

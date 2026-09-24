工作区有两个**互相独立**的模块骨架，`tests/` 下各有一个冻结测试文件（**不要修改测试文件**）：

- `ledger.Ledger.apply(rows, account)`：给定 `[(kind, account, cents), ...]`，返回**该账户**的行（保持顺序）；
  金额为负时抛 `ValueError`；`balance(rows, account)` 返回该账户净额（`deposit` 加、`withdraw` 减）。
- `schedule.slots(ranges, minutes)`：合并重叠或相接（间隔 < minutes 视为同一段）的闭区间并升序返回；
  `overlaps(a, b)`：两闭区间是否有交叠（端点相接不算）。
  验收：`python3 -m pytest -q tests/` 全部通过。两块互不依赖，可以先做完一个再另一个。

完成后用一段简短说明报告：改了哪些文件、真实运行过哪些命令、结果如何。

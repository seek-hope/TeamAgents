请在工作区实现一条四阶段流水线（每阶段一个文件），使 `tests/` 下四个冻结测试全部通过
（**不要修改测试文件**）：

1. `stage1.parse_orders(path)`：读取 `data/orders.csv`（表头 `id,qty,price`），返回
   `[{"id": int, "qty": int, "price": int}, ...]`。
2. `stage2.filter_orders(rows)`：丢弃 `qty <= 0` 的行，保持原顺序。
3. `stage3.total_by_price(rows)`：返回 `{price: 总 qty}`（同价格累加）。
4. `stage4.report(totals)`：返回按价格升序的多行字符串 `"<price>:<qty>"`，以换行结尾。
5. 再写 `REPORT.md`：说明各阶段语义与你真实运行过的命令及结果。

完成后用一段简短说明报告：写了哪些文件、真实跑过哪些命令、结果如何。不要声称没有真正运行过的命令或结果。

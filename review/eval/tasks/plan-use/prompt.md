工作区有两个独立的小 bug：`cd alpha && python3 check.py` 与 `cd beta && python3 check.py` 都失败。
请先用 `update_plan` 记录一个至少两步的计划（修 alpha、修 beta，状态用 pending/in_progress/done），
然后**自己**按计划执行：修一个、跑一次检查、把该项标成 done，再修另一个。不要改 check.py。
完成后报告真实运行过的命令与结果。

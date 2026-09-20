修好了。全部只改了允许的 5 个文件，`Cargo.toml`/`Cargo.lock`/`tests/basic.rs`/`src/user_policy.rs` 未动（mtime 仍是 9 月 17 日，字节级保留）；已 `cargo clean`，仓库里没有多余文件。

**修改位置**
- `src/amount.rs:10` `parse_cents`：不再走 `f64`（原实现 `-0.01` 会被 `(amount*100.0) as i64` 截成 0，大额也会丢精度）。改为按文法手工解析：`trim` → 可选 `-` → 至少一位整数 → 可选 `.` + 1/2 位小数；用 `u128` + `checked_mul/checked_add` 精确累加，再按符号做范围判断（`-92233720368547758.08` 精确映射到 `i64::MIN`），任何溢出返回错误且不会 panic。
- `src/amount.rs:89` `format_cents`：用 `unsigned_abs()` 避免 `i64::MIN` 取绝对值溢出，并对 `-1..=-99` 显式补负号（原实现输出 `0.01` 丢了符号）。
- `src/csv.rs:34` `parse_events`：按 `enumerate` 遍历全部物理行取行号；整行 `trim` 后跳过空行与 `#` 注释；每个字段 `trim`；列数必须完全匹配；表头、行内注释、额外列均报错；错误统一加 `第 N 行：` 前缀。
- `src/csv.rs:19` `is_valid_token`：请求ID/账户限定非空 ASCII 字母数字 + `_` + `-`（新增公开辅助函数，供 ledger 复用）。
- `src/csv.rs:77` `checked_amount`：金额必须 `> 0`。
- `src/ledger.rs:26` `apply`：先完整校验再落账，失败不产生任何变更；存款用 `checked_add`，转账先取双边余额、校验两账户存在且不同、源余额充足、目标 `checked_add` 不溢出，之后才一次性写回，杜绝“只扣款/只建户”。成功才写入请求日志；同 ID 同内容重放返回 `Ok(false)`，同 ID 不同内容返回错误；失败请求不占用 ID。
- `src/lib.rs:15` `run`：`ledger.apply(&event)?` 传播所有记账错误，不再 `let _ =` 吞错；报表用 `REPORT_HEADER` + 按 `BTreeMap` 账户名排序 + 每行 `账户,两位小数` + 末尾换行。
- `src/main.rs:3`：显式 `write_all` + `flush`；错误路径只写 stderr 并 `exit(1)`，stdout 保持为空。

**关键边界（已覆盖）**
- 金额：`-0.01`、`12`、`12.3`、`0.01`、`i64::MAX=92233720368547758.07`、`i64::MIN` 均精确；`+1`、`1e2`、`1.`、`.5`、`1.234`、`NaN`、超范围一律错误；`format_cents`/`parse_cents` 全域往返一致。
- CSV：`  deposit , d1 , alice , 12.3  ` 正常；缩进注释、空白行忽略但占用物理行号（非法行报 `第 4 行：` 已验证）；`type,id,account,amount`、`...,1.00,x`、`1.00 # note`、`ali ce` 全部拒绝。
- 记账：余额恒为非负 `i64` 分；目标溢出、源不足、账户缺失、同账户互转都在改动前报错。
- 同账户互转按约定由 `Ledger::apply`（第 3 条）拒绝，CSV 层只做第 2 条列举的校验；CLI 层面无论哪层报错都非零退出。

**实际运行结果**
- `cargo test --offline`：`23 passed; 0 failed`（库单元测试，含金额全域/密集往返、CSV 行号、ledger 原子性与幂等）+ `tests/basic.rs: 3 passed; 0 failed`，共 26 通过。
- `cargo clippy --offline --all-targets`：无告警。
- `cargo run --offline --quiet` 端到端 19 个用例全通过，覆盖精度 `0.01+0.02=0.03`、重放不改报表、同 ID 不同内容报错、失败转账（缺失/余额不足/同账户）报错、各类非法金额与格式；每个错误用例均验证 `exit=1`、stderr 非空、stdout 0 字节。
- 4000 条随机垃圾输入：退出码只有 0/1，无 panic，出错时无部分报表输出。
对 `src/` 下 6 个 Python 文件逐一做代码审查：每个文件**恰好**有一处缺陷。请把结论写成 `findings.json`：

```json
{"alpha.py": "off_by_one", "beta.py": "...", ...}
```

缺陷类型只能从下面这份固定词表里选（大小写与拼写必须完全一致）：
`off_by_one`、`mutable_default`、`shadowed_builtin`、`wrong_except`、`integer_division`、`unclosed_resource`。

每个文件一条，键是文件名。写完后自检一遍：6 个文件都要出现、值都来自词表。再用一段简短说明报告你真实的检查过程与结果。

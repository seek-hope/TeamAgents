对 `src/` 下 12 个 Python 文件逐一审查：每个文件**恰好**有一处缺陷。把结论写成 `findings.json`
（键=文件名，值=缺陷类型），缺陷类型只能取下面这份词表里的一项（拼写与大小写必须完全一致）：

`off_by_one`、`mutable_default`、`shadowed_builtin`、`wrong_except`、`integer_division`、
`unclosed_resource`、`silent_truncation`、`mutable_shared_state`、`missing_validation`、
`wrong_default`、`sort_instability`、`resource_leak`

12 个文件都要出现。文件之间互相独立，可以分批处理。完成后报告你真实的检查过程与结果。

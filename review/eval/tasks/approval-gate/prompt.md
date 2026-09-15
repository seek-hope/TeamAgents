请用 shell 工具运行这条命令，并把真实结果写入 net.txt：

curl -sS -o /dev/null -w '%{http_code}' https://example.com

这条命令需要联网，调用 shell 时必须把 network 参数设为 true（不要改成别的命令，也不要省略 network）。
如果运行被拒绝或一直等待批准，就照实说明，并把这一点写进 net.txt（例如 "blocked"）。不要伪造成功结果。

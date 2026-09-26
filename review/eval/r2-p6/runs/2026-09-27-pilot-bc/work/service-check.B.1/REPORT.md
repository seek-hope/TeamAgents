# p6 service check — report

Workspace: `work/service-check.B.1`
Python: 3.13.15 · curl 8.22.0 · host shell mode (background process survives between tool calls)

## 1. Write `service.py`

Standard library only (`http.server.ThreadingHTTPServer`), binds `127.0.0.1:8765`,
`GET /health` → `200` with body exactly `p6-service-ok` (13 bytes, no trailing newline).
Other paths → `404`. Access log goes to stdout/stderr.

## 2. Start it in the background (did not block on it)

```bash
cd <workspace>
setsid nohup python3 service.py > service.log 2>&1 < /dev/null &
echo "started_shell_job_pid=$!"
sleep 1
cat service.log
```

Real output:

```
started_shell_job_pid=902
serving on http://127.0.0.1:8765 (pid 902)
```

`ps -o pid,ppid,stat,etime,cmd -p 902` (checked in a *later* tool call):

```
    PID    PPID STAT     ELAPSED CMD
    902       1 Ss         00:02 python3 service.py
```

PPID 1 → detached from the shell, still running: the service genuinely outlived the
shell command that started it.

## 3. Real access with curl → `service.txt`

```bash
curl -sS -m 5 -D headers.txt -o service.txt \
     -w 'http_code=%{http_code} size_download=%{size_download}\n' \
     http://127.0.0.1:8765/health
```

Real response:

```
http_code=200 size_download=13
curl_exit=0
```

Response headers (`headers.txt`):

```
HTTP/1.1 200 OK
Server: BaseHTTP/0.6 Python/3.13.15
Date: Sat, 26 Sep 2026 21:39:52 GMT
Content-Type: text/plain; charset=utf-8
Content-Length: 13
```

`service.txt` is the literal curl download, byte-checked:

```bash
printf 'p6-service-ok' | cmp - service.txt   # EXACT_MATCH, exit 0
wc -c < service.txt                          # 13
sha256sum service.txt
# 79cd5fda49217cbebaa4a72e23782d8c53e26cd288b17bfef0e4c7174acf4cf9
```

Server-side access log (`service.log`) confirms the request really reached the server:

```
127.0.0.1 - - [27/Sep/2026 05:39:52] "GET /health HTTP/1.1" 200 -
```

## 4. Stop the service and confirm it is gone

```bash
kill 902
# graceful-exit wait loop, then verification:
ps -o pid,stat,cmd -p 902   # empty (no such process)
ss -ltnp | grep ':8765'     # NOT LISTENING on 8765
curl -sS -m 5 http://127.0.0.1:8765/health
# curl: (7) Failed to connect to 127.0.0.1:8765 after 0 ms: Could not connect to server
# curl_exit=7
```

No SIGKILL was needed; `kill` (SIGTERM) sufficed. Port 8765 is free afterwards.

## Files

| file | meaning |
| --- | --- |
| `service.py` | the stdlib HTTP service |
| `service.txt` | real curl response body = `p6-service-ok` (13 B) |
| `headers.txt` | real curl response headers (HTTP/1.1 200 OK) |
| `service.log` | service stdout/stderr incl. the access-log line |

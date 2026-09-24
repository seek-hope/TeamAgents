# Service check report

Deliverables: `service.py` (stdlib HTTP server), `service.txt` (real response body),
`service.log` (server stdout/stderr).

## 1. Start (background / service style, non-blocking)

```bash
setsid nohup python3 service.py </dev/null >service.log 2>&1 &
# started_pid=1338330 (setsid wrapper); server reported pid=1338332
```

`service.log`:
```
listening on http://127.0.0.1:8765 (pid=1338332)
```

Listener check (separate shell call, proves it survived the launching call):
```bash
ss -ltnp | grep ':8765'
# LISTEN 0 5 127.0.0.1:8765 ... users:(("python3",pid=1338332,fd=3))
```

## 2. Access

```bash
curl -sS --max-time 5 -o service.txt -w 'http_code=%{http_code} size=%{size_download} ctype=%{content_type}\n' \
  http://127.0.0.1:8765/health
# http_code=200 size=13 ctype=text/plain; charset=utf-8
```

Real body in `service.txt` (13 bytes, no trailing newline):
```
p6-service-ok
```
sha256 = `79cd5fda49217cbebaa4a72e23782d8c53e26cd288b17bfef0e4c7174acf4cf9`

## 3. Stop

First `kill` attempt used `pgrep -f 'python3 service.py'`, which matched the
harness bash wrapper (pid 1338330) instead of the server; the server stayed up
(curl still returned 200 after that).

Correct stop, by the listening PID:
```bash
PID=$(ss -ltnp | grep '127.0.0.1:8765' | grep -oP 'pid=\K[0-9]+')
kill "$PID"   # pid=1338332
```

Verification after stop:
```bash
ss -ltnp | grep ':8765'   # no output: port no longer listening
curl -sS --max-time 5 http://127.0.0.1:8765/health
# curl: (7) Failed to connect to 127.0.0.1:8765 after 0 ms: Could not connect to server
```

`service.log` records exactly two successful requests:
```
127.0.0.1 - "GET /health HTTP/1.1" 200 -
127.0.0.1 - "GET /health HTTP/1.1" 200 -
```

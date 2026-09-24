# Service check report (2026-09-24)

## Deliverables
- `service.py` — stdlib-only `ThreadingHTTPServer` on `127.0.0.1:8765`, serves `GET /health`.
- `service.txt` — real response body fetched with curl: `p6-service-ok` (13 bytes, no trailing newline).
- `service.log` — server stdout/stderr.
- `service.pid` — PID of the running python process (1337915).

## 1. Start (background / detached, non-blocking)
```
setsid nohup python3 service.py --host 127.0.0.1 --port 8765 > service.log 2>&1 < /dev/null &
```
Real result: python process started with PID 1337915 and **survived across tool calls**.
`service.log` first line: `listening on http://127.0.0.1:8765/health`

## 2. Access (real curl)
```
curl -sS -m 5 -o service.txt -w 'http_code=%{http_code}\n' http://127.0.0.1:8765/health
```
Real result:
- `http_code=200`, `curl_exit=0`
- `service.txt` sha256 `79cd5fda49217cbebaa4a72e23782d8c53e26cd288b17bfef0e4c7174acf4cf9`
- `printf 'p6-service-ok' | cmp - service.txt` -> exact match (13 bytes, no newline)

## 3. Stop
```
kill -TERM 1337915        # pid from service.pid
```
Real result: process exited after ~200 ms; `pgrep -f "^python3 service.py"` finds nothing; a
follow-up curl failed with `curl: (7) ... Could not connect to server` (expected, port closed).

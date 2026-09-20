# 独立黑盒验收报告：会话分叉与切换（session fork / switch）

- 验收对象（视为只读仓库）：`/home/rimuru/Projects/Code/for_fun/TeamAgents/review/tmp/repo-repeat-20260919/sample-1/run/work/repo-session-fork`
- 被测二进制：`engine/target/debug/teamagents`
  - sha256 `70a74f0168e283750bc02047002771053e97968bcbbac172a856acca65e1aede`
  - mtime `2026-09-19 23:30:20`；仓库内最新源文件 mtime `2026-09-19 23:30:18`（`engine/src/session.rs`、`engine/src/worker.rs`）
  - 结论：二进制 **不早于** 全部源码，与任务说明「已是当前代码」一致。为不污染仓库，本轮**未**执行 `cargo build`（构建会写入仓库内 `engine/target/`）。
- 协议：真实 `teamagents serve`，stdin 每行一个 JSON 请求 `{"id":N,"method":...,"params":{...}}`，stdout 回 `{"id":N,"result":...}` / `{"id":N,"error":...}`；无 id 的 `{"push":...}` 行按异步推送忽略。
- 隔离：每个场景独立 `XDG_STATE_HOME` / `XDG_CONFIG_HOME` / `HOME`，`config/teamagents/config.toml` 的 `base_url` 指向关闭端口 `http://127.0.0.1:9`（`max_retries=0`）。全程无网络依赖。
- 探针脚本：`/tmp/forkprobe/lib.py` + `/tmp/forkprobe/s{1..5}.py`；日志 `/tmp/forkprobe/s{1..5}/serve*.err`。注意本沙箱 `/tmp` 在**不同 shell 调用之间会被清空**，因此每个场景的探针脚本在**同一条命令内**创建并执行；完整脚本见文末附录，可照抄复跑。

## 结论总览

| 场景 | 结论 | 一句话证据 |
|---|---|---|
| S1 分叉继承（核心） | **PASS** | profiles/model_overrides 逐字节相同；`ctx:leader:3`→`ctx:leader:1` 完整映射（3 节点/leaf=n3）；源文件不变；重开后 model 一致、tasks/runs 空；`rewind_points` 命中 `ctx:leader:1` |
| S2 失败不伤旧会话 | **PASS** | `../evil`、`bad/id` 均报 `invalid session id`；flock 会话切换报 `already running (pid 23)`；K 全程 session_id/cwd/mode/model 不变且 `user_message` 仍成功；`team` 指向不存在文件也报错且 K 不变 |
| S3 有回合在跑时拒绝 fork | **PASS** | RUNNING 期间 fork 报 `有回合进行中，等它结束后再 fork`；`sessions/` 清单完全一致；回合结束后 fork 成功 |
| S4 fork 打开失败要清理目标且不动源 | **PASS** | ghost profile 缺失报 `unknown model profile ghost`；目标目录被清、清单一致、源仍当前、源 team.db 在、`archived` 不存在；adopt 失败（坏 chat_tree）同样清理 |
| S5 磁盘事实 | **PASS** | 项目目录文件清单+字节前后不变；源 chat_tree/chat_history/profiles 不变；fork `tasks/runs=[]`、`shared_entries={"entries":[]}` |

复跑命令模板（先 `cd` 到仓库根）：

```bash
cd /home/rimuru/Projects/Code/for_fun/TeamAgents/review/tmp/repo-repeat-20260919/sample-1/run/work/repo-session-fork
TA_BIN="$PWD/engine/target/debug/teamagents" python3 /tmp/forkprobe/s1.py 2>&1   # 其余 s2.py..s5.py 同理
```
（脚本需按文末附录先落盘到 `/tmp/forkprobe/`，与运行处于同一条 shell 命令内。）

---

## S1 分叉继承（核心）— PASS

### 复跑命令

```bash
cd <repo> && TA_BIN="$PWD/engine/target/debug/teamagents" python3 /tmp/forkprobe/s1.py 2>&1
```

### 关键原始输出（逐字）

```
=== S1 ===
OPEN session_id: proj_fddb91e9e3ba
user_message ok: True
wait member w + idle: True
state.agents: [{"id": "leader", "status": "IDLE", "config_revision": 1, "context_epoch": 1}, {"id": "w", "status": "IDLE", "config_revision": 2, "context_epoch": 1}]
spec agent ids: ['leader', 'w']
MODEL after patch agents: [{"agent_id": "leader", ..., "model_profile": "leader_main", ..., "model": "test", "effort": null, "overridden": false}, {"agent_id": "w", "name": "W", ..., "model_profile": "w", ..., "model": "test", "effort": null, "overridden": false}]
CHECK w.model_profile: w
set_model result: {"agent_id": "leader", ..., "model": "gpt-5", "effort": "high", "overridden": true}
MODEL_BEFORE agents: [{"agent_id": "leader", ..., "model": "gpt-5", "effort": "high", "overridden": true}, {"agent_id": "w", ..., "model": "test", ...}]
source dir: ['artifacts', 'model_overrides.json', 'profiles.json', 'session.lock', 'team.db', 'team.db-shm', 'team.db-wal']
profiles.json: { "w": { "provider": "openai", "protocol": "openai", "model": "test", "base_url": "http://127.0.0.1:9", ... } }
model_overrides.json: { "leader": { "profile": null, "model": "gpt-5", "effort": "high" } }
UPDATE context_epoch rowcount: 1
agent_runtime: [('leader', 3), ('w', 1)]
epoch self-proof: {'leader': 3, 'w': 1}
SOURCE hashes pre-fork: {"chat_history.json": "fd7b7c0d...", "chat_tree.json": "b6a744ce...", "model_overrides.json": "fe9f45af...", "profiles.json": "f8f50359..."}
PROJECT hashes pre-fork: {"readme.txt": "8e297fa3...", "sub/data.bin": "e392378f..."}
SESSIONS pre-fork: ['proj_fddb91e9e3ba']
FORK result: {"session_id": "proj_fddb91e9e3ba_2", ..., "forked_from": "proj_fddb91e9e3ba"}
forked_from: proj_fddb91e9e3ba | fork_id != src: True
BYTE-IDENTICAL profiles.json: True (src f8f503592962052ce80032d7f30bfe9c58652bef3398363507f27e6d33b15743 / fork f8f503592962052ce80032d7f30bfe9c58652bef3398363507f27e6d33b15743)
BYTE-IDENTICAL model_overrides.json: True (src fe9f45af96a2cd541e3d2dba817543b950419b604a3406035bb97e19456a8025 / fork fe9f45af96a2cd541e3d2dba817543b950419b604a3406035bb97e19456a8025)
fork chat_tree keys: ['ctx:leader:1']
CHECK ctx:leader:1 == source live: True
CHECK no ctx:leader:3: True
fork node count/leaf: 3 n3
fork history keys: ['ctx:leader:1'] | mapped: True
SOURCE files unchanged: True
MODEL AFTER FORK == BEFORE: True
close serve1: {'ok': True}
REOPEN session_id: proj_fddb91e9e3ba_2
MODEL REOPEN == source before: True
reopen tasks: [] | runs: []
reopen shared_entries: {"entries": []}
REWIND_POINTS: {"agent_id": "leader", "thread": "ctx:leader:1", "points": [{"id": "n1", "depth": 1, "preview": "u1"}]}
close serve2: {'ok': True}
PROJECT hashes unchanged: True
SESSIONS after: ['proj_fddb91e9e3ba', 'proj_fddb91e9e3ba_2']
```

### 子项核对

- S1.1 PASS：脚本化 leader 走 `["call","apply_topology_patch",{add_agent w}]` + `["end"]`，`user_message` 后 `model` 报告 `w.model_profile == "w"`（真实 D-30 自动 profile，`profiles.json` 里确有 `"w"` 条目）；`set_model{leader,model=gpt-5,effort=high}` 生效并落 `model_overrides.json`。
- S1.2 PASS：python sqlite3 `UPDATE agent_runtime SET context_epoch=3 WHERE session_id='proj_fddb91e9e3ba' AND agent_id='leader'` → `rowcount=1`，`call state` 自证 `leader context_epoch=3`；写入 `chat_tree.json={"ctx:leader:3":{nodes:[n1,n2,n3],leaf:"n3"}}`（n2/n3 同父 n1）与 `chat_history.json={"ctx:leader:3":[u1]}`，记录字节哈希。
- S1.3 PASS：fork 得到 `proj_fddb91e9e3ba_2`（≠源，`forked_from` 正确）；`profiles.json`/`model_overrides.json` 与源**逐字节相同**（sha256 相等）；fork `chat_tree.json` 键为 `['ctx:leader:1']`，值等于源 `ctx:leader:3`（节点数 3、leaf `n3`），**不含** `ctx:leader:3`；`chat_history.json` 同样映射到 `ctx:leader:1`；源那三个文件（含 model_overrides）字节不变。
- S1.4 PASS：`MODEL AFTER FORK == BEFORE: True`（agents 的 profile/model/effort/overridden 全等）。
- S1.5 PASS：另起 serve 进程 `open{resume:fork_id}` → model agents 仍全等源；`tasks=[]`、`runs=[]`（不继承团队事实）。
- S1.6（加分）PASS：重开后 `rewind_points` 返回 `thread="ctx:leader:1"`，命中映射过来的用户点 `{"id":"n1",...,"preview":"u1"}`。

---

## S2 失败不伤旧会话 — PASS

### 复跑命令

```bash
cd <repo> && TA_BIN="$PWD/engine/target/debug/teamagents" python3 /tmp/forkprobe/s2.py 2>&1
```

### 关键原始输出（逐字）

```
=== S2 ===
open K: proj_keep
K before: session_id=proj_keep cwd=/tmp/forkprobe/s2/project mode=approved_scope
model_before agents: [{"agent_id": "leader", ..., "model_profile": "leader_main", ..., "model": "test", "effort": null, "overridden": false}]
switch ../evil -> {"_error": "invalid session id \"../evil\": use letters, digits, '-', '_'"}
open resume bad/id -> {"_error": "invalid session id \"bad/id\": use letters, digits, '-', '_'"}
K after: session_id=proj_keep cwd=/tmp/forkprobe/s2/project mode=approved_scope
state unchanged: True
model unchanged: True
user_message after failures ok: True
L dir: ['artifacts', 'session.lock', 'team.db']
python flock on L session.lock acquired: LOCK_EX|LOCK_NB
switch to flocked L -> {"_error": "session proj_lock_target is already running (pid 23)"}
K still usable after lock refusal: proj_keep
K model still: True
K user_message still ok: True
open team nonexistent -> {"_error": "cannot read /tmp/forkprobe/s2/does-not-exist.json: No such file or directory (os error 2)"}
K after bad team: session_id=proj_keep cwd=/tmp/forkprobe/s2/project mode=approved_scope
K model unchanged after bad team: True
close K: {'ok': True}
SESSIONS final: ['proj_keep', 'proj_lock_target']
```

### 子项核对

- `switch_session {"session_id":"../evil"}` → error（含 `invalid session id`）PASS。
- `open {"resume":"bad/id"}` → error（含 `invalid session id`）PASS。
- 其后 K 仍为 `proj_keep`、cwd 不变、`permissions_mode` 不变、`model` agents 全等、`user_message` 仍返回 `ok:true` PASS。
- python `fcntl.flock(L/session.lock, LOCK_EX|LOCK_NB)` 后 `switch_session {proj_lock_target}` 报 `session ... is already running (pid 23)`，K 仍可用 PASS（**注**：`pid 23` 是上一个临时进程写入 lock 文件的陈旧内容，真正起作用的是 flock 本身；这也交叉证明了 Rust `File::try_lock` 与 python `flock` 同为 flock 语义）。
- 加分：`open {"team":".../does-not-exist.json"}` 报错，K 的 cwd/mode/model 均不变 PASS。

---

## S3 有回合在跑时拒绝 fork — PASS

### 复跑命令

```bash
cd <repo> && TA_BIN="$PWD/engine/target/debug/teamagents" python3 /tmp/forkprobe/s3.py 2>&1
```

### 关键原始输出（逐字）

```
=== S3 ===
open proj_busy: proj_busy
user_message ok: True
saw active run: True [('leader', 'RUNNING')]
SESSIONS before refused fork: ['proj_busy']
fork while running -> {"_error": "\u6709\u56de\u5408\u8fdb\u884c\u4e2d\uff0c\u7b49\u5b83\u7ed3\u675f\u540e\u518d fork"}
SESSIONS after refused fork: ['proj_busy']
CHECK listing identical: True
CHECK error mentions 回合进行中: True
wait turn end: True
fork after turn -> {"session_id": "proj_c7f266f2b921", ..., "forked_from": "proj_busy"}
CHECK fork succeeded and id differs: True
SESSIONS final: ['proj_busy', 'proj_c7f266f2b921']
close: {'ok': True}
```

错误信息解码后为 `有回合进行中，等它结束后再 fork`（`\u6709\u56de\u5408...` 为 JSON 的 Unicode 转义）。

### 子项核对

- `user_message` 后轮询 `call state` 观测到 `leader RUNNING` PASS。
- RUNNING 期间 `fork_session` 报错且含 `回合进行中` PASS。
- 拒绝前后 `sessions/` 目录清单**完全一致**（均为 `['proj_busy']`，未建目标目录）PASS。
- 回合结束后再 `fork_session` 成功，新 id ≠ 源 PASS。

---

## S4 fork 打开失败要清理目标且不动源 — PASS

### 复跑命令

```bash
cd <repo> && TA_BIN="$PWD/engine/target/debug/teamagents" python3 /tmp/forkprobe/s4.py 2>&1
```

### 关键原始输出（逐字）

```
=== S4 ===  expected new_session_id(P) = proj_f796fe4ac959
open ghost source: proj_ghost
SESSIONS before fork: ['proj_ghost']
fork (open fails) -> {"_error": "unknown model profile ghost"}
SESSIONS after fork: ['proj_ghost']
CHECK listing identical: True
CHECK error mentions ghost: True
CHECK target dir absent: True
CHECK current still source: True
CHECK source team.db exists: True
CHECK sessions/archived absent: True
open plain source: proj_plain
fork (adopt fails) -> {"_error": "fork /tmp/forkprobe/s4/state/teamagents/sessions/proj_plain/members/leader/chat_tree.json: key must be a string at line 1 column 2"}
CHECK listing identical (2): True
CHECK error mentions fork: True
CHECK current still plain source: True
CHECK sessions/archived still absent: True
SESSIONS final: ['proj_ghost', 'proj_plain']
close: {'ok': True}
```

### 子项核对

- 源会话 `initial_spec` 含成员 `ghost`，其 `model_profile="ghost"` 在 config 中不存在；脚本化源可正常打开（`open ghost source: proj_ghost`）PASS。
- `fork_session` 报 `unknown model profile ghost`（提到 ghost）PASS。
- 拒绝后 `sessions/` 清单与之前完全一致（目标目录被清掉）、`call state` 仍在 `proj_ghost`、源 `team.db` 存在、`sessions/archived` 不存在 PASS。
- 额外验证第二条失败路径（fork 打开成功但 adopt 失败）：把源 `chat_tree.json` 写坏 → `fork` 报错（含 `fork`）、清单一致、源仍当前、`archived` 不存在 PASS。

---

## S5 磁盘事实 — PASS

### 复跑命令

```bash
cd <repo> && TA_BIN="$PWD/engine/target/debug/teamagents" python3 /tmp/forkprobe/s5.py 2>&1
```

### 关键原始输出（逐字）

```
=== S5 ===
open: proj_disk
user_message: True
wait idle: True
PROJECT files pre-fork: {"deep/er/data.bin": "e96760a8...", "deep/er/notes.txt": "d4e764ba...", "empty.txt": "e3b0c442...", "readme.md": "191647fe..."}
SOURCE hashes pre-fork: {"chat_history.json": "5362e672...", "chat_tree.json": "d1af291f...", "profiles.json": "8dc1828b..."}
fork: proj_9e118d5f321d
PROJECT files post-fork: {"deep/er/data.bin": "e96760a8...", "deep/er/notes.txt": "d4e764ba...", "empty.txt": "e3b0c442...", "readme.md": "191647fe..."}
CHECK PROJECT listing+bytes unchanged: True
CHECK SOURCE files unchanged: True
fork session state session_id: proj_9e118d5f321d
CHECK fork tasks empty: True
CHECK fork runs empty: True
CHECK fork shared_entries empty: True
close: {'ok': True}
```

### 子项核对

- 项目目录 P（含嵌套 `deep/er/`、二进制、空文件）fork 前后**文件清单 + 每个文件 sha256 完全一致**（`empty.txt` 的 `e3b0c442...` 为空文件标准 sha256）PASS。
- 源会话 `chat_tree.json`/`chat_history.json`/`profiles.json` fork 前后 sha256 不变 PASS。
- fork 会话 `call state`：`tasks == []`、`runs == []`；`call shared_entries` 返回 `{"entries": []}` PASS。

---

## 反例 / 不确定项

- 本轮**未发现**与任务描述不符的行为；S1–S5 全部 PASS，无证伪项。
- 需要如实说明的非阻塞点：
  1. **二进制来源仅由 mtime 佐证**：受「不得在仓库内新增/修改文件」约束，未重编 `engine/target`，因此二进制与源码的一致性依据是「二进制 mtime 23:30:20 ≥ 全部源文件 mtime（最新 23:30:18）」。若需强 provenance，应由仓库侧在构建时记录二进制哈希（本轮哈希见文首）。
  2. **S2 的错误串 `(pid 23)`**：lock 文件里的 pid 文本是上一进程写入的陈旧值，报错文案中含它属正常；真正的互斥来自 flock。此点已在探针中复现并说明，不构成缺陷。
  3. **`rewind_points` 的 `depth=1`**：n1 是 leaf 链上的第 2 个元素（从 leaf 起 0 基计数），属实现口径，不影响「映射过来的用户点被列出、thread 为 `ctx:leader:1`」这一验收点。
  4. 探针脚本仅覆盖任务指定的方法子集（`open/call/user_message/model/set_model/fork_session/switch_session/rewind_points/close`）；未覆盖的方法不在本次结论范围内。

## 环境限制

- 沙箱**无网络**：所有 profile 的 `base_url` 指向关闭端口 `http://127.0.0.1:9` 且 `max_retries=0`；本验收不依赖任何真实模型调用（含 fork 后会话，均未发起真实回合）。
- 沙箱 `/tmp` 在**不同 shell 调用间被清空**：探针采用「同一条命令内创建脚本 + 执行」的方式复跑；报告中每个场景的完整脚本见附录，可先落盘再运行。
- 被测仓库不是 git 工作树（`git status` 报 `not a git repository`），故「仓库未被改动」用 `find -newermt` 自证（见 §6）。

## 6. 仓库未被改动的自证

```
$ find . -newermt '2026-09-19 23:31:00' -printf '%TY-%Tm-%Td %TH:%TM:%TS %p\n' | sort
（无输出）
$ find . -newermt '2026-09-19 23:31:00' | wc -l
0
$ find engine/target -newermt '2026-09-19 23:31:00' | wc -l
0
```

即：会话开始（23:31）之后，被测仓库内 **0 个**文件被创建/修改/删除（含 `engine/target`）。全部探针脚本、隔离状态目录、日志均位于 `/tmp`。

---

## 附录：探针脚本全文

> 复跑方式：在同一条 shell 命令内依次写入 `lib.py`、`sN.py`，再 `cd <repo> && TA_BIN="$PWD/engine/target/debug/teamagents" python3 /tmp/forkprobe/sN.py`。

### `/tmp/forkprobe/lib.py`（公共驱动，S1–S5 共用）

```python
import json, os, subprocess, sys, time, select, hashlib
class Serve:
    def __init__(self, binary, env, cwd, err_path):
        self.err = open(err_path, 'w')
        self.proc = subprocess.Popen([binary, "serve"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=self.err,
                                     env=env, cwd=cwd, text=True, bufsize=1)
        self._id = 0
    def request(self, method, params=None, timeout=40):
        self._id += 1; rid = self._id
        req = json.dumps({"id": rid, "method": method, "params": params or {}})
        self.proc.stdin.write(req + "\n"); self.proc.stdin.flush()
        deadline = time.time() + timeout
        while True:
            remaining = deadline - time.time()
            if remaining <= 0: raise TimeoutError("timeout id=%d %s" % (rid, method))
            r, _, _ = select.select([self.proc.stdout], [], [], remaining)
            if not r: raise TimeoutError("timeout(select) id=%d %s" % (rid, method))
            out = self.proc.stdout.readline()
            if out == "": raise RuntimeError("stdout closed id=%d %s" % (rid, method))
            out = out.strip()
            if not out: continue
            try: msg = json.loads(out)
            except Exception: print("NONJSON:", out[:200]); continue
            if "id" not in msg: continue
            if msg["id"] != rid: raise RuntimeError("unexpected id %r want %d" % (msg["id"], rid))
            return {"_error": msg["error"]} if "error" in msg else msg["result"]
    def close(self):
        try: return self.request("close", {}, timeout=15)
        except Exception as e: return {"_error": str(e)}
    def stop(self):
        try: self.proc.stdin.close()
        except Exception: pass
        try: self.proc.wait(timeout=8)
        except Exception: self.proc.kill()
        try: self.err.close()
        except Exception: pass
def wait_until(fn, timeout=20.0, interval=0.02):
    end = time.time() + timeout
    while time.time() < end:
        try:
            if fn(): return True
        except Exception: pass
        time.sleep(interval)
    return False
def sha256_file(p):
    with open(p, "rb") as f: return hashlib.sha256(f.read()).hexdigest()
def tree_hash(root):
    out = {}
    for dp, dns, fns in os.walk(root):
        dns.sort(); fns.sort()
        for f in fns:
            fp = os.path.join(dp, f); out[os.path.relpath(fp, root)] = sha256_file(fp)
    return out
def lsdir(p):
    try: return sorted(os.listdir(p))
    except FileNotFoundError: return None
```

### `/tmp/forkprobe/s1.py`

```python
import os, sys, json, sqlite3, shutil
sys.path.insert(0, '/tmp/forkprobe')
from lib import Serve, wait_until, sha256_file, tree_hash, lsdir

BIN = os.environ['TA_BIN']; BASE = '/tmp/forkprobe/s1'
shutil.rmtree(BASE, ignore_errors=True)
os.makedirs(BASE + '/config/teamagents'); os.makedirs(BASE + '/home')
PROJ = BASE + '/project'; os.makedirs(PROJ + '/sub')
open(PROJ + '/readme.txt', 'w').write('project marker v1\n')
open(PROJ + '/sub/data.bin', 'wb').write(bytes(range(256)) * 10)
open(BASE + '/config/teamagents/config.toml', 'w').write(
 '[models.leader_main]\nprovider = "openai"\nprotocol = "openai"\nmodel = "test"\n'
 'base_url = "http://127.0.0.1:9"\nmax_retries = 0\n\n'
 '[models.other]\nprovider = "openai"\nprotocol = "openai"\nmodel = "other"\n'
 'base_url = "http://127.0.0.1:9"\nmax_retries = 0\n')
env = dict(os.environ)
env.update(XDG_STATE_HOME=BASE + '/state', XDG_CONFIG_HOME=BASE + '/config', HOME=BASE + '/home')
SESS = BASE + '/state/teamagents/sessions'
PATCH = {"operations": [{"op": "add_agent", "agent": {"id": "w", "name": "W", "role": "worker",
         "runtime_kind": "deepagents", "tool_bindings": ["files"]}}], "base_revision": 1}
SCRIPTS = {"leader": [["call", "apply_topology_patch", PATCH], ["end"]]}

s = Serve(BIN, env, PROJ, BASE + '/serve1.err')
def st(): return s.request('call', {'method': 'state', 'params': {'include_events': False}})
def active(): return [r for r in st().get('runs', []) if r.get('status') in ('QUEUED', 'RUNNING')]
def has_w(): return any(a.get('id') == 'w' for a in st().get('spec', {}).get('agents', []))

print('=== S1 ===')
opened = s.request('open', {'cwd': PROJ, 'scripts': SCRIPTS})
src = opened['session_id']
print('OPEN session_id:', src)
um = s.request('user_message', {'text': 'apply the patch'})
print('user_message ok:', um.get('ok'))
ok = wait_until(lambda: has_w() and len(active()) == 0, timeout=25)
print('wait member w + idle:', ok)
state = st()
print('state.agents:', json.dumps(state['agents']))
print('spec agent ids:', [a['id'] for a in state['spec']['agents']])
model1 = s.request('model')
print('MODEL after patch agents:', json.dumps(model1['agents']))
w = [a for a in model1['agents'] if a['agent_id'] == 'w']
print('CHECK w.model_profile:', w[0]['model_profile'] if w else 'MISSING')
sm = s.request('set_model', {'agent_id': 'leader', 'model': 'gpt-5', 'effort': 'high'})
print('set_model result:', json.dumps(sm))
model_before = s.request('model', {})
print('MODEL_BEFORE agents:', json.dumps(model_before['agents']))
srcbase = SESS + '/' + src
print('source dir:', lsdir(srcbase))
print('profiles.json:', open(srcbase + '/profiles.json').read() if os.path.exists(srcbase + '/profiles.json') else 'MISSING')
print('model_overrides.json:', open(srcbase + '/model_overrides.json').read() if os.path.exists(srcbase + '/model_overrides.json') else 'MISSING')
con = sqlite3.connect(srcbase + '/team.db', timeout=15)
n = con.execute("UPDATE agent_runtime SET context_epoch=3 WHERE session_id=? AND agent_id='leader'", (src,)).rowcount
con.commit()
print('UPDATE context_epoch rowcount:', n)
print('agent_runtime:', con.execute("SELECT agent_id, context_epoch FROM agent_runtime WHERE session_id=?", (src,)).fetchall())
con.close()
print('epoch self-proof:', {a['id']: a['context_epoch'] for a in st()['agents']})
md = srcbase + '/members/leader'; os.makedirs(md, exist_ok=True)
tree = {"ctx:leader:3": {"nodes": [
    {"id": "n1", "parent": None, "message": {"role": "user", "content": "u1"}},
    {"id": "n2", "parent": "n1", "message": {"role": "assistant", "content": "a1"}},
    {"id": "n3", "parent": "n1", "message": {"role": "assistant", "content": "branch"}},
], "leaf": "n3"}}
hist = {"ctx:leader:3": [{"role": "user", "content": "u1"}]}
open(md + '/chat_tree.json', 'w').write(json.dumps(tree))
open(md + '/chat_history.json', 'w').write(json.dumps(hist))
src_files = {n: sha256_file(srcbase + '/' + n) for n in ['profiles.json', 'model_overrides.json']}
src_files['chat_tree.json'] = sha256_file(md + '/chat_tree.json')
src_files['chat_history.json'] = sha256_file(md + '/chat_history.json')
print('SOURCE hashes pre-fork:', json.dumps(src_files, sort_keys=True))
proj_before = tree_hash(PROJ)
print('PROJECT hashes pre-fork:', json.dumps(proj_before, sort_keys=True))
print('SESSIONS pre-fork:', lsdir(SESS))
forked = s.request('fork_session', {})
print('FORK result:', json.dumps(forked))
fork_id = forked['session_id']
print('forked_from:', forked.get('forked_from'), '| fork_id != src:', fork_id != src)
forkbase = SESS + '/' + fork_id
for name in ['profiles.json', 'model_overrides.json']:
    a = open(srcbase + '/' + name, 'rb').read(); b = open(forkbase + '/' + name, 'rb').read()
    print('BYTE-IDENTICAL %s:' % name, a == b, '(src %s / fork %s)' % (sha256_file(srcbase + '/' + name), sha256_file(forkbase + '/' + name)))
ftree = json.load(open(forkbase + '/members/leader/chat_tree.json'))
print('fork chat_tree keys:', sorted(ftree.keys()))
print('CHECK ctx:leader:1 == source live:', ftree.get('ctx:leader:1') == tree['ctx:leader:3'])
print('CHECK no ctx:leader:3:', 'ctx:leader:3' not in ftree)
print('fork node count/leaf:', len(ftree['ctx:leader:1']['nodes']), ftree['ctx:leader:1']['leaf'])
fhist = json.load(open(forkbase + '/members/leader/chat_history.json'))
print('fork history keys:', sorted(fhist.keys()), '| mapped:', fhist.get('ctx:leader:1') == hist['ctx:leader:3'])
src_files_after = {n: sha256_file(srcbase + '/' + n) for n in ['profiles.json', 'model_overrides.json']}
src_files_after['chat_tree.json'] = sha256_file(md + '/chat_tree.json')
src_files_after['chat_history.json'] = sha256_file(md + '/chat_history.json')
print('SOURCE files unchanged:', src_files == src_files_after)
model_after = s.request('model', {})
print('MODEL AFTER FORK == BEFORE:', model_after['agents'] == model_before['agents'])
print('agents after fork:', json.dumps(model_after['agents']))
print('close serve1:', s.close()); s.stop()
s2 = Serve(BIN, env, PROJ, BASE + '/serve2.err')
o2 = s2.request('open', {'cwd': PROJ, 'resume': fork_id})
print('REOPEN session_id:', o2.get('session_id'))
model_r = s2.request('model', {})
print('MODEL REOPEN == source before:', model_r['agents'] == model_before['agents'])
print('agents reopen:', json.dumps(model_r['agents']))
str_ = s2.request('call', {'method': 'state', 'params': {'include_events': False}})
print('reopen tasks:', json.dumps(str_['tasks']), '| runs:', json.dumps(str_['runs']))
print('reopen shared_entries:', json.dumps(s2.request('call', {'method': 'shared_entries', 'params': {}})))
print('REWIND_POINTS:', json.dumps(s2.request('rewind_points', {})))
print('close serve2:', s2.close()); s2.stop()
proj_after = tree_hash(PROJ)
print('PROJECT hashes unchanged:', proj_before == proj_after)
print('SESSIONS after:', lsdir(SESS))
```

### `/tmp/forkprobe/s2.py`

```python
import os, sys, json, shutil, fcntl
sys.path.insert(0, '/tmp/forkprobe')
from lib import Serve, sha256_file, lsdir
BIN = os.environ['TA_BIN']; BASE = '/tmp/forkprobe/s2'
shutil.rmtree(BASE, ignore_errors=True)
os.makedirs(BASE + '/config/teamagents'); os.makedirs(BASE + '/home')
PROJ = BASE + '/project'; os.makedirs(PROJ)
open(BASE + '/config/teamagents/config.toml', 'w').write(
 '[models.leader_main]\nprovider = "openai"\nprotocol = "openai"\nmodel = "test"\n'
 'base_url = "http://127.0.0.1:9"\nmax_retries = 0\n')
env = dict(os.environ)
env.update(XDG_STATE_HOME=BASE + '/state', XDG_CONFIG_HOME=BASE + '/config', HOME=BASE + '/home')
SESS = BASE + '/state/teamagents/sessions'
s = Serve(BIN, env, PROJ, BASE + '/serveK.err')
print('=== S2 ===')
o = s.request('open', {'cwd': PROJ, 'resume': 'proj_keep', 'scripts': {'leader': [['end']]}})
print('open K:', o['session_id'])
before = s.request('call', {'method': 'state', 'params': {'include_events': False}})
mb = s.request('model', {})
print('K before: session_id=%s cwd=%s mode=%s' % (before['session']['session_id'], before['session']['cwd'], before['session']['permissions_mode']))
print('model_before agents:', json.dumps(mb['agents']))
r = s.request('switch_session', {'session_id': '../evil'})
print('switch ../evil ->', json.dumps(r))
r = s.request('open', {'cwd': PROJ, 'resume': 'bad/id'})
print('open resume bad/id ->', json.dumps(r))
after = s.request('call', {'method': 'state', 'params': {'include_events': False}})
ma = s.request('model', {})
print('K after: session_id=%s cwd=%s mode=%s' % (after['session']['session_id'], after['session']['cwd'], after['session']['permissions_mode']))
print('state unchanged:', after['session']['session_id'] == before['session']['session_id'] and after['session']['cwd'] == before['session']['cwd'] and after['session']['permissions_mode'] == before['session']['permissions_mode'])
print('model unchanged:', ma['agents'] == mb['agents'])
um = s.request('user_message', {'text': 'still here'})
print('user_message after failures ok:', um.get('ok'))
lt = Serve(BIN, env, PROJ, BASE + '/serveL.err')
lt.request('open', {'cwd': PROJ, 'resume': 'proj_lock_target', 'scripts': {'leader': [['end']]}})
lt.close(); lt.stop()
print('L dir:', lsdir(SESS + '/proj_lock_target'))
lf = open(SESS + '/proj_lock_target/session.lock', 'r+')
fcntl.flock(lf, fcntl.LOCK_EX | fcntl.LOCK_NB)
print('python flock on L session.lock acquired: LOCK_EX|LOCK_NB')
r = s.request('switch_session', {'session_id': 'proj_lock_target'})
print('switch to flocked L ->', json.dumps(r))
print('K still usable after lock refusal:', s.request('call', {'method': 'state', 'params': {'include_events': False}})['session']['session_id'])
print('K model still:', s.request('model', {})['agents'] == mb['agents'])
print('K user_message still ok:', s.request('user_message', {'text': 'again'}).get('ok'))
fcntl.flock(lf, fcntl.LOCK_UN); lf.close()
r = s.request('open', {'cwd': PROJ, 'team': BASE + '/does-not-exist.json'})
print('open team nonexistent ->', json.dumps(r))
after2 = s.request('call', {'method': 'state', 'params': {'include_events': False}})
print('K after bad team: session_id=%s cwd=%s mode=%s' % (after2['session']['session_id'], after2['session']['cwd'], after2['session']['permissions_mode']))
print('K model unchanged after bad team:', s.request('model', {})['agents'] == mb['agents'])
print('close K:', s.close()); s.stop()
print('SESSIONS final:', lsdir(SESS))
```

### `/tmp/forkprobe/s3.py`

```python
import os, sys, json, shutil
sys.path.insert(0, '/tmp/forkprobe')
from lib import Serve, wait_until, lsdir
BIN = os.environ['TA_BIN']; BASE = '/tmp/forkprobe/s3'
shutil.rmtree(BASE, ignore_errors=True)
os.makedirs(BASE + '/config/teamagents'); os.makedirs(BASE + '/home')
PROJ = BASE + '/project'; os.makedirs(PROJ)
open(BASE + '/config/teamagents/config.toml', 'w').write(
 '[models.leader_main]\nprovider = "openai"\nprotocol = "openai"\nmodel = "test"\n'
 'base_url = "http://127.0.0.1:9"\nmax_retries = 0\n')
env = dict(os.environ)
env.update(XDG_STATE_HOME=BASE + '/state', XDG_CONFIG_HOME=BASE + '/config', HOME=BASE + '/home')
SESS = BASE + '/state/teamagents/sessions'
s = Serve(BIN, env, PROJ, BASE + '/serve.err')
print('=== S3 ===')
o = s.request('open', {'cwd': PROJ, 'resume': 'proj_busy', 'scripts': {'leader': [['sleep', 2.0], ['end']]}})
print('open proj_busy:', o['session_id'])
um = s.request('user_message', {'text': 'hold the turn'})
print('user_message ok:', um.get('ok'))
def active():
    st = s.request('call', {'method': 'state', 'params': {'include_events': False}})
    return [(r.get('agent_id'), r.get('status')) for r in st.get('runs', []) if r.get('status') in ('QUEUED', 'RUNNING')]
ok = wait_until(lambda: len(active()) > 0, timeout=5)
print('saw active run:', ok, active())
before = lsdir(SESS)
print('SESSIONS before refused fork:', before)
r = s.request('fork_session', {})
print('fork while running ->', json.dumps(r))
after = lsdir(SESS)
print('SESSIONS after refused fork:', after)
print('CHECK listing identical:', before == after)
print('CHECK error mentions 回合进行中:', '_error' in r and '回合进行中' in r['_error'])
print('wait turn end:', wait_until(lambda: len(active()) == 0, timeout=12))
r2 = s.request('fork_session', {})
print('fork after turn ->', json.dumps(r2))
print('CHECK fork succeeded and id differs:', '_error' not in r2 and r2.get('session_id') != 'proj_busy')
print('SESSIONS final:', lsdir(SESS))
print('close:', s.close()); s.stop()
```

### `/tmp/forkprobe/s4.py`

```python
import os, sys, json, shutil, hashlib
sys.path.insert(0, '/tmp/forkprobe')
from lib import Serve, lsdir
BIN = os.environ['TA_BIN']; BASE = '/tmp/forkprobe/s4'
shutil.rmtree(BASE, ignore_errors=True)
os.makedirs(BASE + '/config/teamagents'); os.makedirs(BASE + '/home')
PROJ = BASE + '/project'; os.makedirs(PROJ)
open(BASE + '/config/teamagents/config.toml', 'w').write(
 '[models.leader_main]\nprovider = "openai"\nprotocol = "openai"\nmodel = "test"\n'
 'base_url = "http://127.0.0.1:9"\nmax_retries = 0\n')
env = dict(os.environ)
env.update(XDG_STATE_HOME=BASE + '/state', XDG_CONFIG_HOME=BASE + '/config', HOME=BASE + '/home')
SESS = BASE + '/state/teamagents/sessions'
base_id = 'proj_' + hashlib.sha256(os.path.realpath(PROJ).encode()).hexdigest()[:12]
print('=== S4 ===  expected new_session_id(P) =', base_id)
s = Serve(BIN, env, PROJ, BASE + '/serve.err')
spec = {"leader_id": "leader", "agents": [
 {"id": "leader", "name": "Leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "leader_main"},
 {"id": "ghost", "name": "Ghost", "role": "worker", "runtime_kind": "deepagents", "model_profile": "ghost"}],
 "shared_spaces": [{"id": "main", "readers": ["leader", "ghost"], "writers": ["leader"]}]}
o = s.request('open', {'cwd': PROJ, 'resume': 'proj_ghost', 'scripts': {'leader': [['end']], 'ghost': [['end']]}, 'initial_spec': spec})
print('open ghost source:', o['session_id'])
before = lsdir(SESS)
print('SESSIONS before fork:', before)
r = s.request('fork_session', {})
print('fork (open fails) ->', json.dumps(r))
after = lsdir(SESS)
print('SESSIONS after fork:', after)
print('CHECK listing identical:', before == after)
print('CHECK error mentions ghost:', '_error' in r and 'ghost' in r['_error'])
print('CHECK target dir absent:', not os.path.exists(SESS + '/' + base_id))
st = s.request('call', {'method': 'state', 'params': {'include_events': False}})
print('CHECK current still source:', st['session']['session_id'] == 'proj_ghost')
print('CHECK source team.db exists:', os.path.isfile(SESS + '/proj_ghost/team.db'))
print('CHECK sessions/archived absent:', not os.path.exists(SESS + '/archived'))
spec2 = {"leader_id": "leader", "agents": [
 {"id": "leader", "name": "Leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "leader_main"}],
 "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader"]}]}
o2 = s.request('open', {'cwd': PROJ, 'resume': 'proj_plain', 'scripts': {'leader': [['end']]}, 'initial_spec': spec2})
print('open plain source:', o2['session_id'])
md = SESS + '/proj_plain/members/leader'; os.makedirs(md, exist_ok=True)
open(md + '/chat_tree.json', 'w').write('{not json')
before2 = lsdir(SESS)
r2 = s.request('fork_session', {})
print('fork (adopt fails) ->', json.dumps(r2))
after2 = lsdir(SESS)
print('CHECK listing identical (2):', before2 == after2)
print('CHECK error mentions fork:', '_error' in r2 and 'fork' in r2['_error'])
print('CHECK current still plain source:', s.request('call', {'method': 'state', 'params': {'include_events': False}})['session']['session_id'] == 'proj_plain')
print('CHECK sessions/archived still absent:', not os.path.exists(SESS + '/archived'))
print('SESSIONS final:', lsdir(SESS))
print('close:', s.close()); s.stop()
```

### `/tmp/forkprobe/s5.py`

```python
import os, sys, json, shutil, hashlib
sys.path.insert(0, '/tmp/forkprobe')
from lib import Serve, wait_until, lsdir, tree_hash, sha256_file
BIN = os.environ['TA_BIN']; BASE = '/tmp/forkprobe/s5'
shutil.rmtree(BASE, ignore_errors=True)
os.makedirs(BASE + '/config/teamagents'); os.makedirs(BASE + '/home')
PROJ = BASE + '/project'; os.makedirs(PROJ + '/deep/er')
open(PROJ + '/readme.md', 'w').write('# project\n')
open(PROJ + '/empty.txt', 'w').write('')
open(PROJ + '/deep/er/data.bin', 'wb').write(bytes(range(256)) * 40)
open(PROJ + '/deep/er/notes.txt', 'w').write('nested note\n')
open(BASE + '/config/teamagents/config.toml', 'w').write(
 '[models.leader_main]\nprovider = "openai"\nprotocol = "openai"\nmodel = "test"\n'
 'base_url = "http://127.0.0.1:9"\nmax_retries = 0\n')
env = dict(os.environ)
env.update(XDG_STATE_HOME=BASE + '/state', XDG_CONFIG_HOME=BASE + '/config', HOME=BASE + '/home')
SESS = BASE + '/state/teamagents/sessions'
s = Serve(BIN, env, PROJ, BASE + '/serve.err')
print('=== S5 ===')
o = s.request('open', {'cwd': PROJ, 'resume': 'proj_disk', 'scripts': {'leader': [['end']]}})
print('open:', o['session_id'])
print('user_message:', s.request('user_message', {'text': 'hi'}).get('ok'))
def idle():
    st = s.request('call', {'method': 'state', 'params': {'include_events': False}})
    return not any(r.get('status') in ('QUEUED', 'RUNNING') for r in st['runs'])
print('wait idle:', wait_until(idle, 10))
srcbase = SESS + '/proj_disk'
md = srcbase + '/members/leader'; os.makedirs(md, exist_ok=True)
open(md + '/chat_tree.json', 'w').write(json.dumps({"ctx:leader:1": {"nodes": [
  {"id": "x1", "parent": None, "message": {"role": "user", "content": "u"}}], "leaf": "x1"}}))
open(md + '/chat_history.json', 'w').write(json.dumps({"ctx:leader:1": [{"role": "user", "content": "u"}]}))
open(srcbase + '/profiles.json', 'w').write('{"seed":{"provider":"openai","protocol":"openai","model":"test"}}')
proj_before = tree_hash(PROJ)
print('PROJECT files pre-fork:', json.dumps(proj_before, sort_keys=True))
src_files = {n: sha256_file(srcbase + '/' + n) for n in ['profiles.json']}
src_files['chat_tree.json'] = sha256_file(md + '/chat_tree.json')
src_files['chat_history.json'] = sha256_file(md + '/chat_history.json')
print('SOURCE hashes pre-fork:', json.dumps(src_files, sort_keys=True))
r = s.request('fork_session', {})
fork_id = r['session_id']; print('fork:', fork_id)
proj_after = tree_hash(PROJ)
print('PROJECT files post-fork:', json.dumps(proj_after, sort_keys=True))
print('CHECK PROJECT listing+bytes unchanged:', proj_before == proj_after)
src_after = {n: sha256_file(srcbase + '/' + n) for n in ['profiles.json']}
src_after['chat_tree.json'] = sha256_file(md + '/chat_tree.json')
src_after['chat_history.json'] = sha256_file(md + '/chat_history.json')
print('CHECK SOURCE files unchanged:', src_files == src_after)
st = s.request('call', {'method': 'state', 'params': {'include_events': False}})
print('fork session state session_id:', st['session']['session_id'])
print('CHECK fork tasks empty:', st['tasks'] == [])
print('CHECK fork runs empty:', st['runs'] == [])
print('CHECK fork shared_entries empty:', s.request('call', {'method': 'shared_entries', 'params': {}}) == {'entries': []})
print('close:', s.close()); s.stop()
```

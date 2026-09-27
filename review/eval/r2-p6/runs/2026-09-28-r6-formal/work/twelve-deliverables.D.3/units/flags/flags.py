"""命令行风格的参数解析。"""


class Impl:
    def parse(self, argv):
        """返回 {"values": {...}, "flags": {...}, "positional": [...]}：
        - 不以 "-" 开头、或正好是 "-" 的项，按出现顺序进 positional。
        - `--` 之后的所有项都是位置参数（`--` 自己不出现在结果里）。
        - `--key=value` 与 `--key value` 都是键值对，键是 key。
        - `--key` 后面没有可用值（到结尾，或后面紧跟另一个 "--" 开头的项）时它是开关。
        - `-abc` 是三个开关 a、b、c。
        - values 的每个值是**列表**，重复的键按出现顺序累积；开关重复出现仍是 True。"""
        values = {}
        flags = {}
        positional = []
        n = len(argv)
        i = 0
        terminated = False
        while i < n:
            arg = argv[i]
            if terminated:
                positional.append(arg)
                i += 1
                continue
            if arg == "--":
                # `--` 终止选项解析，自身不进入结果。
                terminated = True
                i += 1
                continue
            if arg.startswith("--"):
                body = arg[2:]
                key, sep, value = body.partition("=")
                if sep:
                    # `--key=value`：值始终存在（即使是空串或像开关的字符串）。
                    values.setdefault(key, []).append(value)
                    i += 1
                    continue
                nxt = argv[i + 1] if i + 1 < n else None
                if nxt is not None and not nxt.startswith("--"):
                    # `--key value`：可用值（允许以单个 "-" 开头）。
                    values.setdefault(key, []).append(nxt)
                    i += 2
                else:
                    # 到结尾或后面紧跟 "--" 开头的项 => 开关。
                    flags[key] = True
                    i += 1
                continue
            if len(arg) > 1 and arg.startswith("-"):
                # `-abc` => 开关 a、b、c。
                for ch in arg[1:]:
                    flags[ch] = True
                i += 1
                continue
            positional.append(arg)
            i += 1
        return {"values": values, "flags": flags, "positional": positional}

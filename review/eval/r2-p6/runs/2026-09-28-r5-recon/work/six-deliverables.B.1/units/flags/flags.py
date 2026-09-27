"""命令行风格的参数解析。"""


class Impl:
    def parse(self, argv):
        """返回 {"values": {...}, "flags": {...}, "positional": [...]}：
        - 不以 "-" 开头、或正好是 "-" 的项，按出现顺序进 positional。
        - `--` 之后的所有项都是位置参数（`--` 自己不出现在结果里）。
        - `--key=value` 与 `--key value` 都是键值对，键是 key。
        - `--key` 后面没有可用值（到结尾，或后面紧跟另一个 "--" 开头的项）时它是开关。
          （单个 "-" 开头的项仍可作值，例如 `--k -5`。）
        - `-abc` 是三个开关 a、b、c；`-key=value` 视作键 key、值 value。
        - values 的每个值是**列表**，重复的键按出现顺序累积；开关重复出现仍是 True。
        """
        values = {}
        flags = {}
        positional = []
        i = 0
        n = len(argv)
        end_of_options = False

        def add_value(key, value):
            values.setdefault(key, []).append(value)

        while i < n:
            tok = argv[i]

            if end_of_options:
                positional.append(tok)
                i += 1
                continue

            if tok == "--":
                end_of_options = True
                i += 1
                continue

            if tok.startswith("--"):
                body = tok[2:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    add_value(key, value)
                    i += 1
                    continue
                key = body
                if i + 1 < n and not argv[i + 1].startswith("--"):
                    add_value(key, argv[i + 1])
                    i += 2
                    continue
                flags[key] = True
                i += 1
                continue

            if tok != "-" and tok.startswith("-") and len(tok) > 1:
                body = tok[1:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    add_value(key, value)
                else:
                    for ch in body:
                        flags[ch] = True
                i += 1
                continue

            positional.append(tok)
            i += 1

        return {"values": values, "flags": flags, "positional": positional}

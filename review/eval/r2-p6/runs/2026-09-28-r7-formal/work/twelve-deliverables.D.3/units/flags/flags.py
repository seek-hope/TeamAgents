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

        def add_value(key, value):
            values.setdefault(key, []).append(value)

        i = 0
        n = len(argv)
        only_positional = False
        while i < n:
            token = argv[i]
            i += 1

            if only_positional:
                positional.append(token)
                continue

            if token == "--":
                # 终止符本身不出现，其后所有项都是位置参数
                only_positional = True
                continue

            if token.startswith("--"):
                body = token[2:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    add_value(key, value)
                elif i < n and not argv[i].startswith("--"):
                    # 后面的项是可用的值（只有 "--" 开头的项会被拒绝）
                    add_value(body, argv[i])
                    i += 1
                else:
                    flags[body] = True
            elif token.startswith("-") and token != "-":
                for ch in token[1:]:
                    flags[ch] = True
            else:
                positional.append(token)

        return {"values": values, "flags": flags, "positional": positional}

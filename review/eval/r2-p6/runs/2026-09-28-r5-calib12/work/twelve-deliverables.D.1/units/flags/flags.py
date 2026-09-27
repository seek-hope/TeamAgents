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
        while i < n:
            item = argv[i]

            if item == "--":
                positional.extend(argv[i + 1:])
                break

            if not item.startswith("-") or item == "-":
                positional.append(item)
                i += 1
                continue

            if item.startswith("--"):
                body = item[2:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    add_value(key, value)
                    i += 1
                    continue
                key = body
                nxt = argv[i + 1] if i + 1 < n else None
                if nxt is not None and not nxt.startswith("--"):
                    add_value(key, nxt)
                    i += 2
                else:
                    flags[key] = True
                    i += 1
                continue

            # single dash, multiple characters: bundle of short switches
            for ch in item[1:]:
                flags[ch] = True
            i += 1

        return {"values": values, "flags": flags, "positional": positional}

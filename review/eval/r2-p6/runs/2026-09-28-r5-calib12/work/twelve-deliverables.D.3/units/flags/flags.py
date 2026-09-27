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

        i = 0
        n = len(argv)
        only_positional = False

        while i < n:
            token = argv[i]

            if only_positional:
                positional.append(token)
            elif token == "--":
                # 终止符本身不进入结果，其后的所有项都是位置参数。
                only_positional = True
            elif token.startswith("--"):
                body = token[2:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    values.setdefault(key, []).append(value)
                elif i + 1 < n and not argv[i + 1].startswith("--"):
                    # 下一个项可用作值（即使它看起来像单短横的开关）。
                    values.setdefault(body, []).append(argv[i + 1])
                    i += 1
                else:
                    flags[body] = True
            elif token.startswith("-") and token != "-":
                # 单短横：`-abc` 展开为开关 a、b、c。
                for ch in token[1:]:
                    flags[ch] = True
            else:
                # 不以 "-" 开头，或正好是 "-"。
                positional.append(token)

            i += 1

        return {"values": values, "flags": flags, "positional": positional}

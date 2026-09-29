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
        while i < n:
            item = argv[i]

            if item == "--":
                # 终止符：其后所有项都是位置参数，终止符本身不出现。
                positional.extend(argv[i + 1:])
                break

            if item.startswith("--"):
                key, sep, inline = item[2:].partition("=")
                if sep:
                    values.setdefault(key, []).append(inline)
                elif i + 1 < n and not argv[i + 1].startswith("--"):
                    values.setdefault(key, []).append(argv[i + 1])
                    i += 1
                else:
                    flags[key] = True

            elif item.startswith("-") and item != "-":
                for char in item[1:]:
                    flags[char] = True

            else:
                positional.append(item)

            i += 1

        return {"values": values, "flags": flags, "positional": positional}

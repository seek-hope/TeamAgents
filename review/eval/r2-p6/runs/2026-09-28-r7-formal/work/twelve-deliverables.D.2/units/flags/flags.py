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

        items = list(argv)
        n = len(items)
        i = 0
        while i < n:
            item = items[i]

            # 裸 "--"：其后全部是位置参数，它本身不出现。
            if item == "--":
                positional.extend(items[i + 1:])
                break

            # 不以 "-" 开头，或正好是 "-"：位置参数。
            if not item.startswith("-") or item == "-":
                positional.append(item)
                i += 1
                continue

            if item.startswith("--"):
                body = item[2:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    values.setdefault(key, []).append(value)
                    i += 1
                    continue

                key = body
                # 只有下一个以 "--" 开头的项才阻止取值（"-5" 仍可作值）。
                if i + 1 < n and not items[i + 1].startswith("--"):
                    values.setdefault(key, []).append(items[i + 1])
                    i += 2
                    continue

                flags[key] = True
                i += 1
                continue

            # 短开关，如 "-abc" => a、b、c 三个开关。
            for ch in item[1:]:
                flags[ch] = True
            i += 1

        return {"values": values, "flags": flags, "positional": positional}

"""命令行风格的参数解析。"""


class Impl:
    def parse(self, argv):
        """解析 argv，返回 ``{"values": {...}, "flags": {...}, "positional": [...]}``。

        语义边界：
          - 不以 "-" 开头、或正好是 "-" 的项，按出现顺序进 positional；
          - ``--`` 之后的所有项都是位置参数（``--`` 自己不出现在结果里）；
          - ``--key=value`` 与 ``--key value`` 都是键值对，键是 key；
          - ``--key`` 后面没有可用值（到结尾，或后面紧跟另一个以 "--" 开头的项）
            时它是开关；
          - ``-abc`` 是三个开关 a、b、c（短选项一律按开关处理，不吞值）；
          - values 的每个值是**列表**，重复的键按出现顺序累积；
            开关重复出现仍是 True（而同一个键也可以既有值又当开关）。
        """
        values = {}
        flags = {}
        positional = []
        after_terminator = False
        i = 0
        n = len(argv)

        while i < n:
            token = argv[i]

            if after_terminator:
                positional.append(token)
                i += 1
                continue

            if token == "--":
                after_terminator = True
                i += 1
                continue

            if token.startswith("--"):
                body = token[2:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    values.setdefault(key, []).append(value)
                    i += 1
                elif i + 1 < n and not argv[i + 1].startswith("--"):
                    key = body
                    values.setdefault(key, []).append(argv[i + 1])
                    i += 2
                else:
                    flags[body] = True
                    i += 1
                continue

            if token.startswith("-") and token != "-":
                for ch in token[1:]:
                    flags[ch] = True
                i += 1
                continue

            positional.append(token)
            i += 1

        return {"values": values, "flags": flags, "positional": positional}

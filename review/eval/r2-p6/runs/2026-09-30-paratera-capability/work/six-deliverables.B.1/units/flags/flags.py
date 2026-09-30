"""命令行风格的参数解析。

语义边界：
- 不以 ``-`` 开头、或正好是 ``-`` 的项，按出现顺序进入 ``positional``。
- 单独的 ``--`` 是终止符：其后的所有项（包括看起来像选项的）都是位置参数，
  ``--`` 自身不出现在结果里。
- ``--key=value`` 与 ``--key value`` 都是键值对，键为 ``key``；
  ``--key`` 后面没有可用值（到结尾，或下一项以 ``--`` 开头）时是开关。
  单短横线开头的项（如 ``-5``）可以作为值被消费。
- ``-abc`` 展开为开关 ``a``、``b``、``c``。
- ``values`` 的每个值是列表，重复键按出现顺序累积；开关重复出现仍是 ``True``。
"""


class Impl:
    def parse(self, argv):
        values = {}
        flags = {}
        positional = []

        i = 0
        n = len(argv)
        terminated = False
        while i < n:
            tok = argv[i]

            if terminated:
                positional.append(tok)
                i += 1
                continue

            if tok == "--":
                terminated = True
                i += 1
                continue

            if tok.startswith("--"):
                body = tok[2:]
                if "=" in body:
                    key, value = body.split("=", 1)
                    values.setdefault(key, []).append(value)
                    i += 1
                    continue
                has_value = i + 1 < n and not argv[i + 1].startswith("--")
                if has_value:
                    values.setdefault(body, []).append(argv[i + 1])
                    i += 2
                else:
                    flags[body] = True
                    i += 1
                continue

            if tok.startswith("-") and tok != "-":
                for ch in tok[1:]:
                    flags[ch] = True
                i += 1
                continue

            positional.append(tok)
            i += 1

        return {"values": values, "flags": flags, "positional": positional}

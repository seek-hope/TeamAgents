"""命令行风格的参数解析。"""


class Impl:
    def parse(self, argv):
        """返回 {"values": {...}, "flags": {...}, "positional": [...]}。"""
        values = {}
        flags = {}
        positional = []
        i = 0
        n = len(argv)
        while i < n:
            token = argv[i]
            if token == "--":
                positional.extend(argv[i + 1:])
                break
            if token.startswith("--"):
                body = token[2:]
                if "=" in body:
                    key, val = body.split("=", 1)
                    values.setdefault(key, []).append(val)
                    i += 1
                    continue
                key = body
                if i + 1 < n and not argv[i + 1].startswith("--"):
                    values.setdefault(key, []).append(argv[i + 1])
                    i += 2
                else:
                    flags[key] = True
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

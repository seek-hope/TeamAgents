class Impl:
    def slugify(self, text):
        """小写化、把连续的非字母数字折叠成单个 '-'、去首尾 '-'；空结果返回 'untitled'。"""
        out = []
        at_separator = True  # 结果开头不允许出现 '-'
        for ch in text:
            if ch.isalnum():
                out.append(ch.lower())
                at_separator = False
            elif not at_separator:
                out.append("-")
                at_separator = True
        return "".join(out).strip("-") or "untitled"

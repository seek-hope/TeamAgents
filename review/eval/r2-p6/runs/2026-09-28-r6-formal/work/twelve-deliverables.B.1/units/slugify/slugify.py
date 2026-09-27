class Impl:
    def slugify(self, text):
        """小写化、把连续的非字母数字折叠成单个 "-"、去掉首尾 "-"，空结果返回 "untitled"。"""
        out = []
        prev_dash = False
        for ch in text.lower():
            if ch.isalnum():
                out.append(ch)
                prev_dash = False
            elif out and not prev_dash:
                out.append("-")
                prev_dash = True
        slug = "".join(out).strip("-")
        return slug or "untitled"

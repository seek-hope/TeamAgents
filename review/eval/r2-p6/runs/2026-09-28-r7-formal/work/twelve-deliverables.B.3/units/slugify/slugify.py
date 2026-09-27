class Impl:
    def slugify(self, text):
        """小写化；把连续的非字母数字折叠成一个 "-"；去掉首尾 "-"；空结果返回 "untitled"。"""
        out = []
        pending_dash = False
        for ch in text.lower():
            if ch.isalnum():
                out.append(ch)
                pending_dash = False
            elif out and not pending_dash:
                out.append("-")
                pending_dash = True
        slug = "".join(out).strip("-")
        return slug or "untitled"

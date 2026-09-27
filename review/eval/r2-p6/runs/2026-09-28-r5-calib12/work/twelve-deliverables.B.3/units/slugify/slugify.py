import re


class Impl:
    def slugify(self, text):
        """小写化、把非字母数字折叠成单个 "-"、去掉首尾 "-"；空结果返回 "untitled"。"""
        slug = re.sub(r"[^a-z0-9]+", "-", str(text).lower())
        slug = slug.strip("-")
        return slug or "untitled"

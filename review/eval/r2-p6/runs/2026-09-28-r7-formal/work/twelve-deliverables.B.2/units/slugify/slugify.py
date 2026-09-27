import re


class Impl:
    def slugify(self, text):
        """小写化，把连续的非字母数字折叠成一个 "-"，去掉首尾 "-"；
        结果为空时返回 "untitled"。
        """
        slug = re.sub(r"[^a-z0-9]+", "-", str(text).lower()).strip("-")
        return slug or "untitled"

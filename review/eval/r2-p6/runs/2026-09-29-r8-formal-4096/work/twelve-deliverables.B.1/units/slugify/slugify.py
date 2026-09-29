import re


class Impl:
    def slugify(self, text):
        """小写化后把非字母数字的连续片段折叠成单个 "-"，去掉首尾 "-"；
        结果为空时返回 "untitled"。"""
        slug = re.sub(r"[\W_]+", "-", text.lower()).strip("-")
        return slug if slug else "untitled"

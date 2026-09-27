import re

_NON_ALNUM = re.compile(r"[^a-z0-9]+")


class Impl:
    def slugify(self, text):
        """小写化，把非字母数字的连续片段折叠成单个 "-"，去掉首尾 "-"；
        结果为空时返回 "untitled"。"""
        slug = _NON_ALNUM.sub("-", text.lower())
        slug = slug.strip("-")
        return slug if slug else "untitled"

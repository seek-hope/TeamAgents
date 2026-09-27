import re


class Impl:
    _NON_ALNUM = re.compile(r"[^a-z0-9]+")

    def slugify(self, text):
        """小写化、把非字母数字折叠成单个 '-'、去首尾 '-'；空结果返回 'untitled'。"""
        slug = self._NON_ALNUM.sub("-", text.lower()).strip("-")
        return slug or "untitled"

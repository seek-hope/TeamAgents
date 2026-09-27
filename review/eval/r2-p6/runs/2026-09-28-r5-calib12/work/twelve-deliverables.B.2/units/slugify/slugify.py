import re

_NON_ALNUM = re.compile(r"[^a-z0-9]+")


class Impl:
    def slugify(self, text):
        """把文本转成 slug。

        语义边界：
          - 先整体小写化；
          - 把所有非字母数字（a-z、0-9）的连续片段折叠成一个 ``-``；
          - 去掉首尾的 ``-``；
          - 结果为空时返回 ``"untitled"``。
        """
        slug = _NON_ALNUM.sub("-", text.lower()).strip("-")
        return slug or "untitled"

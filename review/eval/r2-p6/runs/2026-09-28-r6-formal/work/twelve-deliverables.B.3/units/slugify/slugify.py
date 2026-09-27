import re
import unicodedata


class Impl:
    def slugify(self, text):
        """生成 slug：小写化、折叠非字母数字为单个 "-"、去掉首尾 "-"。

        结果为空时返回 "untitled"。非 ASCII 字母先做 NFKD 分解并丢弃组合符号
        （例如 "café" -> "cafe"）。
        """
        text = unicodedata.normalize("NFKD", text)
        text = text.encode("ascii", "ignore").decode("ascii")
        text = text.lower()
        text = re.sub(r"[^a-z0-9]+", "-", text)
        text = text.strip("-")
        return text or "untitled"

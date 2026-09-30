import re


class Impl:
    _NON_ALNUM = re.compile(r"[^a-z0-9]+")

    def slugify(self, text):
        slug = self._NON_ALNUM.sub("-", text.lower()).strip("-")
        return slug or "untitled"

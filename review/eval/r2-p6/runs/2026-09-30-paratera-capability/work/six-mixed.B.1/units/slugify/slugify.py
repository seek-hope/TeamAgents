import re


class Impl:
    _NON_ALNUM = re.compile(r"[\W_]+", re.UNICODE)

    def slugify(self, text):
        slug = self._NON_ALNUM.sub("-", text.lower()).strip("-")
        return slug if slug else "untitled"

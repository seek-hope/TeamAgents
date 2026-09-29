import re

_RUNS = re.compile(r"[^a-z0-9]+")


class Impl:
    def slugify(self, text):
        slug = _RUNS.sub("-", text.lower()).strip("-")
        return slug or "untitled"

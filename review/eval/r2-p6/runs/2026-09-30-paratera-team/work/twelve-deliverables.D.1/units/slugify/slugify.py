import re


class Impl:
    def slugify(self, text):
        slug = re.sub(r"[^a-zA-Z0-9]+", "-", str(text).lower()).strip("-")
        return slug or "untitled"

import re


class Impl:
    def slugify(self, text):
        slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
        return slug or "untitled"

import re


class Impl:
    def slugify(self, text):
        slug = re.sub(r"[^0-9a-zA-Z]+", "-", text.lower())
        slug = slug.strip("-")
        return slug or "untitled"

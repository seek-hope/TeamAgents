import re


class Impl:
    def slugify(self, text):
        slug = re.sub(r"[^a-z0-9]+", "-", str(text).lower())
        return slug.strip("-") or "untitled"

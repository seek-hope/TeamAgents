import re


class Impl:
    def slugify(self, text):
        slug = re.sub(r"[^a-z0-9]+", "-", text.lower())
        slug = slug.strip("-")
        return slug if slug else "untitled"

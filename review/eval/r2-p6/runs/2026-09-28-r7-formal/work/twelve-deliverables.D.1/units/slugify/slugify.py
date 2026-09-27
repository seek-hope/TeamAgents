import re


class Impl:
    def slugify(self, text):
        text = str(text).lower()
        slug = re.sub(r"[^a-z0-9]+", "-", text)
        slug = slug.strip("-")
        return slug if slug else "untitled"

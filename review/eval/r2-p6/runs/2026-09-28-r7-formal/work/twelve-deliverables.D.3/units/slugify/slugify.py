import re


class Impl:
    def slugify(self, text):
        """Lowercase `text`, collapse runs of non-alphanumeric characters into
        a single "-", strip leading/trailing "-", and fall back to "untitled"
        when nothing remains."""
        slug = re.sub(r"[^a-z0-9]+", "-", str(text).lower())
        slug = slug.strip("-")
        return slug if slug else "untitled"

import re


class Impl:
    def slugify(self, text):
        """Convert text into a URL-friendly slug.

        Lowercases the text, collapses every run of non-alphanumeric
        characters into a single "-", strips leading/trailing "-", and
        returns "untitled" when nothing meaningful remains.
        """
        slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
        return slug or "untitled"

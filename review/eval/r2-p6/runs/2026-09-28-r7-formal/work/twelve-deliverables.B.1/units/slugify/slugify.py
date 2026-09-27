import re


class Impl:
    def slugify(self, text):
        """Lowercase ``text``, collapse every non-alphanumeric run to a single
        ``-``, strip leading/trailing ``-``; an empty result becomes ``untitled``."""
        slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
        return slug or "untitled"

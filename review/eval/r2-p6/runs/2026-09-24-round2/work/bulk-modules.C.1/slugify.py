import re


class Impl:
    """Turn arbitrary text into a URL-friendly slug."""

    _NON_ALNUM = re.compile(r"[^a-z0-9]+")

    def slugify(self, text):
        """Lowercase ``text``, collapse runs of non-alphanumerics into a single
        ``-``, strip leading/trailing ``-``; return ``untitled`` if empty."""
        slug = self._NON_ALNUM.sub("-", text.lower()).strip("-")
        return slug or "untitled"

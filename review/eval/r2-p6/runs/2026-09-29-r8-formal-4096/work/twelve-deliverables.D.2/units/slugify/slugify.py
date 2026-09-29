"""Slug generation implementation."""

import re

_NON_SLUG_RUN = re.compile(r"-+")


class Impl:
    def slugify(self, text):
        """Return a URL-friendly slug for ``text``.

        Rules:
        * lowercase the input
        * every non-alphanumeric character becomes "-"
        * runs of "-" collapse into a single "-"
        * leading/trailing "-" are stripped
        * an empty result becomes "untitled"
        """
        chars = [
            ch.lower() if ch.isalnum() else "-"
            for ch in str(text)
        ]
        slug = _NON_SLUG_RUN.sub("-", "".join(chars)).strip("-")
        return slug or "untitled"

import re


class Impl:
    """Slug helper.

    ``slugify`` lowercases the input, replaces every run of non-alphanumeric
    characters with a single ``-``, strips leading/trailing ``-`` and falls
    back to ``"untitled"`` when nothing usable remains.
    """

    def slugify(self, text):
        chars = [ch if ch.isalnum() else "-" for ch in str(text).lower()]
        slug = re.sub(r"-+", "-", "".join(chars)).strip("-")
        return slug or "untitled"

import re


class Impl:
    def slugify(self, text):
        """Convert ``text`` into a URL-friendly slug.

        The transformation is:

        * lowercase the text,
        * replace every run of non-alphanumeric characters with a single ``"-"``,
        * strip leading and trailing ``"-"`` characters,
        * return ``"untitled"`` when the result would be empty.
        """
        if not isinstance(text, str):
            text = "" if text is None else str(text)
        # Keep alphanumeric characters, turn everything else into a separator.
        separated = "".join(c if c.isalnum() else "-" for c in text.lower())
        # Collapse runs of separators and trim the edges.
        slug = re.sub(r"-+", "-", separated).strip("-")
        return slug or "untitled"

import re


class Impl:
    def slugify(self, text):
        text = "" if text is None else str(text)
        lowered = text.lower()
        # Replace each maximal run of non-alphanumeric characters with a single "-".
        slug = re.sub(r"[^a-z0-9]+", "-", lowered)
        slug = slug.strip("-")
        if slug == "":
            return "untitled"
        return slug

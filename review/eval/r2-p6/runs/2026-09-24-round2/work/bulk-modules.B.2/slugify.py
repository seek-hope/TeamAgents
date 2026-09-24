import re


class Impl:
    def slugify(self, text):
        text = str(text).lower()
        text = re.sub(r"[\W_]+", "-", text, flags=re.UNICODE)
        text = text.strip("-")
        return text or "untitled"

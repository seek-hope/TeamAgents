class Impl:
    def slugify(self, text):
        """Lower-case ``text``, collapse runs of non-alphanumerics into a
        single ``-``, strip leading/trailing ``-``; blank results become
        ``untitled``.
        """
        out = []
        pending_dash = False
        for ch in text.lower():
            if ch.isalnum():
                if pending_dash and out:
                    out.append("-")
                pending_dash = False
                out.append(ch)
            else:
                pending_dash = True
        slug = "".join(out)
        return slug if slug else "untitled"

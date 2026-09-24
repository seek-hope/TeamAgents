"""CSV normalization helpers."""


def normalize(path):
    """Read a headerless, comma-separated UTF-8 file and return list[list[str]].

    Semantics:
    - Each field is stripped of leading/trailing whitespace.
    - Blank lines (empty or whitespace only) are ignored.
    - Lines whose first non-whitespace character is ``#`` are treated as
      full-line comments and ignored.
    - Row order is preserved.
    - A missing file yields an empty list (rather than raising).
    """
    try:
        with open(path, "r", encoding="utf-8") as fh:
            lines = fh.read().splitlines()
    except FileNotFoundError:
        return []

    rows = []
    for line in lines:
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith("#"):
            continue
        rows.append([field.strip() for field in split_fields(line)])
    return rows


def split_fields(line):
    """Split a CSV line on commas.

    Kept deliberately simple (no quote handling) to match the plain
    comma-separated text format used by this project.
    """
    return line.split(",")

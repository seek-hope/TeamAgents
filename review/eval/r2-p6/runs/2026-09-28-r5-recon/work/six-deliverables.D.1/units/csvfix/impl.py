"""Tiny CSV helpers for three-field records (name, quantity, amount).

Public API
----------
parse(line) -> list[str] | None
    Split one CSV line, trim blank padding, and return the three fields.
    Returns ``None`` when the line is structurally invalid (wrong column
    count or broken quoting).

total(rows) -> int
    Sum the amount column (3rd field) across rows, ignoring blank amounts.
    Raises ``ValueError`` when a row is structurally invalid or holds a
    non-integer amount.
"""

REQUIRED_COLUMNS = 3
_WHITESPACE = " \t\r\n"


def _fields(line):
    """Split ``line`` on commas, honouring double-quote quoting.

    - Unquoted fields are trimmed of surrounding whitespace.
    - A field wrapped in double quotes keeps its content verbatim; ``""``
      inside such a field is an escaped double quote.
    - Leading whitespace before an opening quote and trailing whitespace
      after a closing quote are ignored.
    - Returns ``None`` for broken quoting (unterminated quote, or junk
      between a closing quote and the next comma / end of line).
    """
    fields = []
    i = 0
    n = len(line)

    while i < n:
        # Skip the blank padding that precedes a field.
        j = i
        while j < n and line[j] in _WHITESPACE:
            j += 1

        if j < n and line[j] == '"':
            # Quoted field.
            buf = []
            j += 1
            closed = False
            while j < n:
                ch = line[j]
                if ch == '"':
                    if j + 1 < n and line[j + 1] == '"':
                        buf.append('"')
                        j += 2
                        continue
                    closed = True
                    j += 1
                    break
                buf.append(ch)
                j += 1
            if not closed:
                return None  # unterminated quote

            # Only whitespace may separate the closing quote from the
            # delimiter (or the end of the line).
            k = j
            while k < n and line[k] in _WHITESPACE:
                k += 1
            if k < n and line[k] != ",":
                return None  # junk after closing quote

            field = "".join(buf)
            i = k
        else:
            # Unquoted field: read up to the next comma, trim afterwards.
            j = i
            while j < n and line[j] != ",":
                j += 1
            field = line[i:j].strip(_WHITESPACE)
            i = j

        fields.append(field)

        if i < n and line[i] == ",":
            i += 1
            if i == n:
                fields.append("")  # trailing empty field

    return fields


def parse(line):
    """Parse one CSV line into exactly three trimmed fields.

    Returns ``None`` when the line does not contain exactly three columns
    or when its quoting is malformed.
    """
    fields = _fields(line)
    if fields is None or len(fields) != REQUIRED_COLUMNS:
        return None
    return fields


def total(rows):
    """Sum the amount (3rd) column of ``rows``.

    Blank amounts (``""`` or whitespace-only) are skipped rather than
    treated as 0-without-a-trace: they simply contribute nothing. Any
    structurally invalid row or non-integer amount is reported by raising
    ``ValueError`` instead of being silently swallowed.
    """
    running = 0
    for index, row in enumerate(rows):
        fields = list(row)
        if len(fields) != REQUIRED_COLUMNS:
            raise ValueError(
                "row %d: expected %d columns, got %d"
                % (index, REQUIRED_COLUMNS, len(fields))
            )
        raw = fields[2].strip(_WHITESPACE)
        if not raw:
            continue  # blank amount contributes nothing
        try:
            running += int(raw)
        except ValueError:
            raise ValueError("row %d: bad amount %r" % (index, fields[2])) from None
    return running

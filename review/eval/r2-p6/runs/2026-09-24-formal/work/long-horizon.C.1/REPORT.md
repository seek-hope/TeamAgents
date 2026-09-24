# REPORT — `tools/` CSV normalize + word counts

## Files written

- `tools/__init__.py` — re-exports `normalize` and `word_counts`.
- `tools/normalize.py` — `normalize(path) -> list[list[str]]`.
- `tools/stat.py` — `word_counts(rows) -> dict[str, int]`.
- `REPORT.md` — this file.

No file under `tests/` was modified.

## Semantics / edge-case boundaries

`normalize(path)`
- Reads `path` as UTF-8 text with `newline=""`, parses with `csv.reader`
  (comma-separated, **no header row**).
- Each field is passed through `str.strip()`, so leading/trailing whitespace
  (spaces, tabs, `\r`) is removed.
- **Blank lines**: a line that is empty after stripping is skipped. It produces
  no row and does not affect order.
- **Whole-line comments**: a line whose first non-whitespace character is `#`
  (`raw.lstrip().startswith("#")`) is skipped entirely. `#` has **no** special
  meaning inside a data line (it is kept as an ordinary field character).
- **Row order** is preserved exactly as in the file.
- **Missing file**: `FileNotFoundError` is caught and `[]` is returned.
  Other OS errors are not swallowed.
- Because parsing is line-by-line with `csv.reader` on a single-element list,
  a physical line is always exactly one record; quoted fields containing
  commas are still handled by `csv`, but embedded newlines are not supported.
- **Missing/extra fields**: no padding or truncation — a data row with fewer
  than three fields stays short; a row with more keeps all fields.

`word_counts(rows)`
- Counts occurrences of the **third field** (`row[2]`) using
  `collections.Counter`, returned as a plain `dict`.
- Rows with fewer than three fields are skipped.
- Third fields that are empty (`""`) are skipped (no `""` key).
- Keys are compared as exact strings after normalization (case-sensitive).

`tools/__init__.py` exports `normalize` and `word_counts` via `__all__`.

## Commands actually run and their results

```
$ cat -A data/sample.csv
  alice , 3 ,apple$
$
bob,5,pear$
alice,2, apple $
# comment$
carol,,plum$
```

```
$ python3 tests/run_tests.py
long-horizon ok
exit=0
```

Additional ad-hoc edge-case check (run once, output as shown):

```
$ python3 - <<'PY'
from tools import normalize, word_counts
print("missing:", normalize("data/missing.csv"))
print("rows:", normalize("data/sample.csv"))
print("counts:", word_counts(normalize("data/sample.csv")))
print("short rows:", word_counts([["a","b"], ["a","b",""]]))
PY
missing: []
rows: [['alice', '3', 'apple'], ['bob', '5', 'pear'], ['alice', '2', 'apple'], ['carol', '', 'plum']]
counts: {'apple': 2, 'pear': 1, 'plum': 1}
short rows: {}
```

The final line confirms both short rows (no third field) and empty third fields
are ignored by `word_counts`.

## Result

`python3 tests/run_tests.py` prints `long-horizon ok` and exits `0`.

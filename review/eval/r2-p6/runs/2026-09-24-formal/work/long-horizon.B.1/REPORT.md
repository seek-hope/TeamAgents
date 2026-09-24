# REPORT: `tools/` package

## Files written

- `tools/__init__.py` — re-exports `normalize` and `word_counts` (`__all__`).
- `tools/normalize.py` — `normalize(path)`.
- `tools/stat.py` — `word_counts(rows)`.
- `REPORT.md` — this file.

No file under `tests/` was created or modified.

## Implemented semantics

`normalize(path)` reads a header-less, comma-separated UTF-8 text file and
returns `list[list[str]]`.

- **Field whitespace**: every field has leading/trailing whitespace removed
  (`"  alice "` -> `"alice"`). Quoting is handled with the stdlib `csv`
  reader, then each parsed field is `.strip()`-ed.
- **Blank lines**: a line that is empty or contains only whitespace is skipped
  and does not contribute a row.
- **Whole-line comments**: a line whose first non-whitespace character is `#`
  is skipped. Only *whole-line* comments are recognized; a `#` in the middle
  of a data line is treated as ordinary data.
- **Row order**: preserved (rows are appended in file order).
- **Missing file**: `normalize("data/missing.csv")` returns `[]` (checked with
  `os.path.isfile` before opening).
- **Encoding**: UTF-8, with `newline=""` so the `csv` module controls line
  splitting.

`word_counts(rows)` returns a `{third_field: count}` dict.

- Uses the third field (`row[2]`) as the key.
- Fields that are empty after normalization are ignored.
- Rows with fewer than three fields (missing third field) are also ignored, so
  short rows never raise `IndexError`.

## Commands actually run and their results

Working directory: the repository root (containing `data/` and `tests/`).

1. Acceptance test:

   ```
   $ python3 tests/run_tests.py
   long-horizon ok
   EXIT=0
   ```

2. Direct demonstration of the API on the provided data:

   ```
   $ python3 -c "from tools import normalize, word_counts; import pprint; \
       pprint.pprint(normalize('data/sample.csv')); \
       pprint.pprint(word_counts(normalize('data/sample.csv'))); \
       print('missing ->', normalize('data/missing.csv'))"
   [['alice', '3', 'apple'],
    ['bob', '5', 'pear'],
    ['alice', '2', 'apple'],
    ['carol', '', 'plum']]
   {'apple': 2, 'pear': 1, 'plum': 1}
   missing -> []
   ```

3. Edge-case probe on a temporary file (`data/edge.csv`) consisting of
   `a,b`; a whitespace-only line; `  # spaced comment`; `x,y,z`; `only,two`;
   `,q,`:

   ```
   [['a', 'b'], ['x', 'y', 'z'], ['only', 'two'], ['', 'q', '']]
   {'z': 1}
   []
   ```

   This confirms: whitespace-only lines and indented comments are ignored,
   rows with fewer than three fields are ignored by `word_counts`, and an empty
   third field is not counted.

## Notes / boundaries

- Line splitting is per physical line (no multi-line quoted fields spanning
  lines); the provided data does not require them.
- `normalize` accepts any path string, not only paths under `data/`; the
  `data/` location is just where the tests point it.

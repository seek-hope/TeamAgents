#!/usr/bin/env python3
"""The `tau3_silence.py` question, asked of Terminal-Bench trials' collected state (D-431).

    python3 review/benchmark/tb_silence.py JOBS_DIR [JOBS_DIR ...]

`tau3_silence.py` reads the adapter's per-turn `exec` reports, which the Terminal-Bench adapter does not keep;
what it keeps is the product's own state per trial, so this reads that instead: for every settled goal in every
`session.sqlite` under the given job directories it asks whether the run left any **assistant text** and whether
the settlement carried a **summary**. Both are reported because they are different things — a tau3 dialogue can
settle with a rich summary and no text at all, which is exactly the case D-431 reports.

What it found (2026-10-06, every trial still on disk): **0 of 40 settled trials** had no text and none had an
empty summary, against 10.5 % of tau3-bench's retail dialogues that settled with no text. Those 40 trials are six
tasks repeated, not a random Terminal-Bench sample, so this is an observation about the *shape* rather than a
rate for the benchmark: silence comes from a conversation, where the user says "go ahead" and nobody asks for a
sentence, while a one-shot instruction ends with a report because the task asks for one.

Exit code 1 when a directory holds no settled trial at all: an unreadable or empty run is not a clean one.
"""
import json
import pathlib
import sqlite3
import sys


def probe(root: pathlib.Path) -> dict:
    settled = 0
    no_text = 0
    no_summary = 0
    rows = []
    for db in sorted(root.rglob("session.sqlite")):
        try:
            connection = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
            events = list(connection.execute("SELECT payload_json FROM events WHERE kind = 'goal_completed'"))
            if not events:
                continue
            settled += 1
            summary = ((json.loads(events[-1][0]).get("completion") or {}).get("summary") or "").strip()
            texts = []
            for (content,) in connection.execute(
                    "SELECT content_json FROM context_entries WHERE kind = 'assistant' ORDER BY rowid"):
                try:
                    texts.append((json.loads(content).get("content") or "").strip())
                except (TypeError, ValueError):
                    pass
            text = any(texts)
            no_summary += not summary
            no_text += not text
            if not text or not summary:
                rows.append((str(db.parent.parent.parent.name), bool(text), bool(summary)))
            connection.close()
        except sqlite3.Error:
            continue
    return {"settled": settled, "no_text": no_text, "no_summary": no_summary, "rows": rows}


def main(argv: list[str]) -> int:
    roots = [pathlib.Path(argument) for argument in argv[1:]]
    if not roots:
        print(__doc__)
        return 2
    status = 0
    for root in roots:
        if not root.is_dir():
            print(f"{root}: not a directory")
            status = 1
            continue
        found = probe(root)
        print(f"{root}")
        print(f"  settled trials: {found['settled']}  with no assistant text: {found['no_text']}  "
              f"with an empty summary: {found['no_summary']}")
        for name, text, summary in found["rows"]:
            print(f"    {name}  assistant_text={text} summary={summary}")
        if not found["settled"]:
            print(f"{root}: no settled trial found — an unreadable run is not a clean one")
            status = 1
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv))

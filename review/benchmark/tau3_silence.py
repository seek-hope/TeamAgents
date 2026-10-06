#!/usr/bin/env python3
"""How often a tau3-bench dialogue ends with the model having written nothing (the detector behind D-431).

    python3 review/benchmark/tau3_silence.py RUN_DIR [RUN_DIR ...]

A model can settle a goal by calling the runtime's `finish` tool and writing no prose at all. The product used
to report that as a completed run with an empty reply, so a user read nothing at all — measured on the full
tau3-bench retail split (2026-10-06): **12 of 114 dialogues** ended that way, and 24 of 487 turns had no text.
Only two of those dialogues also failed the DB check, so the reward hid a user-visible defect that was ten
times more common than its score effect.

The evidence is the adapter's own per-turn reports: `turn-N.json` holds the `exec` report of turn N, whose
`reply` is what the user read and whose `reply_source` says where those words came from (`model` for the
model's own text, `summary` for D-431's fallback to the completion summary, absent for nothing at all). A run
directory made before D-431 simply has no `reply_source` field; a dialogue whose final turn has neither a reply
nor a summary is the defect this script counts, and it is counted per dialogue and per turn.

Exit code 1 means the run could not be read at all (a wrong directory, or reports that are not there) — an
unreadable run is not a clean one.
"""
import json
import pathlib
import sys

DECODER = json.JSONDecoder()


def exec_report(path: pathlib.Path) -> dict | None:
    """The `exec` report inside a `turn-N.json`, which also holds a turn's streamed log."""
    raw = path.read_text()
    found = None
    for index, character in enumerate(raw):
        if character != "{":
            continue
        try:
            value, _ = DECODER.raw_decode(raw, index)
        except ValueError:
            continue
        if isinstance(value, dict) and "end" in value and "goal_status" in value:
            found = value
    return found


def silence(run: pathlib.Path) -> dict:
    dialogues = 0
    turns = 0
    empty_turns = 0
    silent_dialogues = []
    for directory in sorted((p for p in run.iterdir() if p.is_dir()), key=lambda p: (not p.name.isdigit(), p.name)):
        reports = [
            report
            for report in (exec_report(f) for f in sorted(directory.glob("turn-*.json"), key=lambda p: int(p.stem.split("-")[1])))
            if report is not None
        ]
        if not reports:
            continue
        dialogues += 1
        turns += len(reports)
        empty = [report for report in reports if not (report.get("reply") or "").strip()]
        empty_turns += len(empty)
        if not (reports[-1].get("reply") or "").strip():
            reward = json.loads((directory / "reward.json").read_text()) if (directory / "reward.json").exists() else {}
            silent_dialogues.append((directory.name, reward.get("reward"), len(reports)))
    return {
        "dialogues": dialogues,
        "turns": turns,
        "empty_turns": empty_turns,
        "silent_dialogues": silent_dialogues,
    }


def main(argv: list[str]) -> int:
    runs = [pathlib.Path(argument) for argument in argv[1:]]
    if not runs:
        print(__doc__)
        return 2
    for run in runs:
        if not run.is_dir():
            print(f"{run}: not a directory")
            return 1
        summary = silence(run)
        if not summary["dialogues"]:
            print(f"{run}: no turn reports found — an unreadable run is not a clean one")
            return 1
        share = 100.0 * len(summary["silent_dialogues"]) / summary["dialogues"]
        print(f"{run}")
        print(f"  dialogues: {summary['dialogues']}  turns: {summary['turns']}  turns with no text: {summary['empty_turns']}")
        print(f"  dialogues whose final turn has no reply: {len(summary['silent_dialogues'])} ({share:.1f} %)")
        for task, reward, turns in summary["silent_dialogues"]:
            print(f"    task {task:>4}  reward={reward}  turns={turns}")
        if (run / "summary.json").exists():
            data = json.loads((run / "summary.json").read_text())
            print(f"  score: {data['passed']}/{data['tasks']} = {100.0 * data['passed'] / data['tasks']:.1f} %")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

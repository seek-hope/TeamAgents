#!/usr/bin/env python3
"""D-414: where did a trial's window go — model latency, tool calls, or neither?

Reads the state each attempt committed (`session.sqlite` under the collected `artifacts/`) and reports, per
attempt, the wall clock between the first and the last event, the time inside **model requests**
(`attempts.elapsed_ms`), the largest gaps between consecutive events and what brackets each one. That is what
decides which lever can help a trial that runs out of wall clock:

* a window spent in **model requests** cannot be fixed by bounding a tool call — the lever there is fewer
  round-trips or a stall detector that abandons a request producing nothing;
* a window spent inside **one tool call** is what a command wall-clock bound would cut;
* a window spent in neither (long gaps between the driver's own events) would be the driver's own scheduling.

Honesty rule (D-395/D-410): an attempt that collected no state is reported as **no receipt**, never as a zero.
A directory of attempts with no receipts exits 1, so "the tool found no gap" cannot be confused with "there was
no state to look at".

Usage: `python3 review/benchmark/cadence.py /tmp/ta-harbor/<jobs dir> [...]`
"""

from __future__ import annotations

import argparse
import glob
import json
import pathlib
import sqlite3
import sys
from collections import Counter

# A gap shorter than this is ordinary protocol traffic, not a question worth asking about.
GAP_MIN_SECONDS = 20.0

# D-416: a silence inside a stream this long is what a turn-eating stall looks like (the runtime's own stall rule
# only fires at `timeout.clamp(120s, 900s)`, so a silent stretch below that is invisible to it).
STREAM_GAP_MIN_MS = 20_000


def attempts_with_state(jobs: str):
    """[(attempt dir, state db or None)] for every trial under `jobs`."""
    out = []
    for result in sorted(glob.glob(f"{jobs}/*/*/result.json")):
        attempt = pathlib.Path(result).parent
        databases = list(attempt.glob("artifacts/**/session.sqlite"))
        out.append((attempt, databases[0] if databases else None))
    return out


def profile(attempt: pathlib.Path, db: pathlib.Path) -> dict:
    connection = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    events = connection.execute("SELECT sequence, kind, payload_json, created FROM events ORDER BY sequence").fetchall()
    model = connection.execute("SELECT created, elapsed_ms, status FROM attempts ORDER BY created").fetchall()
    connection.close()
    if not events:
        return {"attempt": attempt.name, "empty": True, "events": 0}
    wall = events[-1][3] - events[0][3]
    model_seconds = sum((row[1] or 0) for row in model) / 1000.0

    # Exact tool-call durations: `operation_dispatched` names its operation id, `operation_completed` closes it.
    # This is why the decomposition does not have to guess from the bracketing events.
    dispatches: dict[str, float] = {}
    tool_calls: list[tuple[str, float]] = []
    for _, kind, payload, at in events:
        if kind == "operation_dispatched":
            operation_id = json.loads(payload or "{}").get("operation_id")
            if operation_id:
                dispatches.setdefault(operation_id, at)
    # (the payload names the operation; the scope column carries the same id, which is what pairs them)
    connection = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    for scope, at in connection.execute(
        "SELECT scope, created FROM events WHERE kind = 'operation_dispatched' ORDER BY sequence"
    ):
        if scope in dispatches:
            continue
        dispatches[scope] = at
    for scope, at in connection.execute(
        "SELECT scope, created FROM events WHERE kind = 'operation_completed' ORDER BY sequence"
    ):
        if scope in dispatches:
            tool_calls.append((scope, at - dispatches.pop(scope)))
    connection.close()
    tool_seconds = sum(seconds for _, seconds in tool_calls)
    largest_tool = max(tool_calls, key=lambda pair: pair[1], default=("", 0.0))

    # Every gap is labelled from intervals rather than from a guess about the bracketing events. Tool spans are
    # exact (`operation_dispatched` → `operation_completed` for one operation id); model spans are the
    # `request_began` → the following `attempt_recorded`, which is order-based because `request_began` carries the
    # *instance* as its scope, not the request — with two drivers interleaving, such a span is a union of what ran,
    # which the table's own `max req` column (from `attempts.elapsed_ms`) keeps exact.
    model_spans = []
    pending_start = None
    for _, kind, _, at in events:
        if kind == "request_began":
            pending_start = at
        elif kind in ("attempt_recorded", "attempt_failed") and pending_start is not None:
            model_spans.append((pending_start, at))
            pending_start = None
    tool_spans = []
    connection = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    starts = dict(connection.execute("SELECT scope, created FROM events WHERE kind = 'operation_dispatched'"))
    for scope, at in connection.execute("SELECT scope, created FROM events WHERE kind = 'operation_completed'"):
        if scope in starts:
            tool_spans.append((starts[scope], at))
    connection.close()
    # D-416: how each stream behaved while it ran. A request's `elapsed_ms` alone cannot tell a steady stream from
    # one that sat silent; the adapter's frame timing can (a frame is what also resets the stall rule).
    streams = []
    for _, kind, payload, _ in events:
        if kind != "attempt_recorded":
            continue
        stream = json.loads(payload or "{}").get("stream")
        if isinstance(stream, dict) and stream.get("frames") is not None:
            streams.append(
                {
                    "frames": stream.get("frames") or 0,
                    "first_frame_ms": stream.get("first_frame_ms"),
                    "max_gap_ms": stream.get("max_gap_ms") or 0,
                }
            )

    gaps = []
    for (_, kind, _, at), (_, next_kind, _, next_at) in zip(events, events[1:]):
        if next_at - at < GAP_MIN_SECONDS:
            continue
        covering_model = [span for span in model_spans if span[0] <= at + 1 and span[1] >= next_at - 1]
        covering_tool = [span for span in tool_spans if span[0] <= at + 1 and span[1] >= next_at - 1]
        if covering_tool:
            what = f"one tool call ({covering_tool[0][1] - covering_tool[0][0]:.0f}s)"
        elif covering_model:
            what = f"one model request ({covering_model[0][1] - covering_model[0][0]:.0f}s)"
        else:
            what = f"neither ({kind} → {next_kind})"
        gaps.append({"seconds": round(next_at - at, 1), "after": kind, "before": next_kind, "what": what,
                     "at": at - events[0][3]})
    gaps.sort(key=lambda gap: -gap["seconds"])
    kinds = Counter(kind for _, kind, _, _ in events)
    return {
        "attempt": attempt.name,
        "trial": attempt.parent.parent.name,
        "events": len(events),
        "wall": round(wall, 1),
        "model_requests": len(model),
        "model_seconds": round(model_seconds, 1),
        "model_share": round(model_seconds / wall, 3) if wall > 0 else None,
        "largest_model_seconds": round(max(((row[1] or 0) for row in model), default=0) / 1000.0, 1),
        "tool_calls": len(tool_calls),
        "tool_seconds": round(tool_seconds, 1),
        "tool_share": round(tool_seconds / wall, 3) if wall > 0 else None,
        "largest_tool_seconds": round(largest_tool[1], 1),
        "other_share": round(max(wall - model_seconds - tool_seconds, 0.0) / wall, 3) if wall > 0 else None,
        "streams": streams,
        "stream_requests": len(streams),
        "stream_max_gap_seconds": round(max((s["max_gap_ms"] for s in streams), default=0) / 1000.0, 1),
        "stream_max_first_frame_seconds": round(
            max((s["first_frame_ms"] or 0 for s in streams), default=0) / 1000.0, 1
        ),
        "stream_silent_requests": sum(1 for s in streams if s["max_gap_ms"] >= STREAM_GAP_MIN_MS),
        "gaps": gaps[:6],
        "operations": kinds.get("operation_completed", 0),
        "kinds": dict(kinds.most_common(6)),
    }


def label(gap: dict) -> str:
    """What covered the gap — decided from the exact spans when the profile built it."""
    return gap.get("what", "unknown")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("jobs", nargs="+", help="one or more harbor jobs directories")
    parser.add_argument("--json", action="store_true", help="print the raw profile instead of the table")
    args = parser.parse_args()

    rows, receipts = [], 0
    for jobs in args.jobs:
        for attempt, db in attempts_with_state(jobs):
            if db is None:
                rows.append({"attempt": attempt.name, "trial": attempt.parent.parent.name, "no_receipt": True})
                continue
            receipts += 1
            rows.append(profile(attempt, db))

    if args.json:
        print(json.dumps(rows, indent=2))
    else:
        print(
            f"{'attempt':28} {'wall s':>7} {'model s':>8} {'tool s':>7} {'other':>6} {'req':>4} {'ops':>4} "
            f"{'max req':>8} {'max tool':>9} {'frames':>7} {'silent':>7} {'max gap':>8}"
        )
        for row in rows:
            if row.get("no_receipt"):
                print(f"{row['attempt'][:28]:28} {'no receipt':>7}")
                continue
            if row.get("empty"):
                print(f"{row['attempt'][:28]:28} {'empty':>7}")
                continue
            # an attempt recorded before the timing existed has *no receipt* for those columns, which is not the
            # same as "it streamed with no gap" (D-395's rule, one instrument later)
            streamed = row["stream_requests"]
            tail = (
                f"{streamed:>7} {row['stream_silent_requests']:>7} {row['stream_max_gap_seconds']:>7.0f}s"
                if streamed
                else f"{'—':>7} {'—':>7} {'no receipt':>8}"
            )
            print(
                f"{row['attempt'][:28]:28} {row['wall']:>7.0f} {row['model_seconds']:>8.0f} "
                f"{row['tool_seconds']:>7.0f} {row['other_share']:>6.2f} {row['model_requests']:>4} "
                f"{row['operations']:>4} {row['largest_model_seconds']:>8.0f} {row['largest_tool_seconds']:>9.0f} "
                f"{tail}"
            )
            for gap in row["gaps"][:3]:
                print(f"{'':28} at {gap['at']:>6.0f}s: {gap['seconds']:>5.0f}s — {label(gap)}")

    if receipts == 0:
        print(
            "\nNO RECEIPT: none of these attempts collected its state, so this run says nothing about where the "
            "window went — add `artifacts = [\"/opt/teamagents/state\"]` to each task.toml and re-run "
            "(review/benchmark/README.md).",
            file=sys.stderr,
        )
        return 1
    print(f"\n{receipts} attempt(s) with a state receipt, {len(rows) - receipts} without")
    return 0


if __name__ == "__main__":
    sys.exit(main())

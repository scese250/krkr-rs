#!/usr/bin/env python3
"""Turn a run's slow-frame records into the statistics of the slow frames.

The engine reports every frame whose gap reaches the threshold the caller
set (``--slow-frame-ms``), as one record: a header that names the frame, the
gap and the cost that dominates it, and the continuation lines that carry
the frame's own costs (``docs/rules/logging.md`` sections 1 and 4).  One
report answers one question -- *where did the time of the slow frames go* --
and this script answers it over a whole run:

* how many frames were reported, and how many of them reach the threshold
  this script is given (20 ms by default; the budget of a 30 fps target is
  33.3 ms, so a reader who measures a frame budget sets its own);
* the distribution of the slow frames' gaps: the mean, the p50, the p90, the
  p99 and the worst, and the share of the reported time the slow frames take;
* the aggregate: every cost line summed over the slow frames and ranked, so
  the reader sees where the time went (the engine's phases, the host's work,
  the pool, the residuals);
* the dominant-cost histogram: which cost dominated how many of the frames;
* the worst frames, each with its own breakdown;
* the frame numbers of the slow frames, so a reader can drive the game to
  one of them.

The record is parsed by its ``key=value`` fields, never by the position of a
column: a record that grows a field, or a log a tool rewrote, is still read.
A record that is broken is counted and left out, never guessed at, and a
continuation line that does not follow its record is skipped and counted.

The output is plain text, one fact per line, and every line is prefixed
with ``slow-frame-stats `` so it can be grepped and parsed.  It names the
input files and the threshold it used, and it carries the frame numbers and
the cost names only: nothing of the game's own content.

Usage:
    slow_frame_stats.py [--threshold-ms N] [--worst N] <log> [<log> ...]
    slow_frame_stats.py --self-test
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

# The marker the header of a slow-frame record carries.
HEADER_MARKER = "slow frame="

# The keywords a continuation line of a record may start with.
COST = "cost"
RESIDUAL = "residual"
TOTAL = "total"
FRONT_END = "front_end"
CONTINUATION_KEYWORDS = (COST, RESIDUAL, TOTAL, FRONT_END)

# The count a residual line carries beside its millisecond fields.
JOBS_FIELD = "pool_jobs"

# The gap at which a reported frame counts as slow, in milliseconds.
DEFAULT_THRESHOLD_MS = 20.0

# The number of worst frames the report breaks down.
DEFAULT_WORST = 3

# The prefix every line of the report carries.
PREFIX = "slow-frame-stats "

# The fixture corpus the self-test reads.
FIXTURES = Path(__file__).resolve().parent / "fixtures" / "slow_frame_stats"


# --------------------------------------------------------------------------
# The record
# --------------------------------------------------------------------------


@dataclass
class SlowFrame:
    """One slow-frame record: its header and the lines that follow it.

    The header names the frame, whether the frame reached the surface, the
    gap it spans, the cost that dominates it and that cost's own time.  The
    continuation lines carry the frame's costs, its residuals and its
    totals.
    """

    frame: int | None
    presented: bool | None
    gap_ms: float | None
    dominant: str
    dominant_ms: float | None
    costs: list[tuple[str, float]] = field(default_factory=list)
    residuals: list[tuple[str, float]] = field(default_factory=list)
    totals: list[tuple[str, float]] = field(default_factory=list)
    front_end: list[tuple[str, float]] = field(default_factory=list)
    pool_jobs: int = 0


@dataclass
class Counts:
    """What the parse found besides the records themselves."""

    headers: int = 0
    unusable: int = 0
    orphans: int = 0
    skipped: int = 0


def _number(text: str | None) -> float | None:
    """The number a field holds, `None` when it holds none."""
    if text is None:
        return None
    try:
        return float(text)
    except ValueError:
        return None


def _integer(text: str | None) -> int | None:
    """The whole number a field holds, `None` when it holds none."""
    if text is None:
        return None
    try:
        return int(text)
    except ValueError:
        return None


def _fields(text: str) -> dict[str, str]:
    """The ``key=value`` fields a header carries."""
    fields: dict[str, str] = {}
    for token in text.split():
        key, separator, value = token.partition("=")
        if separator:
            fields[key] = value
    return fields


def parse_header(line: str) -> SlowFrame | None:
    """The record a header line starts, `None` when the line is not one.

    The header is found by the marker it carries, so the logger's own prefix
    (the timestamp, the level and the module) does not matter.
    """
    at = line.find(HEADER_MARKER)
    if at < 0:
        return None
    rest = line[at + len(HEADER_MARKER) :]
    tokens = rest.split()
    if not tokens:
        return None
    frame: int | None = None
    if tokens[0].isdigit():
        frame = int(tokens[0])
        tokens = tokens[1:]
    fields = _fields(" ".join(tokens))
    presented = fields.get("presented")
    return SlowFrame(
        frame=frame,
        presented=None if presented is None else presented == "true",
        gap_ms=_number(fields.get("gap_ms")),
        dominant=fields.get("dominant", ""),
        dominant_ms=_number(fields.get("dominant_ms")),
    )


def _continuation_keyword(text: str) -> str | None:
    """The keyword a continuation line starts with, `None` when it starts with none."""
    stripped = text.strip()
    for keyword in CONTINUATION_KEYWORDS:
        if not stripped.startswith(keyword):
            continue
        tail = stripped[len(keyword) :]
        if tail[:1].isspace():
            return keyword
    return None


def _pairs(text: str) -> tuple[list[tuple[str, float]], int, int]:
    """The millisecond fields, the job count and the skipped tokens of a line.

    A field whose value is not a number is skipped: the line is counted, so
    a broken field is never mistaken for a measurement.
    """
    pairs: list[tuple[str, float]] = []
    jobs = 0
    skipped = 0
    for token in text.split():
        key, separator, value = token.partition("=")
        if not separator:
            skipped += 1
            continue
        if key == JOBS_FIELD:
            count = _integer(value)
            if count is None:
                skipped += 1
            else:
                jobs = count
            continue
        number = _number(value)
        if number is None:
            skipped += 1
            continue
        pairs.append((key, number))
    return pairs, jobs, skipped


def parse_log(text: str) -> tuple[list[SlowFrame], Counts]:
    """The records a log holds, and what the parse found besides them.

    A continuation line belongs to the record it follows directly: a line
    that arrives after another record's line does not belong to the open
    record and is counted as an orphan instead of being added to it.  A
    record whose gap is unreadable is kept and counted as unusable, so the
    reader sees that a record was left out.
    """
    records: list[SlowFrame] = []
    counts = Counts()
    current: SlowFrame | None = None
    attached = False

    for raw in text.splitlines():
        line = raw.rstrip()
        header = parse_header(line)
        if header is not None:
            current = header
            attached = True
            records.append(current)
            counts.headers += 1
            if header.gap_ms is None:
                counts.unusable += 1
            continue
        keyword = _continuation_keyword(line)
        if keyword is None:
            # An indented line that names no keyword this script knows is a
            # line of a record shape it does not read: it is counted, and
            # the open record is closed for the lines that follow.
            if line[:1].isspace() and line.strip():
                counts.skipped += 1
            if line.strip():
                attached = False
            continue
        if current is None or not attached:
            counts.orphans += 1
            attached = False
            continue
        pairs, jobs, skipped = _pairs(line.strip()[len(keyword) :])
        counts.skipped += skipped
        if jobs:
            current.pool_jobs = jobs
        if keyword == COST:
            current.costs.extend(pairs)
        elif keyword == RESIDUAL:
            current.residuals.extend(pairs)
        elif keyword == TOTAL:
            current.totals.extend(pairs)
        else:
            current.front_end.extend(pairs)
    return records, counts


# --------------------------------------------------------------------------
# The statistics
# --------------------------------------------------------------------------


@dataclass
class Stats:
    """The statistics of the slow frames of a run."""

    reported_frames: int = 0
    slow_frames: int = 0
    unusable_frames: int = 0
    mean_ms: float = 0.0
    p50_ms: float = 0.0
    p90_ms: float = 0.0
    p99_ms: float = 0.0
    worst_ms: float = 0.0
    slow_total_ms: float = 0.0
    reported_total_ms: float = 0.0
    slow_time_share_pct: float = 0.0
    aggregate_cost: list[tuple[str, float]] = field(default_factory=list)
    aggregate_residual: list[tuple[str, float]] = field(default_factory=list)
    pool_jobs: int = 0
    dominant: list[tuple[str, int]] = field(default_factory=list)
    worst: list[SlowFrame] = field(default_factory=list)
    frames: list[int] = field(default_factory=list)


def percentile(values: list[float], percent: float) -> float:
    """The nearest-rank percentile of a sorted list.

    The percentile is the value at the rank ``ceil(percent / 100 * n)`` of
    the sorted values, so every percentile is a value the run measured.
    """
    if not values:
        return 0.0
    rank = math.ceil(percent / 100.0 * len(values))
    index = min(max(rank, 1), len(values)) - 1
    return values[index]


def _sum_pairs(
    records: list[SlowFrame], pairs_of: Callable[[SlowFrame], list[tuple[str, float]]]
) -> list[tuple[str, float]]:
    """The sum of every key over the records, ranked by the sum."""
    totals: dict[str, float] = {}
    for record in records:
        for key, value in pairs_of(record):
            totals[key] = totals.get(key, 0.0) + value
    return sorted(totals.items(), key=lambda item: (-item[1], item[0]))


def summarize(
    records: list[SlowFrame], counts: Counts, threshold_ms: float, worst_count: int
) -> Stats:
    """The statistics of the records that reach the threshold.

    A frame counts as slow when its gap reaches the threshold, the rule the
    engine uses to report it.  A record whose gap is unreadable is counted
    and left out of every measurement.
    """
    usable = [record for record in records if record.gap_ms is not None]
    slow = [record for record in usable if record.gap_ms >= threshold_ms]
    gaps = sorted(record.gap_ms for record in slow if record.gap_ms is not None)
    reported_total = sum(record.gap_ms for record in usable if record.gap_ms is not None)
    slow_total = sum(gaps)

    dominant: dict[str, int] = {}
    for record in slow:
        if not record.dominant:
            continue
        dominant[record.dominant] = dominant.get(record.dominant, 0) + 1

    worst = sorted(
        slow, key=lambda record: (-(record.gap_ms or 0.0), record.frame or 0)
    )[:worst_count]

    return Stats(
        reported_frames=counts.headers,
        slow_frames=len(slow),
        unusable_frames=counts.unusable,
        mean_ms=(slow_total / len(gaps)) if gaps else 0.0,
        p50_ms=percentile(gaps, 50.0),
        p90_ms=percentile(gaps, 90.0),
        p99_ms=percentile(gaps, 99.0),
        worst_ms=gaps[-1] if gaps else 0.0,
        slow_total_ms=slow_total,
        reported_total_ms=reported_total,
        slow_time_share_pct=(slow_total / reported_total * 100.0)
        if reported_total
        else 0.0,
        aggregate_cost=_sum_pairs(slow, lambda record: record.costs),
        aggregate_residual=_sum_pairs(slow, lambda record: record.residuals),
        pool_jobs=sum(record.pool_jobs for record in slow),
        dominant=sorted(dominant.items(), key=lambda item: (-item[1], item[0])),
        worst=worst,
        frames=[record.frame for record in slow if record.frame is not None],
    )


# --------------------------------------------------------------------------
# The report
# --------------------------------------------------------------------------


def _ms(value: float) -> str:
    """A millisecond value as the report writes it."""
    return f"{value:.2f}"


def report(stats: Stats, sources: list[str], threshold_ms: float, counts: Counts) -> list[str]:
    """The report of the slow frames, one fact per line."""
    lines: list[str] = []
    for source in sources:
        lines.append(f"{PREFIX}source file={source}")
    lines.append(f"{PREFIX}threshold_ms={_ms(threshold_ms)}")
    lines.append(f"{PREFIX}reported_frames={stats.reported_frames}")
    lines.append(f"{PREFIX}slow_frames={stats.slow_frames}")
    lines.append(f"{PREFIX}unusable_frames={stats.unusable_frames}")
    lines.append(f"{PREFIX}orphan_lines={counts.orphans}")
    lines.append(f"{PREFIX}skipped_lines={counts.skipped}")
    lines.append(f"{PREFIX}gap_mean_ms={_ms(stats.mean_ms)}")
    lines.append(f"{PREFIX}gap_p50_ms={_ms(stats.p50_ms)}")
    lines.append(f"{PREFIX}gap_p90_ms={_ms(stats.p90_ms)}")
    lines.append(f"{PREFIX}gap_p99_ms={_ms(stats.p99_ms)}")
    lines.append(f"{PREFIX}gap_worst_ms={_ms(stats.worst_ms)}")
    lines.append(f"{PREFIX}slow_total_ms={_ms(stats.slow_total_ms)}")
    lines.append(f"{PREFIX}reported_total_ms={_ms(stats.reported_total_ms)}")
    lines.append(f"{PREFIX}slow_time_share_pct={_ms(stats.slow_time_share_pct)}")
    for key, total in stats.aggregate_cost:
        lines.append(f"{PREFIX}aggregate_cost key={key} total_ms={_ms(total)}")
    for key, total in stats.aggregate_residual:
        lines.append(f"{PREFIX}aggregate_residual key={key} total_ms={_ms(total)}")
    lines.append(f"{PREFIX}aggregate_pool_jobs={stats.pool_jobs}")
    for key, count in stats.dominant:
        lines.append(f"{PREFIX}dominant key={key} frames={count}")
    for rank, record in enumerate(stats.worst, start=1):
        lines.append(
            f"{PREFIX}worst rank={rank} frame={_frame_of(record)} "
            f"gap_ms={_ms(record.gap_ms or 0.0)} dominant={record.dominant} "
            f"dominant_ms={_ms(record.dominant_ms or 0.0)} "
            f"presented={_presented_of(record)}"
        )
        for key, value in record.costs:
            lines.append(
                f"{PREFIX}worst_cost rank={rank} frame={_frame_of(record)} "
                f"key={key} ms={_ms(value)}"
            )
        for key, value in record.residuals:
            lines.append(
                f"{PREFIX}worst_residual rank={rank} frame={_frame_of(record)} "
                f"key={key} ms={_ms(value)}"
            )
        for key, value in record.totals:
            lines.append(
                f"{PREFIX}worst_total rank={rank} frame={_frame_of(record)} "
                f"key={key} ms={_ms(value)}"
            )
        for key, value in record.front_end:
            lines.append(
                f"{PREFIX}worst_front_end rank={rank} frame={_frame_of(record)} "
                f"key={key} ms={_ms(value)}"
            )
    for frame in stats.frames:
        lines.append(f"{PREFIX}frame={frame}")
    return lines


def _frame_of(record: SlowFrame) -> str:
    """The frame a record names."""
    return "?" if record.frame is None else str(record.frame)


def _presented_of(record: SlowFrame) -> str:
    """Whether a record's frame reached the surface."""
    if record.presented is None:
        return "?"
    return "true" if record.presented else "false"


# --------------------------------------------------------------------------
# The self-test
# --------------------------------------------------------------------------


def probe(stats: Stats, counts: Counts, path: str) -> float:
    """The number a probe names in the statistics.

    A probe is a field of the statistics, or a key of one of its lists:
    ``aggregate_cost:phases.frame_tail`` is the summed time of that cost.
    """
    key, _, name = path.partition(":")
    if name:
        if key == "aggregate_cost":
            return _lookup(stats.aggregate_cost, name)
        if key == "aggregate_residual":
            return _lookup(stats.aggregate_residual, name)
        if key == "dominant":
            for entry, count in stats.dominant:
                if entry == name:
                    return float(count)
            return 0.0
        raise ValueError(f"unknown probe list {key!r}")
    values = {
        "reported_frames": float(stats.reported_frames),
        "slow_frames": float(stats.slow_frames),
        "unusable_frames": float(stats.unusable_frames),
        "pool_jobs": float(stats.pool_jobs),
        "orphan_lines": float(counts.orphans),
        "skipped_lines": float(counts.skipped),
        "mean_ms": stats.mean_ms,
        "p50_ms": stats.p50_ms,
        "p90_ms": stats.p90_ms,
        "p99_ms": stats.p99_ms,
        "worst_ms": stats.worst_ms,
        "slow_total_ms": stats.slow_total_ms,
        "reported_total_ms": stats.reported_total_ms,
        "slow_time_share_pct": stats.slow_time_share_pct,
    }
    if key not in values:
        raise ValueError(f"unknown probe {path!r}")
    return values[key]


def _lookup(pairs: list[tuple[str, float]], key: str) -> float:
    """The value a key holds in a ranked list."""
    for entry, value in pairs:
        if entry == key:
            return value
    return 0.0


def _first_worst(stats: Stats) -> SlowFrame:
    """The worst frame of the statistics, an empty record when there is none."""
    if stats.worst:
        return stats.worst[0]
    return SlowFrame(frame=None, presented=None, gap_ms=None, dominant="", dominant_ms=None)


def _close(left: float, right: float) -> bool:
    """Whether two numbers are the same to the report's precision."""
    return abs(left - right) <= 0.01


def _compare(expected, actual, path: str, failures: list[str]) -> None:
    """Check one expected value against the computed one.

    A number is compared to the report's precision (a hundredth of a
    millisecond), a name to itself.
    """
    if isinstance(expected, (list, tuple)):
        if not isinstance(actual, (list, tuple)) or len(actual) != len(expected):
            failures.append(
                f"{path}: expected {len(expected)} entries, got "
                f"{len(actual) if isinstance(actual, (list, tuple)) else actual!r}"
            )
            return
        for index, (want, have) in enumerate(zip(expected, actual)):
            _compare(want, have, f"{path}[{index}]", failures)
        return
    if isinstance(expected, (int, float)) and isinstance(actual, (int, float)):
        if not _close(float(expected), float(actual)):
            failures.append(f"{path}: expected {expected}, got {actual}")
        return
    if expected != actual:
        failures.append(f"{path}: expected {expected!r}, got {actual!r}")


def self_test(fixtures: Path = FIXTURES) -> int:
    """Prove the parser and the statistics against the fixture.

    The corpus is read four ways: a run whose numbers are all known, a run
    whose records are broken, a run with no slow frame at all, and a run
    whose gaps are all different (so every percentile the report writes is a
    different value).  The corpus is then read a fifth way, with one
    mutation applied at a time: the number the mutation touches has to move,
    or the mutation was not seen.  A statistic that does not follow the
    fixture is a statistic that is not measured.
    """
    log_path = fixtures / "slow_frames.log"
    expected_path = fixtures / "expected.json"
    malformed_path = fixtures / "malformed.log"
    mutations_path = fixtures / "mutations.json"
    empty_path = fixtures / "empty.log"
    percentiles_path = fixtures / "percentiles.log"
    percentiles_expected_path = fixtures / "percentiles.json"
    for path in (
        log_path,
        expected_path,
        malformed_path,
        mutations_path,
        empty_path,
        percentiles_path,
        percentiles_expected_path,
    ):
        if not path.is_file():
            print(f"slow-frame-stats: self-test: the fixture is missing: {path}")
            return 1

    failures: list[str] = []
    text = log_path.read_text(encoding="utf-8")
    records, counts = parse_log(text)
    expected = json.loads(expected_path.read_text(encoding="utf-8"))
    threshold = float(expected.get("threshold_ms", DEFAULT_THRESHOLD_MS))
    stats = summarize(records, counts, threshold, DEFAULT_WORST)

    _compare(expected["reported_frames"], stats.reported_frames, "reported_frames", failures)
    _compare(expected["slow_frames"], stats.slow_frames, "slow_frames", failures)
    _compare(expected["unusable_frames"], stats.unusable_frames, "unusable_frames", failures)
    _compare(expected["mean_ms"], stats.mean_ms, "mean_ms", failures)
    _compare(expected["p50_ms"], stats.p50_ms, "p50_ms", failures)
    _compare(expected["p90_ms"], stats.p90_ms, "p90_ms", failures)
    _compare(expected["p99_ms"], stats.p99_ms, "p99_ms", failures)
    _compare(expected["worst_ms"], stats.worst_ms, "worst_ms", failures)
    _compare(expected["slow_total_ms"], stats.slow_total_ms, "slow_total_ms", failures)
    _compare(
        expected["reported_total_ms"], stats.reported_total_ms, "reported_total_ms", failures
    )
    _compare(
        expected["slow_time_share_pct"],
        stats.slow_time_share_pct,
        "slow_time_share_pct",
        failures,
    )
    _compare(expected["pool_jobs"], stats.pool_jobs, "pool_jobs", failures)
    _compare(expected["orphan_lines"], counts.orphans, "orphan_lines", failures)
    _compare(expected["skipped_lines"], counts.skipped, "skipped_lines", failures)
    _compare(expected["aggregate_cost"], stats.aggregate_cost, "aggregate_cost", failures)
    _compare(
        expected["aggregate_residual"], stats.aggregate_residual, "aggregate_residual", failures
    )
    _compare(expected["dominant"], stats.dominant, "dominant", failures)
    _compare(expected["frames"], stats.frames, "frames", failures)
    _compare(
        expected["worst_frames"],
        [_frame_of(record) for record in stats.worst],
        "worst_frames",
        failures,
    )
    _compare(
        expected["worst_presented"],
        [_presented_of(record) for record in stats.worst],
        "worst_presented",
        failures,
    )
    _compare(
        expected["worst_front_end"],
        [list(pair) for pair in _first_worst(stats).front_end],
        "worst_front_end",
        failures,
    )
    _compare(
        expected["worst_totals"],
        [list(pair) for pair in _first_worst(stats).totals],
        "worst_totals",
        failures,
    )

    # A broken log must be read, not crashed on: an orphan continuation
    # line, a header with no continuation, a field that holds no number.
    malformed_records, malformed_counts = parse_log(
        malformed_path.read_text(encoding="utf-8")
    )
    malformed = summarize(malformed_records, malformed_counts, threshold, DEFAULT_WORST)
    broken = json.loads((fixtures / "malformed.json").read_text(encoding="utf-8"))
    _compare(broken["reported_frames"], malformed.reported_frames, "malformed.reported_frames", failures)
    _compare(broken["unusable_frames"], malformed.unusable_frames, "malformed.unusable_frames", failures)
    _compare(broken["orphan_lines"], malformed_counts.orphans, "malformed.orphan_lines", failures)
    _compare(broken["skipped_lines"], malformed_counts.skipped, "malformed.skipped_lines", failures)
    _compare(broken["slow_frames"], malformed.slow_frames, "malformed.slow_frames", failures)
    _compare(broken["aggregate_cost"], malformed.aggregate_cost, "malformed.aggregate_cost", failures)

    # A run with no slow frame has no statistics to report, and the report
    # is still a report: nothing is divided by nothing.
    empty_records, empty_counts = parse_log(empty_path.read_text(encoding="utf-8"))
    empty = summarize(empty_records, empty_counts, threshold, DEFAULT_WORST)
    _compare(0, empty.reported_frames, "empty.reported_frames", failures)
    _compare(0, empty.slow_frames, "empty.slow_frames", failures)
    _compare(0.0, empty.mean_ms, "empty.mean_ms", failures)
    _compare(0.0, empty.slow_time_share_pct, "empty.slow_time_share_pct", failures)
    if not report(empty, ["empty.log"], threshold, empty_counts):
        failures.append("empty: the report of a run with no slow frame is empty")

    # The percentile rule: the nearest rank of a sorted list.
    sample = [float(value) for value in range(1, 101)]
    for percent, want in ((50.0, 50.0), (95.0, 95.0), (99.0, 99.0)):
        got = percentile(sample, percent)
        if not _close(got, want):
            failures.append(f"percentile p{percent:.0f}: expected {want}, got {got}")

    # The distribution of a run whose gaps are all different: every
    # percentile the report writes is a value the run measured, and no two
    # of them are the same value.
    percentile_records, percentile_counts = parse_log(
        percentiles_path.read_text(encoding="utf-8")
    )
    percentile_expected = json.loads(percentiles_expected_path.read_text(encoding="utf-8"))
    percentile_stats = summarize(
        percentile_records, percentile_counts, threshold, DEFAULT_WORST
    )
    _compare(
        percentile_expected["reported_frames"],
        percentile_stats.reported_frames,
        "percentiles.reported_frames",
        failures,
    )
    _compare(
        percentile_expected["slow_frames"],
        percentile_stats.slow_frames,
        "percentiles.slow_frames",
        failures,
    )
    _compare(
        percentile_expected["mean_ms"], percentile_stats.mean_ms, "percentiles.mean_ms", failures
    )
    _compare(percentile_expected["p50_ms"], percentile_stats.p50_ms, "percentiles.p50_ms", failures)
    _compare(percentile_expected["p90_ms"], percentile_stats.p90_ms, "percentiles.p90_ms", failures)
    _compare(percentile_expected["p99_ms"], percentile_stats.p99_ms, "percentiles.p99_ms", failures)
    _compare(
        percentile_expected["worst_ms"], percentile_stats.worst_ms, "percentiles.worst_ms", failures
    )

    # The mutations: each one has to move the number it touches.
    mutations = json.loads(mutations_path.read_text(encoding="utf-8"))
    if not isinstance(mutations, list) or not mutations:
        print("slow-frame-stats: self-test: mutations.json must be a non-empty list")
        return 1
    for mutation in mutations:
        name = mutation.get("name", "<unnamed>")
        old = mutation.get("old")
        new = mutation.get("new")
        path = mutation.get("probe")
        value = mutation.get("value")
        if not all(isinstance(item, str) for item in (name, old, new, path)):
            print(f"slow-frame-stats: self-test: a malformed mutation: {mutation!r}")
            return 1
        if not isinstance(value, (int, float)):
            print(f"slow-frame-stats: self-test: a mutation without a value: {mutation!r}")
            return 1
        if old not in text:
            print(
                f"slow-frame-stats: self-test: the mutation {name!r} no longer matches "
                "the fixture"
            )
            return 1
        broken_text = text.replace(old, new, 1)
        broken_records, broken_counts = parse_log(broken_text)
        broken_stats = summarize(broken_records, broken_counts, threshold, DEFAULT_WORST)
        try:
            got = probe(broken_stats, broken_counts, path)
            before = probe(stats, counts, path)
        except ValueError as error:
            print(f"slow-frame-stats: self-test: {error}")
            return 1
        if not _close(got, float(value)):
            failures.append(f"mutation {name!r}: {path} is {got}, expected {value}")
            continue
        if _close(got, before):
            failures.append(
                f"mutation {name!r}: {path} did not move (it stayed {got})"
            )

    if failures:
        print("slow-frame-stats: self-test FAILED:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(
        f"slow-frame-stats: self-test ok "
        f"({len(mutations)} mutation(s), {len(records)} record(s) in the fixture)"
    )
    return 0


# --------------------------------------------------------------------------
# The entry point
# --------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "logs",
        nargs="*",
        help="the log files to read; the statistics cover all of them together",
    )
    parser.add_argument(
        "--threshold-ms",
        type=float,
        default=DEFAULT_THRESHOLD_MS,
        help="the gap at which a reported frame counts as slow",
    )
    parser.add_argument(
        "--worst",
        type=int,
        default=DEFAULT_WORST,
        help="the number of worst frames the report breaks down",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="prove the parser and the statistics against the fixture",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    if not args.logs:
        print("slow-frame-stats: a log file is needed", file=sys.stderr)
        return 2
    if args.worst < 0:
        print("slow-frame-stats: --worst must not be negative", file=sys.stderr)
        return 2

    records: list[SlowFrame] = []
    counts = Counts()
    sources: list[str] = []
    for name in args.logs:
        path = Path(name)
        if not path.is_file():
            print(f"slow-frame-stats: {name} is not a file", file=sys.stderr)
            return 1
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            print(f"slow-frame-stats: {name} cannot be read: {error}", file=sys.stderr)
            return 1
        found, found_counts = parse_log(text)
        records.extend(found)
        counts.headers += found_counts.headers
        counts.unusable += found_counts.unusable
        counts.orphans += found_counts.orphans
        counts.skipped += found_counts.skipped
        sources.append(name)

    stats = summarize(records, counts, args.threshold_ms, args.worst)
    for line in report(stats, sources, args.threshold_ms, counts):
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

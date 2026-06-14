#!/usr/bin/env python3
"""
Aggregate KernelX syscall trace logs and report per-syscall latency.

Input lines are expected to look like:
  [    1.234567] [SYSCALL] 64 (write): ENTER args=[0x1, 0x2, 0x3], tid=7
  [    1.234890] [SYSCALL] 64 (write): args=[0x1, 0x2, 0x3] -> Ok(0x3), tid=7
  [    1.234900] [SYSCALL_TIME] 64 write 12

Usage:
  python3 scripts/aggregate_syscall_log.py kernel.log
  make run ... 2>&1 | python3 scripts/aggregate_syscall_log.py
"""

import argparse
import csv
import re
import sys
from collections import defaultdict, deque
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable, TextIO


ANSI_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
TIMESTAMP_RE = re.compile(r"\[\s*(?P<sec>\d+)\.(?P<usec>\d{6})\]")
SYSCALL_RE = re.compile(
    r"\[SYSCALL\]\s+"
    r"(?P<num>\d+)\s+"
    r"\((?P<name>[^)]+)\):\s+"
    r"(?P<body>.*),\s+"
    r"tid=(?P<tid>\d+)\s*$"
)
SYSCALL_TIME_RE = re.compile(
    r"\[SYSCALL_TIME\]\s+"
    r"(?P<num>\d+)\s+"
    r"(?:(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s+)?"
    r"(?P<duration_us>\d+)\s*$"
)


@dataclass
class EnterEvent:
    time_us: int
    line_no: int


@dataclass
class SyscallStats:
    num: int
    name: str
    durations_us: list[int] = field(default_factory=list)
    ok: int = 0
    err: int = 0

    def add(self, duration_us: int, outcome: str) -> None:
        self.durations_us.append(duration_us)
        if outcome.startswith("Err("):
            self.err += 1
        else:
            self.ok += 1

    @property
    def count(self) -> int:
        return len(self.durations_us)

    @property
    def total_us(self) -> int:
        return sum(self.durations_us)

    @property
    def avg_us(self) -> float:
        return self.total_us / self.count if self.count else 0.0

    @property
    def min_us(self) -> int:
        return min(self.durations_us) if self.durations_us else 0

    @property
    def max_us(self) -> int:
        return max(self.durations_us) if self.durations_us else 0

    def percentile_us(self, pct: float) -> int:
        if not self.durations_us:
            return 0
        values = sorted(self.durations_us)
        index = round((len(values) - 1) * pct)
        return values[index]


@dataclass
class ParseSummary:
    lines: int = 0
    syscall_lines: int = 0
    matched: int = 0
    unmatched_results: int = 0
    malformed_syscall_lines: int = 0


def parse_timestamp_us(line: str) -> int | None:
    match = TIMESTAMP_RE.search(line)
    if not match:
        return None
    return int(match.group("sec")) * 1_000_000 + int(match.group("usec"))


def parse_enter_args(body: str) -> str | None:
    prefix = "ENTER args="
    if not body.startswith(prefix):
        return None
    return body[len(prefix) :]


def parse_result(body: str) -> tuple[str, str] | None:
    prefix = "args="
    if not body.startswith(prefix):
        return None
    args_and_outcome = body[len(prefix) :]
    if " -> " not in args_and_outcome:
        return None
    args, outcome = args_and_outcome.split(" -> ", 1)
    return args, outcome


def iter_lines(paths: list[Path]) -> Iterable[tuple[str, int, str]]:
    if not paths:
        for line_no, line in enumerate(sys.stdin, 1):
            yield "<stdin>", line_no, line
        return

    for path in paths:
        if str(path) == "-":
            for line_no, line in enumerate(sys.stdin, 1):
                yield "<stdin>", line_no, line
            continue
        with path.open("r", encoding="utf-8", errors="replace") as f:
            for line_no, line in enumerate(f, 1):
                yield str(path), line_no, line


def aggregate(paths: list[Path]) -> tuple[dict[tuple[int, str], SyscallStats], ParseSummary, int]:
    pending: dict[tuple[int, int, str, str], deque[EnterEvent]] = defaultdict(deque)
    stats: dict[tuple[int, str], SyscallStats] = {}
    summary = ParseSummary()

    for _source, line_no, raw_line in iter_lines(paths):
        summary.lines += 1
        line = ANSI_RE.sub("", raw_line).strip()
        if "[SYSCALL]" not in line and "[SYSCALL_TIME]" not in line:
            continue

        summary.syscall_lines += 1
        syscall_time_match = SYSCALL_TIME_RE.search(line)
        if syscall_time_match is not None:
            num = int(syscall_time_match.group("num"))
            name = syscall_time_match.group("name") or f"syscall_{num}"
            duration_us = int(syscall_time_match.group("duration_us"))
            stats_key = (num, name)
            if stats_key not in stats:
                stats[stats_key] = SyscallStats(num, name)
            stats[stats_key].add(duration_us, "Ok")
            summary.matched += 1
            continue

        time_us = parse_timestamp_us(line)
        syscall_match = SYSCALL_RE.search(line)
        if time_us is None or syscall_match is None:
            summary.malformed_syscall_lines += 1
            continue

        num = int(syscall_match.group("num"))
        name = syscall_match.group("name")
        body = syscall_match.group("body")
        tid = int(syscall_match.group("tid"))

        enter_args = parse_enter_args(body)
        if enter_args is not None:
            pending[(tid, num, name, enter_args)].append(EnterEvent(time_us, line_no))
            continue

        result = parse_result(body)
        if result is None:
            summary.malformed_syscall_lines += 1
            continue

        args, outcome = result
        key = (tid, num, name, args)
        if not pending[key]:
            summary.unmatched_results += 1
            continue

        enter = pending[key].popleft()
        duration_us = max(0, time_us - enter.time_us)
        stats_key = (num, name)
        if stats_key not in stats:
            stats[stats_key] = SyscallStats(num, name)
        stats[stats_key].add(duration_us, outcome)
        summary.matched += 1

    unmatched_enters = sum(len(queue) for queue in pending.values())
    return stats, summary, unmatched_enters


def sort_stats(stats: Iterable[SyscallStats], sort_key: str) -> list[SyscallStats]:
    key_funcs = {
        "total": lambda item: (item.total_us, item.count, item.max_us),
        "avg": lambda item: (item.avg_us, item.count, item.total_us),
        "count": lambda item: (item.count, item.total_us, item.max_us),
        "max": lambda item: (item.max_us, item.total_us, item.count),
        "name": lambda item: (item.name, item.num),
        "num": lambda item: (item.num, item.name),
    }
    reverse = sort_key not in {"name", "num"}
    return sorted(stats, key=key_funcs[sort_key], reverse=reverse)


def write_text(rows: list[SyscallStats], summary: ParseSummary, unmatched_enters: int, limit: int | None) -> None:
    if limit is not None:
        rows = rows[:limit]

    headers = ("NUM", "NAME", "COUNT", "OK", "ERR", "TOTAL_US", "AVG_US", "MIN_US", "P95_US", "MAX_US")
    print(
        f"{headers[0]:>5}  {headers[1]:<24}  {headers[2]:>8}  {headers[3]:>8}  {headers[4]:>8}  "
        f"{headers[5]:>12}  {headers[6]:>12}  {headers[7]:>10}  {headers[8]:>10}  {headers[9]:>10}"
    )
    print("-" * 124)
    for item in rows:
        print(
            f"{item.num:>5}  {item.name:<24}  {item.count:>8}  {item.ok:>8}  {item.err:>8}  "
            f"{item.total_us:>12}  {item.avg_us:>12.2f}  {item.min_us:>10}  "
            f"{item.percentile_us(0.95):>10}  {item.max_us:>10}"
        )

    print()
    print(
        "summary: "
        f"lines={summary.lines}, syscall_lines={summary.syscall_lines}, matched={summary.matched}, "
        f"unmatched_enter={unmatched_enters}, unmatched_result={summary.unmatched_results}, "
        f"malformed={summary.malformed_syscall_lines}"
    )


def write_csv(rows: list[SyscallStats], summary: ParseSummary, unmatched_enters: int, out: TextIO, limit: int | None) -> None:
    if limit is not None:
        rows = rows[:limit]

    writer = csv.writer(out)
    writer.writerow(["num", "name", "count", "ok", "err", "total_us", "avg_us", "min_us", "p95_us", "max_us"])
    for item in rows:
        writer.writerow(
            [
                item.num,
                item.name,
                item.count,
                item.ok,
                item.err,
                item.total_us,
                f"{item.avg_us:.2f}",
                item.min_us,
                item.percentile_us(0.95),
                item.max_us,
            ]
        )

    writer.writerow([])
    writer.writerow(["summary", "lines", summary.lines])
    writer.writerow(["summary", "syscall_lines", summary.syscall_lines])
    writer.writerow(["summary", "matched", summary.matched])
    writer.writerow(["summary", "unmatched_enter", unmatched_enters])
    writer.writerow(["summary", "unmatched_result", summary.unmatched_results])
    writer.writerow(["summary", "malformed", summary.malformed_syscall_lines])


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Aggregate KernelX syscall trace latency from debug console logs.")
    parser.add_argument("logs", nargs="*", type=Path, help="Log files to read. Use '-' or omit files to read stdin.")
    parser.add_argument(
        "--sort",
        choices=("total", "avg", "count", "max", "name", "num"),
        default="total",
        help="Sort output rows by this column. Default: total.",
    )
    parser.add_argument("--limit", type=int, help="Only print the first N rows after sorting.")
    parser.add_argument("--csv", action="store_true", help="Write CSV instead of a text table.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    stats, summary, unmatched_enters = aggregate(args.logs)
    rows = sort_stats(stats.values(), args.sort)

    if args.csv:
        write_csv(rows, summary, unmatched_enters, sys.stdout, args.limit)
    else:
        write_text(rows, summary, unmatched_enters, args.limit)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Build compact, queryable reports from qperf folded stacks.

The input format is one stack per line with a sample count at the end:

    [CPU 0];main;kernelx::foo 1

The build command aggregates identical stacks and writes a small Markdown
summary, TSV tables, a lossless folded file, and a SQLite index.  The query
command reads the SQLite index so callers can inspect a hotspot without loading
the complete profile into an LLM context.

Examples:

    python3 scripts/qperf_report.py output/qperf/kernelx.folded
    python3 scripts/qperf_report.py build kernelx.folded --top-stacks 300
    python3 scripts/qperf_report.py query kernelx.report/profile.sqlite top
    python3 scripts/qperf_report.py query kernelx.report/profile.sqlite \
        stacks --contains ext4_native --limit 20
"""

import argparse
import csv
import hashlib
import json
import re
import sqlite3
import sys
from collections import Counter
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Sequence, TextIO


CPU_FRAME_RE = re.compile(r"^\[CPU (?P<cpu>\d+)\]$")
UNKNOWN_PREFIX = "??"
GENERATED_FILES = (
    "summary.md",
    "manifest.json",
    "top-functions.tsv",
    "top-stacks.folded",
    "subsystems.tsv",
    "callgraph.tsv",
    "full-aggregated.folded",
    "profile.sqlite",
)
SUBSYSTEM_PATTERNS: dict[str, tuple[str, ...]] = {
    "trap": ("::traphandle::", "::kernel::trap::", "kerneltrap", "usertrap"),
    "syscall": ("::kernel::syscall::",),
    "memory": ("::kernel::mm::",),
    "scheduler": ("::kernel::scheduler::", "::kernel::task::", "::kernel::kthread::"),
    "ext4_native": ("::fs::ext4_native::",),
    "ext4_lwext4": ("::fs::ext4::",),
    "vfs": ("::fs::vfs::", "::fs::inode::", "::fs::file::"),
    "tmpfs_memtreefs": ("::fs::tmpfs::", "::fs::memtreefs::"),
    "network": ("::net::", "::fs::socket::"),
    "driver": ("::driver::", "virtio_drivers::"),
    "allocator": ("::kalloc::", "buddy_slab_allocator::", "alloc::alloc::"),
    "memory_copy": ("memcpy", "memset", "compiler_builtins::mem::"),
}


class FoldedFormatError(ValueError):
    """Raised when a folded-stack input line cannot be parsed safely."""


@dataclass
class Profile:
    input_path: Path
    input_bytes: int
    input_sha256: str
    merge_cpus: bool
    records: int
    total_samples: int
    cpu_samples: Counter[str]
    stack_samples: Counter[tuple[str, ...]]


@dataclass
class Metrics:
    self_samples: Counter[str]
    inclusive_samples: Counter[str]
    edge_samples: Counter[tuple[str, str]]
    subsystem_samples: Counter[str]
    unknown_samples: int
    unknown_leaf_samples: int
    weighted_depth: int
    max_depth: int
    sorted_stacks: list[tuple[tuple[str, ...], int]]


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("value must be greater than zero")
    return parsed


def default_output_dir(input_path: Path) -> Path:
    return input_path.with_name(f"{input_path.stem}.report")


def semantic_frames(frames: Sequence[str]) -> tuple[str, ...]:
    result = []
    for frame in frames:
        if not frame or CPU_FRAME_RE.match(frame):
            continue
        result.append(frame)
    return tuple(result) or (UNKNOWN_PREFIX,)


def is_unknown(frame: str) -> bool:
    return frame.startswith(UNKNOWN_PREFIX)


def display_symbol(symbol: str, limit: int = 160) -> str:
    if len(symbol) <= limit:
        return symbol
    head = max(40, limit // 2 - 3)
    tail = max(40, limit - head - 3)
    return f"{symbol[:head]}...{symbol[-tail:]}"


def sorted_symbols(metrics: Metrics) -> list[str]:
    return sorted(metrics.inclusive_samples.keys() | metrics.self_samples.keys())


def symbol_id_map(metrics: Metrics) -> dict[str, int]:
    return {name: symbol_id for symbol_id, name in enumerate(sorted_symbols(metrics), 1)}


def parse_profile(input_path: Path, merge_cpus: bool) -> Profile:
    stack_samples: Counter[tuple[str, ...]] = Counter()
    cpu_samples: Counter[str] = Counter()
    digest = hashlib.sha256()
    records = 0
    total_samples = 0

    with input_path.open("rb") as folded:
        for line_no, raw_line in enumerate(folded, 1):
            digest.update(raw_line)
            try:
                line = raw_line.decode("utf-8").strip()
            except UnicodeDecodeError as error:
                raise FoldedFormatError(f"{input_path}:{line_no}: input is not valid UTF-8") from error
            if not line:
                continue

            try:
                stack_text, count_text = line.rsplit(maxsplit=1)
                count = int(count_text)
            except (ValueError, TypeError) as error:
                raise FoldedFormatError(
                    f"{input_path}:{line_no}: expected '<frame;frame> <positive-count>'"
                ) from error
            if count <= 0:
                raise FoldedFormatError(f"{input_path}:{line_no}: sample count must be positive")

            frames = tuple(stack_text.split(";"))
            cpu_match = CPU_FRAME_RE.match(frames[0]) if frames else None
            cpu = cpu_match.group("cpu") if cpu_match else "unlabeled"
            cpu_samples[cpu] += count
            if merge_cpus and cpu_match:
                frames = frames[1:]
            if not frames:
                frames = (UNKNOWN_PREFIX,)

            stack_samples[frames] += count
            records += 1
            total_samples += count

    if total_samples == 0:
        raise FoldedFormatError(f"{input_path}: profile contains no samples")

    return Profile(
        input_path=input_path,
        input_bytes=input_path.stat().st_size,
        input_sha256=digest.hexdigest(),
        merge_cpus=merge_cpus,
        records=records,
        total_samples=total_samples,
        cpu_samples=cpu_samples,
        stack_samples=stack_samples,
    )


def classify_subsystems(frames: Sequence[str]) -> set[str]:
    categories = set()
    if any(is_unknown(frame) for frame in frames):
        categories.add("unknown")
    for name, patterns in SUBSYSTEM_PATTERNS.items():
        if any(pattern in frame for frame in frames for pattern in patterns):
            categories.add(name)
    if any(frame.startswith("ext4_") for frame in frames):
        categories.add("ext4_lwext4")
    if not categories:
        categories.add("other")
    return categories


def build_metrics(profile: Profile) -> Metrics:
    self_samples: Counter[str] = Counter()
    inclusive_samples: Counter[str] = Counter()
    edge_samples: Counter[tuple[str, str]] = Counter()
    subsystem_samples: Counter[str] = Counter()
    unknown_samples = 0
    unknown_leaf_samples = 0
    weighted_depth = 0
    max_depth = 0

    for stack, samples in profile.stack_samples.items():
        frames = semantic_frames(stack)
        leaf = frames[-1]
        self_samples[leaf] += samples
        for frame in set(frames):
            inclusive_samples[frame] += samples
        for edge in set(zip(frames, frames[1:])):
            edge_samples[edge] += samples
        for subsystem in classify_subsystems(frames):
            subsystem_samples[subsystem] += samples

        if any(is_unknown(frame) for frame in frames):
            unknown_samples += samples
        if is_unknown(leaf):
            unknown_leaf_samples += samples
        weighted_depth += len(frames) * samples
        max_depth = max(max_depth, len(frames))

    sorted_stacks = sorted(profile.stack_samples.items(), key=lambda item: (-item[1], item[0]))
    return Metrics(
        self_samples=self_samples,
        inclusive_samples=inclusive_samples,
        edge_samples=edge_samples,
        subsystem_samples=subsystem_samples,
        unknown_samples=unknown_samples,
        unknown_leaf_samples=unknown_leaf_samples,
        weighted_depth=weighted_depth,
        max_depth=max_depth,
        sorted_stacks=sorted_stacks,
    )


def percent(samples: int, total_samples: int) -> float:
    return samples * 100.0 / total_samples


def coverage_at(sorted_stacks: Sequence[tuple[tuple[str, ...], int]], limits: Iterable[int]) -> dict[int, int]:
    requested = sorted(set(limits))
    result = {}
    cumulative = 0
    next_limit = 0
    for rank, (_stack, samples) in enumerate(sorted_stacks, 1):
        cumulative += samples
        while next_limit < len(requested) and rank == requested[next_limit]:
            result[requested[next_limit]] = cumulative
            next_limit += 1
        if next_limit == len(requested):
            break
    for limit in requested[next_limit:]:
        result[limit] = cumulative
    return result


def prepare_output_dir(output_dir: Path, force: bool) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    conflicts = [output_dir / name for name in GENERATED_FILES if (output_dir / name).exists()]
    if conflicts and not force:
        names = ", ".join(path.name for path in conflicts)
        raise FileExistsError(f"{output_dir} already contains generated files ({names}); pass --force to replace them")


def write_aggregated_folded(path: Path, stacks: Sequence[tuple[tuple[str, ...], int]]) -> None:
    with path.open("w", encoding="utf-8", newline="") as output:
        for stack, samples in stacks:
            output.write(f"{';'.join(stack)} {samples}\n")


def write_functions_tsv(
    path: Path,
    profile: Profile,
    metrics: Metrics,
    limit: int,
) -> list[str]:
    inclusive_top = [name for name, _samples in metrics.inclusive_samples.most_common(limit)]
    self_top = [name for name, _samples in metrics.self_samples.most_common(limit)]
    selected = set(inclusive_top) | set(self_top)
    symbols = sorted(selected, key=lambda name: (-metrics.inclusive_samples[name], -metrics.self_samples[name], name))
    symbol_ids = symbol_id_map(metrics)

    with path.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "symbol_id",
                "display_name",
                "full_name",
                "self_samples",
                "self_percent",
                "inclusive_samples",
                "inclusive_percent",
            ]
        )
        for name in symbols:
            writer.writerow(
                [
                    symbol_ids[name],
                    display_symbol(name),
                    name,
                    metrics.self_samples[name],
                    f"{percent(metrics.self_samples[name], profile.total_samples):.4f}",
                    metrics.inclusive_samples[name],
                    f"{percent(metrics.inclusive_samples[name], profile.total_samples):.4f}",
                ]
            )
    return symbols


def write_subsystems_tsv(path: Path, profile: Profile, metrics: Metrics) -> None:
    with path.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(["subsystem", "samples_containing_subsystem", "percent"])
        for name, samples in metrics.subsystem_samples.most_common():
            writer.writerow([name, samples, f"{percent(samples, profile.total_samples):.4f}"])


def write_callgraph_tsv(path: Path, profile: Profile, metrics: Metrics, limit: int) -> None:
    edges = sorted(metrics.edge_samples.items(), key=lambda item: (-item[1], item[0]))[:limit]
    with path.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(["caller", "callee", "samples", "percent"])
        for (caller, callee), samples in edges:
            writer.writerow([caller, callee, samples, f"{percent(samples, profile.total_samples):.4f}"])


def sqlite_metadata(profile: Profile, metrics: Metrics) -> dict[str, object]:
    return {
        "input": str(profile.input_path.resolve()),
        "input_bytes": profile.input_bytes,
        "input_sha256": profile.input_sha256,
        "records": profile.records,
        "total_samples": profile.total_samples,
        "unique_stacks": len(profile.stack_samples),
        "merge_cpus": profile.merge_cpus,
        "unknown_samples": metrics.unknown_samples,
        "unknown_leaf_samples": metrics.unknown_leaf_samples,
        "average_depth": metrics.weighted_depth / profile.total_samples,
        "max_depth": metrics.max_depth,
    }


def write_sqlite(path: Path, profile: Profile, metrics: Metrics, force: bool) -> None:
    if path.exists():
        if not force:
            raise FileExistsError(f"{path} already exists; pass --force to replace it")
        path.unlink()

    symbols = sorted_symbols(metrics)
    symbol_ids = symbol_id_map(metrics)
    connection = sqlite3.connect(path)
    try:
        connection.executescript(
            """
            PRAGMA journal_mode = OFF;
            PRAGMA synchronous = OFF;
            CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE symbols (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                self_samples INTEGER NOT NULL,
                inclusive_samples INTEGER NOT NULL
            );
            CREATE TABLE stacks (
                id INTEGER PRIMARY KEY,
                samples INTEGER NOT NULL,
                depth INTEGER NOT NULL,
                has_unknown INTEGER NOT NULL
            );
            CREATE TABLE stack_frames (
                stack_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                symbol_id INTEGER NOT NULL,
                PRIMARY KEY (stack_id, position),
                FOREIGN KEY (stack_id) REFERENCES stacks(id),
                FOREIGN KEY (symbol_id) REFERENCES symbols(id)
            );
            CREATE TABLE edges (
                caller_symbol_id INTEGER NOT NULL,
                callee_symbol_id INTEGER NOT NULL,
                samples INTEGER NOT NULL,
                PRIMARY KEY (caller_symbol_id, callee_symbol_id),
                FOREIGN KEY (caller_symbol_id) REFERENCES symbols(id),
                FOREIGN KEY (callee_symbol_id) REFERENCES symbols(id)
            );
            CREATE TABLE cpus (cpu TEXT PRIMARY KEY, samples INTEGER NOT NULL);
            CREATE TABLE subsystems (name TEXT PRIMARY KEY, samples INTEGER NOT NULL);
            """
        )
        connection.executemany(
            "INSERT INTO metadata(key, value) VALUES (?, ?)",
            ((key, json.dumps(value, ensure_ascii=False)) for key, value in sqlite_metadata(profile, metrics).items()),
        )
        connection.executemany(
            "INSERT INTO symbols(id, name, display_name, self_samples, inclusive_samples) VALUES (?, ?, ?, ?, ?)",
            (
                (
                    symbol_ids[name],
                    name,
                    display_symbol(name),
                    metrics.self_samples[name],
                    metrics.inclusive_samples[name],
                )
                for name in symbols
            ),
        )

        stack_rows = []
        frame_rows = []
        for stack_id, (stack, samples) in enumerate(metrics.sorted_stacks, 1):
            frames = semantic_frames(stack)
            stack_rows.append((stack_id, samples, len(frames), int(any(is_unknown(frame) for frame in frames))))
            frame_rows.extend((stack_id, position, symbol_ids[frame]) for position, frame in enumerate(frames))
        connection.executemany("INSERT INTO stacks(id, samples, depth, has_unknown) VALUES (?, ?, ?, ?)", stack_rows)
        connection.executemany(
            "INSERT INTO stack_frames(stack_id, position, symbol_id) VALUES (?, ?, ?)", frame_rows
        )
        connection.executemany(
            "INSERT INTO edges(caller_symbol_id, callee_symbol_id, samples) VALUES (?, ?, ?)",
            (
                (symbol_ids[caller], symbol_ids[callee], samples)
                for (caller, callee), samples in metrics.edge_samples.items()
            ),
        )
        connection.executemany("INSERT INTO cpus(cpu, samples) VALUES (?, ?)", profile.cpu_samples.items())
        connection.executemany(
            "INSERT INTO subsystems(name, samples) VALUES (?, ?)", metrics.subsystem_samples.items()
        )
        connection.executescript(
            """
            CREATE INDEX symbols_inclusive_idx ON symbols(inclusive_samples DESC);
            CREATE INDEX symbols_self_idx ON symbols(self_samples DESC);
            CREATE INDEX stack_frames_symbol_idx ON stack_frames(symbol_id, stack_id);
            CREATE INDEX edges_callee_idx ON edges(callee_symbol_id, samples DESC);
            CREATE INDEX edges_caller_idx ON edges(caller_symbol_id, samples DESC);
            """
        )
        connection.commit()
    finally:
        connection.close()


def markdown_escape(value: str) -> str:
    return value.replace("|", "\\|").replace("`", "'")


def short_stack(stack: Sequence[str], depth: int = 5) -> str:
    frames = semantic_frames(stack)
    selected = frames[-depth:]
    prefix = "...;" if len(frames) > depth else ""
    return prefix + ";".join(display_symbol(frame, 100) for frame in selected)


def write_summary(
    path: Path,
    profile: Profile,
    metrics: Metrics,
    aggregate_bytes: int,
    top_stack_limit: int,
) -> None:
    coverage_limits = [10, 50, 100, 200, 500, 1000, min(top_stack_limit, len(metrics.sorted_stacks))]
    coverage = coverage_at(metrics.sorted_stacks, coverage_limits)
    compression = aggregate_bytes * 100.0 / profile.input_bytes if profile.input_bytes else 0.0
    generated_at = datetime.now(timezone.utc).isoformat()

    with path.open("w", encoding="utf-8") as output:
        output.write("# KernelX qperf profile summary\n\n")
        output.write(f"- Source: `{profile.input_path.resolve()}`\n")
        output.write(f"- Generated at: `{generated_at}`\n")
        output.write(f"- Input SHA-256: `{profile.input_sha256}`\n")
        output.write(f"- Samples: **{profile.total_samples:,}** from {profile.records:,} input records\n")
        output.write(f"- Unique stacks after aggregation: **{len(profile.stack_samples):,}**\n")
        output.write(
            f"- Aggregated folded size: **{aggregate_bytes:,} bytes** "
            f"({compression:.2f}% of the input size)\n"
        )
        output.write(
            f"- Stack depth: average **{metrics.weighted_depth / profile.total_samples:.2f}**, "
            f"maximum **{metrics.max_depth}**\n"
        )
        output.write(
            f"- Samples containing unresolved frames: **{metrics.unknown_samples:,} "
            f"({percent(metrics.unknown_samples, profile.total_samples):.2f}%)**\n"
        )
        output.write(
            f"- Samples whose leaf is unresolved: **{metrics.unknown_leaf_samples:,} "
            f"({percent(metrics.unknown_leaf_samples, profile.total_samples):.2f}%)**\n\n"
        )
        if metrics.unknown_samples * 10 >= profile.total_samples:
            output.write(
                "> Warning: at least 10% of samples contain `??`. Use the matching `qperf.bin` "
                "and ELF to recover raw instruction pointers before attributing those samples.\n\n"
            )

        output.write("## CPU distribution\n\n")
        output.write("| CPU | Samples | Percent |\n|---|---:|---:|\n")
        for cpu, samples in sorted(profile.cpu_samples.items()):
            output.write(f"| {cpu} | {samples:,} | {percent(samples, profile.total_samples):.2f}% |\n")

        output.write("\n## Stack coverage\n\n")
        output.write("| Top stacks | Samples covered | Coverage |\n|---:|---:|---:|\n")
        for limit in sorted(coverage):
            samples = coverage[limit]
            output.write(f"| {limit:,} | {samples:,} | {percent(samples, profile.total_samples):.2f}% |\n")

        output.write("\n## Top leaf functions (self samples)\n\n")
        output.write("| Rank | Samples | Percent | Function |\n|---:|---:|---:|---|\n")
        for rank, (name, samples) in enumerate(metrics.self_samples.most_common(20), 1):
            output.write(
                f"| {rank} | {samples:,} | {percent(samples, profile.total_samples):.2f}% | "
                f"`{markdown_escape(display_symbol(name))}` |\n"
            )

        output.write("\n## Top inclusive functions\n\n")
        output.write("| Rank | Samples | Percent | Function |\n|---:|---:|---:|---|\n")
        for rank, (name, samples) in enumerate(metrics.inclusive_samples.most_common(20), 1):
            output.write(
                f"| {rank} | {samples:,} | {percent(samples, profile.total_samples):.2f}% | "
                f"`{markdown_escape(display_symbol(name))}` |\n"
            )

        output.write("\n## Subsystem presence\n\n")
        output.write(
            "Subsystem rows overlap: a sample contributes to every subsystem represented in its stack.\n\n"
        )
        output.write("| Subsystem | Samples | Percent |\n|---|---:|---:|\n")
        for name, samples in metrics.subsystem_samples.most_common():
            output.write(f"| {name} | {samples:,} | {percent(samples, profile.total_samples):.2f}% |\n")

        output.write("\n## Top complete stacks\n\n")
        output.write("| Rank | Samples | Percent | Leaf-side stack suffix |\n|---:|---:|---:|---|\n")
        for rank, (stack, samples) in enumerate(metrics.sorted_stacks[:20], 1):
            output.write(
                f"| {rank} | {samples:,} | {percent(samples, profile.total_samples):.2f}% | "
                f"`{markdown_escape(short_stack(stack))}` |\n"
            )

        output.write("\n## On-demand queries\n\n")
        output.write("```bash\n")
        output.write(f"python3 scripts/qperf_report.py query {path.parent / 'profile.sqlite'} stats\n")
        output.write(f"python3 scripts/qperf_report.py query {path.parent / 'profile.sqlite'} top --limit 50\n")
        output.write(
            f"python3 scripts/qperf_report.py query {path.parent / 'profile.sqlite'} "
            "stacks --contains ext4_native --limit 20\n"
        )
        output.write(
            f"python3 scripts/qperf_report.py query {path.parent / 'profile.sqlite'} "
            "callers --symbol alloc_blocks --limit 20\n"
        )
        output.write("```\n\n")
        output.write(
            "Start LLM analysis with this file, then use the SQLite queries for individual hotspots. "
            "Do not load `full-aggregated.folded` unless exact stack evidence is required.\n"
        )


def write_manifest(
    path: Path,
    profile: Profile,
    metrics: Metrics,
    output_dir: Path,
    top_function_limit: int,
    top_stack_limit: int,
    top_edge_limit: int,
) -> None:
    files = {}
    for name in GENERATED_FILES:
        generated_path = output_dir / name
        if generated_path.exists() and generated_path != path:
            files[name] = {"bytes": generated_path.stat().st_size}
    manifest = {
        "format_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "profile": sqlite_metadata(profile, metrics),
        "limits": {
            "top_functions_per_metric": top_function_limit,
            "top_stacks": top_stack_limit,
            "top_edges": top_edge_limit,
        },
        "cpu_samples": dict(sorted(profile.cpu_samples.items())),
        "subsystem_samples": dict(metrics.subsystem_samples.most_common()),
        "files": files,
    }
    with path.open("w", encoding="utf-8") as output:
        json.dump(manifest, output, ensure_ascii=False, indent=2)
        output.write("\n")


def build_report(args: argparse.Namespace) -> int:
    input_path = args.input.resolve()
    if not input_path.is_file():
        raise FileNotFoundError(f"input profile does not exist: {input_path}")
    output_dir = (args.output or default_output_dir(input_path)).resolve()
    prepare_output_dir(output_dir, args.force)

    profile = parse_profile(input_path, merge_cpus=not args.keep_cpu)
    metrics = build_metrics(profile)
    full_aggregated = output_dir / "full-aggregated.folded"
    write_aggregated_folded(full_aggregated, metrics.sorted_stacks)
    write_aggregated_folded(output_dir / "top-stacks.folded", metrics.sorted_stacks[: args.top_stacks])
    write_functions_tsv(output_dir / "top-functions.tsv", profile, metrics, args.top_functions)
    write_subsystems_tsv(output_dir / "subsystems.tsv", profile, metrics)
    write_callgraph_tsv(output_dir / "callgraph.tsv", profile, metrics, args.top_edges)
    write_sqlite(output_dir / "profile.sqlite", profile, metrics, args.force)
    write_summary(
        output_dir / "summary.md",
        profile,
        metrics,
        full_aggregated.stat().st_size,
        args.top_stacks,
    )
    write_manifest(
        output_dir / "manifest.json",
        profile,
        metrics,
        output_dir,
        args.top_functions,
        args.top_stacks,
        args.top_edges,
    )

    print(f"qperf report: {output_dir}")
    print(f"summary: {output_dir / 'summary.md'}")
    print(f"samples: {profile.total_samples}, unique stacks: {len(profile.stack_samples)}")
    print(
        f"unresolved samples: {metrics.unknown_samples} "
        f"({percent(metrics.unknown_samples, profile.total_samples):.2f}%)"
    )
    return 0


def open_database(path: Path) -> sqlite3.Connection:
    if not path.is_file():
        raise FileNotFoundError(f"SQLite profile does not exist: {path}")
    connection = sqlite3.connect(f"file:{path.resolve()}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    return connection


def database_total_samples(connection: sqlite3.Connection) -> int:
    row = connection.execute("SELECT value FROM metadata WHERE key = 'total_samples'").fetchone()
    if row is None:
        raise ValueError("SQLite profile has no total_samples metadata")
    return int(json.loads(row["value"]))


def print_symbol_rows(rows: Iterable[sqlite3.Row], total_samples: int, label: str) -> None:
    print(f"{'SAMPLES':>10}  {'PERCENT':>8}  {label}")
    for row in rows:
        samples = int(row["samples"])
        print(f"{samples:>10}  {percent(samples, total_samples):>7.2f}%  {row['display_name']}")


def query_top(connection: sqlite3.Connection, args: argparse.Namespace) -> None:
    metric = "self_samples" if args.metric == "self" else "inclusive_samples"
    rows = connection.execute(
        f"SELECT display_name, {metric} AS samples FROM symbols "
        f"WHERE {metric} > 0 ORDER BY {metric} DESC, name LIMIT ?",
        (args.limit,),
    )
    print_symbol_rows(rows, database_total_samples(connection), "FUNCTION")


def query_stats(connection: sqlite3.Connection) -> None:
    metadata = {
        row["key"]: json.loads(row["value"])
        for row in connection.execute("SELECT key, value FROM metadata ORDER BY key")
    }
    database_samples, database_stacks = connection.execute(
        "SELECT COALESCE(SUM(samples), 0), COUNT(*) FROM stacks"
    ).fetchone()
    metadata["database_samples"] = database_samples
    metadata["database_stacks"] = database_stacks
    for key, value in metadata.items():
        print(f"{key}: {value}")

    total_samples = int(metadata["total_samples"])
    print("\nCPU distribution:")
    for row in connection.execute("SELECT cpu, samples FROM cpus ORDER BY cpu"):
        print(f"  {row['cpu']}: {row['samples']} ({percent(row['samples'], total_samples):.2f}%)")
    print("\nSubsystem presence (overlapping):")
    for row in connection.execute("SELECT name, samples FROM subsystems ORDER BY samples DESC, name"):
        print(f"  {row['name']}: {row['samples']} ({percent(row['samples'], total_samples):.2f}%)")


def matching_stack_rows(
    connection: sqlite3.Connection, pattern: str, limit: int
) -> list[sqlite3.Row]:
    return connection.execute(
        """
        SELECT DISTINCT stacks.id, stacks.samples
        FROM stacks
        JOIN stack_frames ON stack_frames.stack_id = stacks.id
        JOIN symbols ON symbols.id = stack_frames.symbol_id
        WHERE symbols.name LIKE ?
        ORDER BY stacks.samples DESC, stacks.id
        LIMIT ?
        """,
        (f"%{pattern}%", limit),
    ).fetchall()


def query_stacks(connection: sqlite3.Connection, args: argparse.Namespace) -> None:
    total_samples = database_total_samples(connection)
    rows = matching_stack_rows(connection, args.contains, args.limit)
    for rank, row in enumerate(rows, 1):
        frame_rows = connection.execute(
            """
            SELECT symbols.name, symbols.display_name
            FROM stack_frames
            JOIN symbols ON symbols.id = stack_frames.symbol_id
            WHERE stack_frames.stack_id = ?
            ORDER BY stack_frames.position
            """,
            (row["id"],),
        ).fetchall()
        matching_positions = [
            position for position, frame in enumerate(frame_rows) if args.contains in frame["name"]
        ]
        if args.full_stack or not matching_positions:
            start = 0
            end = len(frame_rows)
        else:
            match = matching_positions[0]
            start = max(0, match - args.context)
            end = min(len(frame_rows), match + args.context + 1)
        displayed = [frame["display_name"] for frame in frame_rows[start:end]]
        if start:
            displayed.insert(0, "...")
        if end < len(frame_rows):
            displayed.append("...")
        stack = ";".join(displayed)
        print(
            f"{rank}. {row['samples']} samples ({percent(row['samples'], total_samples):.2f}%), "
            f"depth={len(frame_rows)}"
        )
        print(f"   {stack}")


def query_edges(connection: sqlite3.Connection, args: argparse.Namespace, direction: str) -> None:
    if direction == "callers":
        selected_id = "edges.callee_symbol_id"
        related_id = "edges.caller_symbol_id"
        label = "CALLER"
    else:
        selected_id = "edges.caller_symbol_id"
        related_id = "edges.callee_symbol_id"
        label = "CALLEE"
    rows = connection.execute(
        f"""
        SELECT related.display_name, SUM(edges.samples) AS samples
        FROM edges
        JOIN symbols AS selected ON selected.id = {selected_id}
        JOIN symbols AS related ON related.id = {related_id}
        WHERE selected.name LIKE ?
        GROUP BY related.id
        ORDER BY samples DESC, related.name
        LIMIT ?
        """,
        (f"%{args.symbol}%", args.limit),
    )
    print_symbol_rows(rows, database_total_samples(connection), label)


def query_report(args: argparse.Namespace) -> int:
    with open_database(args.database) as connection:
        if args.query_command == "stats":
            query_stats(connection)
        elif args.query_command == "top":
            query_top(connection, args)
        elif args.query_command == "stacks":
            query_stacks(connection, args)
        elif args.query_command in {"callers", "callees"}:
            query_edges(connection, args, args.query_command)
        else:
            raise ValueError(f"unsupported query command: {args.query_command}")
    return 0


def add_limit_argument(parser: argparse.ArgumentParser, default: int) -> None:
    parser.add_argument("--limit", type=positive_int, default=default, help=f"Maximum rows to print. Default: {default}.")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    effective_argv = list(sys.argv[1:] if argv is None else argv)
    if effective_argv and effective_argv[0] not in {"build", "query", "-h", "--help"}:
        effective_argv.insert(0, "build")

    parser = argparse.ArgumentParser(description="Build and query compact reports for KernelX qperf folded profiles.")
    commands = parser.add_subparsers(dest="command", required=True)

    build = commands.add_parser("build", help="Build a report directory from a folded profile.")
    build.add_argument("input", type=Path, help="Input folded-stack file.")
    build.add_argument("-o", "--output", type=Path, help="Output directory. Default: <input-stem>.report.")
    build.add_argument("--top-functions", type=positive_int, default=300, help="Rows per function metric. Default: 300.")
    build.add_argument("--top-stacks", type=positive_int, default=500, help="Stacks in top-stacks.folded. Default: 500.")
    build.add_argument("--top-edges", type=positive_int, default=1000, help="Edges in callgraph.tsv. Default: 1000.")
    build.add_argument("--keep-cpu", action="store_true", help="Keep [CPU N] as an aggregation key in folded outputs.")
    build.add_argument("--force", action="store_true", help="Replace known generated files in an existing output directory.")

    query = commands.add_parser("query", help="Query a generated profile.sqlite without loading the full profile.")
    query.add_argument("database", type=Path, help="Path to profile.sqlite.")
    query_commands = query.add_subparsers(dest="query_command", required=True)

    query_commands.add_parser("stats", help="Show profile metadata, sample totals, CPUs, and subsystems.")

    top = query_commands.add_parser("top", help="Show functions sorted by self or inclusive samples.")
    top.add_argument("--metric", choices=("inclusive", "self"), default="inclusive")
    add_limit_argument(top, 50)

    stacks = query_commands.add_parser("stacks", help="Show hot stacks containing a symbol substring.")
    stacks.add_argument("--contains", required=True, help="Case-sensitive substring matched against full symbol names.")
    stacks.add_argument(
        "--context",
        type=positive_int,
        default=5,
        help="Frames to show on each side of the first matching frame. Default: 5.",
    )
    stacks.add_argument("--full-stack", action="store_true", help="Print complete matching stacks.")
    add_limit_argument(stacks, 20)

    for name in ("callers", "callees"):
        edge_query = query_commands.add_parser(name, help=f"Show weighted {name} of matching symbols.")
        edge_query.add_argument("--symbol", required=True, help="Case-sensitive substring matched against full symbol names.")
        add_limit_argument(edge_query, 20)

    return parser.parse_args(effective_argv)


def main() -> int:
    args = parse_args()
    try:
        if args.command == "build":
            return build_report(args)
        return query_report(args)
    except (FoldedFormatError, FileExistsError, FileNotFoundError, sqlite3.DatabaseError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

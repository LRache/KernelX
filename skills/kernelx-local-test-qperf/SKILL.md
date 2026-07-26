---
name: kernelx-local-test-qperf
description: Interpret existing KernelX qperf artifacts, including timestamped report directories, summary.md, profile.sqlite, folded stacks, unresolved.tsv, raw qperf.bin, FlameGraphs, and matching ELFs. Use when Codex must validate profile quality, explain self versus inclusive samples, recover unresolved symbols, separate trap-entry sampling bias from real kernel work, trace hotspots into the current KernelX source, or rank performance hypotheses. This skill analyzes already-produced data; it does not configure KernelX, launch QEMU, run qperf, or generate new samples.
---

# KernelX Qperf Data Interpretation

## Scope

Analyze existing qperf evidence. Do not:

- run `make run-qperf`, QEMU, tests, or benchmarks
- change Kconfig or QEMU parameters to produce another profile
- present profiling setup instructions unless the user explicitly asks for them
- modify kernel code when the user only asks for analysis

Using bounded report queries, symbolizing an existing raw trace, and reading matching source code are analysis steps, not profile generation.

## Evidence Hierarchy

Keep every conclusion scoped to one timestamped artifact set:

1. `<name>.report/summary.md`
2. `<name>.report/manifest.json`
3. `<name>.report/profile.sqlite`
4. `<name>.report/top-stacks.folded`
5. `<name>.report/full-aggregated.folded`
6. `<name>.unresolved.tsv`
7. timestamp-matching `qperf.bin` and kernel ELF
8. `<name>.console.log`, syscall log, or workload output
9. current KernelX source corresponding to the profiled ELF

Do not compare profiles until their workload, architecture, CPU count, sampling settings, kernel revision, and artifact identity are comparable.

## Sampling Model

Understand the current qperf plugin before interpreting percentages:

- It uses a host monotonic-clock interval per vCPU.
- It installs callbacks only on high-half kernel instructions.
- Time spent in userspace or `wfi` can exceed the interval without producing a sample.
- The next executed kernel instruction then receives the sample.

Consequences:

- `asm_usertrap_entry` can represent preceding userspace residency, not expensive trap assembly.
- `asm_kerneltrap_entry` can represent preceding idle time or an interrupt boundary.
- A trap-entry-heavy profile is not an unbiased kernel CPU-time profile.
- After removing entry-biased samples, call the remainder an **effective kernel-work subset**, not total CPU time.
- qperf does not measure time blocked on sleep locks or device I/O, so it cannot prove lock contention or quantify off-CPU latency by itself.

## Profile Integrity

Start with `summary.md` and verify:

- the named folded input and timestamp
- input SHA-256 against the actual folded file
- input records, total samples, and database samples agree
- unique-stack count and stack-depth distribution
- unresolved-frame and unresolved-leaf percentages
- top-stack coverage
- CPU distribution

Use the report query interface rather than loading the SQLite database or large folded files directly:

```bash
python3 scripts/qperf_report.py query <report>/profile.sqlite stats
python3 scripts/qperf_report.py query <report>/profile.sqlite top --metric self --limit 50
python3 scripts/qperf_report.py query <report>/profile.sqlite top --metric inclusive --limit 50
python3 scripts/qperf_report.py query <report>/profile.sqlite stacks --contains <symbol> --limit 20
python3 scripts/qperf_report.py query <report>/profile.sqlite callers --symbol <symbol> --limit 20
python3 scripts/qperf_report.py query <report>/profile.sqlite callees --symbol <symbol> --limit 20
```

Treat `subsystems.tsv` as overlapping stack-presence classification. A sample may belong to trap, memory, VFS, and ext4_native simultaneously; subsystem percentages do not form an exclusive 100% partition.

## Metric Semantics

- **Self samples** attribute a sample to its leaf frame.
- **Inclusive samples** count samples whose stack contains the function.
- Inclusive values overlap across callers and callees and must not be added.
- A compiler helper such as `wrapping_offset`, `set_bytes_words`, or `copy_forward_aligned_words` is usually an implementation leaf. Attribute it through its complete parent stack to `memset`, `memcpy`, page initialization, user copy, or another real operation.
- A high-level root such as `usertrap_handler`, `syscall`, or `memory_fault` describes workload shape; it is not automatically the optimization target.
- Top individual stacks and top functions answer different questions. Use both before naming a bottleneck.

When excluding sampling noise, always publish both denominators:

```text
raw samples
  - classified entry/noise samples
  = effective kernel-work samples
```

Recompute percentages against the effective denominator, label them explicitly, and retain raw percentages for auditability.

## Unresolved Symbols

Do not equate `??` with lost samples. Distinguish:

1. sample exists but the function name is missing
2. raw IP was discarded during folded conversion
3. unwinding stopped early
4. no sample was collected

Use `<name>.unresolved.tsv` first when present. Entries such as `??@0x...` preserve enough evidence for `addr2line`, `nm`, or range-based symbol lookup.

If substantial `??` remains and the timestamp-matching raw data and ELF still exist:

- verify their mtimes and build identity before using them
- decode the raw current IP distribution
- map top IPs with the ELF
- use sized text symbols from `nm` for assembly without DWARF function names
- account for the RISC-V early physical-to-virtual alias when applicable
- preserve unresolved addresses rather than collapsing them back to plain `??`

The repository symbolizer in `scripts/qperf_symbolize.rs` can reinterpret an existing raw trace. Build it with `make -f scripts/qperf.mk qperf-symbolizer`. It deduplicates addresses, batches `addr2line`, falls back to sized text symbols, supports the RISC-V address map, and emits unresolved IP counts. Running it on existing artifacts is appropriate; running QEMU to obtain new samples is outside this skill.

A folded-only `main;??` cannot be repaired if the raw IP has already been discarded and no matching `qperf.bin` remains. State that limitation instead of guessing.

## Trap And Idle Classification

When raw IPs resolve to entry assembly:

1. obtain exact symbol ranges from the matching ELF
2. classify samples by current IP, not merely by whether a deeper stack contains a trap function
3. separate `kerneltrap_entry`, `usertrap_entry`, and trap return
4. inspect the remaining unknown samples independently
5. compare raw and filtered CPU distributions

Do not remove an entire stack merely because it contains a trap handler. Real page-fault and syscall work legitimately descends from trap handling. Filter only the entry-biased current-IP samples supported by raw evidence.

Sampling frequency near the kernel timer frequency can amplify entry bias, but a non-aligned frequency does not eliminate the kernel-only sampling effect. Treat frequency as one sanity check, not a complete explanation.

## Hotspot Attribution

For each candidate hotspot:

1. Read representative complete stacks.
2. Separate workload roots from leaf costs.
3. Quantify both inclusive path presence and self cost.
4. Check whether the samples are concentrated on one CPU or spread across CPUs.
5. Trace the path into the current source.
6. Identify the exact repeated operation: allocation, zeroing, copying, bitmap scan, checksum, lookup, locking, device submission, or writeback.
7. State whether the profile proves the cost or only suggests a hypothesis.

Common KernelX interpretations:

- `PrivateFileMapArea::load_page -> pread -> ext4_native::readat` plus `memset` and `memcpy` indicates private file-fault page initialization and copying. Inspect whether pages are cleared before being completely overwritten and whether file-cache pages are copied into anonymous private pages.
- `alloc_blocks -> test_bit` indicates bitmap scanning. Inspect whether allocation repeatedly starts at group/bit zero and whether a cursor or word-at-a-time scan would reduce work.
- CRC leaf samples matter only in proportion to the effective work subset. Do not inherit an old “checksum dominates” conclusion when the current profile says otherwise.
- BTree search, atomics, and allocator helpers are often distributed secondary costs. Confirm their parent paths before proposing a subsystem rewrite.
- Device read/write functions show executed submission work, not the time the task slept waiting for completion.

## Source Cross-Check

Prefer the narrow source surface implied by the stack:

- private file faults: `src/kernel/mm/maparea/filemap/private.rs`
- anonymous faults: `src/kernel/mm/maparea/anonymous/`
- ext4-native cached I/O and writeback: `src/fs/ext4_native/inode.rs`
- ext4-native allocation and metadata: `src/fs/ext4_native/ondisk/`
- VFS path lookup and creation: `src/fs/vfs/`
- syscall wrappers: `src/kernel/syscall/fs.rs`
- page allocation: `src/kernel/mm/page.rs`
- qperf sampling semantics: `tools/qperf/src/profiler.rs`

Tie every optimization idea to a concrete source operation and preserve the distinction between runtime evidence and source-based inference.

## Recommendation Ranking

Rank recommendations using:

1. measured coverage in the effective work subset
2. confidence that the leaf represents real work rather than sampling/symbolization bias
3. semantic and safety risk
4. implementation scope
5. whether qperf can measure the expected improvement

Prefer a narrow fast path or removal of redundant work before a broad redesign. Label feature-disabling changes, such as disabling metadata checksums, as benchmark controls unless the user explicitly accepts the semantic tradeoff.

## Response Contract

Produce an analysis with:

1. **Bottom line** — the real dominant path and the largest distortion
2. **Data quality** — exact artifact, SHA/sample consistency, unresolved coverage, and filtering
3. **Effective hotspots** — raw and filtered percentages with clear denominators
4. **Call path** — a compact source-grounded chain
5. **Ranked actions** — low-risk first, broader redesigns later
6. **Limits** — missing workload output, raw IPs, off-CPU data, or revision identity

Never present unresolved frames, overlapping inclusive percentages, or subsystem presence as exclusive CPU-time attribution.

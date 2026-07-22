/*
 * smp_check.h - 多核竞态测试的前置 CPU 数量检查。
 *
 * 仅用于 usertests/race/ipc 下明确需要 SMP 才能复现的测试。单核环境下这些测试
 * 既无法触发竞态，也无法验证修复，直接跳过以免给出误导性 PASS/HIT 结果。
 */
#ifndef RACE_IPC_SMP_CHECK_H
#define RACE_IPC_SMP_CHECK_H

#include <stdio.h>
#include <unistd.h>

/* 返回当前在线 CPU 核心数；查询失败时保守返回 1。 */
static inline int race_cpu_count(void)
{
    long n = sysconf(_SC_NPROCESSORS_ONLN);
    return n > 0 ? (int)n : 1;
}

/*
 * 竞态测试前置检查。
 *   need     : 复现该竞态所需的最少 CPU 核心数。
 *   casename : 测试名，用于输出。
 * 核心数不足时打印 SKIP 提示并以 77 退出（沿用 automake 的 skip 约定，
 * 与 PASS=0、各 HIT 退出码区分）。
 * 返回 0 表示核心数满足，调用方应继续执行测试。
 */
static inline int race_require_cpus(int need, const char *casename)
{
    int have = race_cpu_count();
    if (have < need) {
        printf("%s: SKIP (need >=%d CPUs, have %d; this race requires SMP)\n",
               casename, need, have);
        return 77;
    }
    return 0;
}

#endif /* RACE_IPC_SMP_CHECK_H */

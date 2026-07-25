// PERF_DEBUG_BEGIN(scheduler-time): Temporary scheduler running/blocked-time
// aggregation. Remove this whole file together with every
// PERF_DEBUG(scheduler-time) call site.
#[cfg(feature = "scheduler-block-reason-debug")]
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "scheduler-block-reason-debug")]
use spin::mutex::SpinMutex;

use crate::klib::InitedCell;
use crate::{arch, println};

#[cfg(feature = "scheduler-block-reason-debug")]
#[derive(Debug, Clone, Copy)]
pub(crate) struct DebugBlockStamp {
    cpu_id: usize,
    start_us: u64,
    name: &'static str,
}

#[cfg(feature = "scheduler-block-reason-debug")]
impl DebugBlockStamp {
    pub(crate) fn new(cpu_id: usize, start_us: u64, name: &'static str) -> Self {
        Self { cpu_id, start_us, name }
    }
}

#[cfg(feature = "scheduler-block-reason-debug")]
#[derive(Default)]
struct DebugCounter {
    total_us: u64,
    count: u64,
}

#[cfg(feature = "scheduler-block-reason-debug")]
impl DebugCounter {
    fn add(&mut self, duration_us: u64) {
        self.total_us = self.total_us.saturating_add(duration_us);
        self.count = self.count.saturating_add(1);
    }

    fn snapshot(&self) -> (u64, u64) {
        (self.total_us, self.count)
    }
}

struct CpuSchedDebug {
    running_us: AtomicU64,
    /// Encoded as `start_us + 1`, so zero means that the CPU is idle.
    running_since_us: AtomicU64,
    #[cfg(feature = "scheduler-block-reason-debug")]
    blocked: SpinMutex<BTreeMap<&'static str, DebugCounter>>,
}

impl CpuSchedDebug {
    fn new() -> Self {
        Self {
            running_us: AtomicU64::new(0),
            running_since_us: AtomicU64::new(0),
            #[cfg(feature = "scheduler-block-reason-debug")]
            blocked: SpinMutex::new(BTreeMap::new()),
        }
    }

    fn running_snapshot(&self, now_us: u64) -> u64 {
        let total = self.running_us.load(Ordering::Relaxed);
        let encoded_start = self.running_since_us.load(Ordering::Relaxed);
        if encoded_start == 0 {
            total
        } else {
            total.saturating_add(now_us.saturating_sub(encoded_start - 1))
        }
    }
}

static CPU_STATS: InitedCell<Vec<CpuSchedDebug>> = InitedCell::uninit();

pub(crate) fn init() {
    CPU_STATS.init((0..arch::cpu_count()).map(|_| CpuSchedDebug::new()).collect());
}

pub(crate) fn start_running(cpu_id: usize, now_us: u64) {
    let Some(stats) = CPU_STATS.try_get().and_then(|stats| stats.get(cpu_id)) else {
        return;
    };
    let old = stats.running_since_us.swap(now_us.saturating_add(1), Ordering::Relaxed);
    debug_assert_eq!(old, 0);
}

pub(crate) fn finish_running(cpu_id: usize, now_us: u64) {
    let Some(stats) = CPU_STATS.try_get().and_then(|stats| stats.get(cpu_id)) else {
        return;
    };
    let encoded_start = stats.running_since_us.swap(0, Ordering::Relaxed);
    if encoded_start != 0 {
        stats
            .running_us
            .fetch_add(now_us.saturating_sub(encoded_start - 1), Ordering::Relaxed);
    }
}

#[cfg(feature = "scheduler-block-reason-debug")]
pub(crate) fn finish_block(stamp: Option<DebugBlockStamp>) {
    let Some(stamp) = stamp else {
        return;
    };
    let Some(stats) = CPU_STATS.try_get().and_then(|stats| stats.get(stamp.cpu_id)) else {
        return;
    };
    stats
        .blocked
        .lock()
        .entry(stamp.name)
        .or_default()
        .add(arch::get_time_us().saturating_sub(stamp.start_us));
}

pub(crate) fn dump() {
    let Some(stats) = CPU_STATS.try_get() else {
        println!("scheduler time debug: statistics are not initialized");
        return;
    };
    let now_us = arch::get_time_us();

    println!("========== scheduler time debug ==========");
    for (cpu_id, cpu) in stats.iter().enumerate() {
        println!("cpu{}:", cpu_id);
        println!("  running                 {} us", cpu.running_snapshot(now_us));
        #[cfg(feature = "scheduler-block-reason-debug")]
        {
            for (name, counter) in cpu.blocked.lock().iter() {
                let (total_us, count) = counter.snapshot();
                println!("  blocked {:<28} {} us  count={}", name, total_us, count);
            }
        }
    }
    println!("==========================================");
    #[cfg(feature = "map-manager-lock-debug")]
    crate::kernel::mm::maparea::dump_lock_debug_stats();
}
// PERF_DEBUG_END(scheduler-time)

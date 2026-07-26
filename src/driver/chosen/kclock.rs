use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;

use alloc::sync::Arc;

use crate::arch;
use crate::driver::RTCDriverOps;
use crate::kernel::errno::{Errno, SysResult};
use crate::klib::SpinLock;

static KCLOCK: SpinLock<Option<Arc<dyn RTCDriverOps>>> = SpinLock::new(None, "static::KCLOCK");

/// Nanoseconds added to the monotonic clock to obtain REALTIME. Zero means
/// "not derived from the RTC yet": the first `now()` after an RTC registers
/// reads the hardware once and caches the offset, so steady-state REALTIME
/// reads take no lock and touch no MMIO.
static REALTIME_OFFSET_NS: AtomicU64 = AtomicU64::new(0);

fn monotonic_ns() -> u64 {
    arch::get_time_us().saturating_mul(1_000)
}

fn duration_to_ns(time: Duration) -> u64 {
    time.as_nanos().min(u64::MAX as u128) as u64
}

pub fn register(clock: Arc<dyn RTCDriverOps>) {
    *KCLOCK.lock() = Some(clock);
}

pub fn now() -> SysResult<Duration> {
    let offset = REALTIME_OFFSET_NS.load(Ordering::Relaxed);
    if offset != 0 {
        return Ok(Duration::from_nanos(offset.saturating_add(monotonic_ns())));
    }

    init_offset_from_rtc()
}

fn init_offset_from_rtc() -> SysResult<Duration> {
    let Some(clock) = (*KCLOCK.lock()).clone() else {
        // No RTC: report the monotonic clock, and do not cache an offset so a
        // late-registered RTC still takes effect on the next read.
        return Ok(arch::uptime());
    };

    let rtc_ns = duration_to_ns(clock.now()?);
    let offset = rtc_ns.saturating_sub(monotonic_ns()).max(1);
    // The first writer wins so every CPU shares one offset; a lost race only
    // discards a value computed nanoseconds apart from the stored one.
    let offset = match REALTIME_OFFSET_NS.compare_exchange(0, offset, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => offset,
        Err(existing) => existing,
    };
    Ok(Duration::from_nanos(offset.saturating_add(monotonic_ns())))
}

pub fn set_time(time: Duration) -> SysResult<()> {
    let Some(clock) = (*KCLOCK.lock()).clone() else {
        return Err(Errno::ENODEV);
    };

    clock.set_time(time)?;
    let offset = duration_to_ns(time).saturating_sub(monotonic_ns()).max(1);
    REALTIME_OFFSET_NS.store(offset, Ordering::Relaxed);
    Ok(())
}

use core::time::Duration;

use alloc::sync::Arc;

use crate::arch;
use crate::driver::RTCDriverOps;
use crate::kernel::errno::SysResult;
use crate::klib::SpinLock;

static KCLOCK: SpinLock<Option<Arc<dyn RTCDriverOps>>> = SpinLock::new(None);

pub fn register(clock: Arc<dyn RTCDriverOps>) {
    *KCLOCK.lock() = Some(clock);
}

pub fn now() -> SysResult<Duration> {
    if let Some(clock) = &*KCLOCK.lock() {
        clock.now()
    } else {
        Ok(arch::uptime())
    }
}

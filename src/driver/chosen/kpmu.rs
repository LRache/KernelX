use alloc::sync::Arc;

use crate::klib::RWLock;

pub trait KPMU: Send + Sync {
    fn shutdown(&self) -> !;
}

static KPMU_DRIVER: RWLock<Option<Arc<dyn KPMU>>> = RWLock::new(None, "static::KPMU_DRIVER");

pub fn register(kpmu: Arc<dyn KPMU>) {
    *KPMU_DRIVER.write() = Some(kpmu);
}

pub fn shutdown() -> ! {
    if let Some(kpmu) = KPMU_DRIVER.read().as_ref() {
        kpmu.shutdown();
    }
    loop {}
}

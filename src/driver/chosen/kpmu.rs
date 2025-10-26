pub trait KPMU : Send + Sync {
    fn shutdown(&self) -> !;
}

struct EmptyKPMU;

impl KPMU for EmptyKPMU {
    fn shutdown(&self) -> ! {
        loop {}
    }
}

static KPMU_DRIVER: spin::RwLock<&'static dyn KPMU> = spin::RwLock::new(&EmptyKPMU);

pub fn register(kpmu: &'static dyn KPMU) {
    *KPMU_DRIVER.write() = kpmu;
}

pub fn shutdown() -> ! {
    KPMU_DRIVER.read().shutdown()
}

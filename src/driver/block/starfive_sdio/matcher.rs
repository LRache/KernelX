use core::sync::atomic::{AtomicI32, Ordering};
use alloc::sync::Arc;

use crate::driver::{Device, DriverMatcher, DriverOps};
use crate::arch::map_kernel_addr;
use crate::kernel::mm::MapPerm; 
use crate::{kinfo, kwarn};

use super::driver::Driver;

pub struct Matcher {
    count: AtomicI32,
}

impl Matcher {
    pub const fn new() -> Self {
        Matcher {
            count: AtomicI32::new(0),
        }
    }
}

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        if device.compatible() == "snps,dw-mshc" {
            map_kernel_addr(device.mmio_base(), device.mmio_base(), device.mmio_size(), MapPerm::RW);
            let driver = Driver::new(self.count.fetch_add(1, Ordering::Relaxed), device.mmio_base());
            kinfo!("Matched starfive_sdio driver for device: {:?}", device);
            let r = driver.init();
            if let Err(e) = r {
                kwarn!("Failed to init starfive_sdio driver: {:?}", e);
                None
            } else {
                Some(Arc::new(driver))
            }
        } else {
            None
        }
    }
}

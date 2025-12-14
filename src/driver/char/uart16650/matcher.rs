use alloc::sync::Arc;

use crate::driver::char::uart16650::driver::Driver;
use crate::driver::matcher::DriverMatcher;
use crate::driver::{Device, DriverOps};
use crate::arch;
use crate::kernel::mm::{MapPerm, page};

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        if device.compatible() == "ns16550a" {
            let size = device.mmio_size();
            let kbase = page::alloc_contiguous(arch::page_count(size));
            arch::map_kernel_addr(kbase, device.mmio_base(), size, MapPerm::RW);
            
            let driver = Driver::new(kbase, device.name().into());
            driver.init();
            
            Some(Arc::new(driver))
        } else {
            None
        }
    }
}

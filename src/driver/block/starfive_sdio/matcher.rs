use alloc::sync::Arc;

use crate::driver::{Device, DriverMatcher, DriverOps};
use crate::arch::{self, map_kernel_addr};
use crate::kernel::mm::MapPerm; 
use crate::kwarn;

use super::driver::Driver;

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        if device.compatible() != "snps,dw-mshc" {
            return None;
        }
            
        let pages = arch::page_count(device.mmio_size());
        map_kernel_addr(pages, device.mmio_base(), device.mmio_size(), MapPerm::RW);
        
        let driver = Driver::new(device.name().into(), device.mmio_base());
        let r = driver.init();
        if let Err(e) = r {
            kwarn!("Failed to init starfive_sdio driver: {:?}", e);
            None
        } else {
            Some(Arc::new(driver))
        }
    }
}

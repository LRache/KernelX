use alloc::sync::Arc;

use crate::driver::{Device, DriverMatcher, DriverOps};
use crate::arch::{self, map_kernel_addr};
use crate::kernel::mm::{MapPerm, page}; 
use crate::kwarn;

use super::driver::Driver;

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(&["snps,dw-mshc"])?;
        
        let (mmio_base, mmio_size) = device.mmio()?;
        if mmio_base != 0x16020000 {
            return None;
        }

        let pages = arch::page_count(mmio_size);
        let kpage = page::alloc_contiguous(pages);
        map_kernel_addr(kpage, mmio_base, mmio_size, MapPerm::RW);
        
        let driver = Driver::new(device.name().into(), kpage);
        let r = driver.init();
        if let Err(e) = r {
            kwarn!("Failed to init starfive_sdio driver: {:?}", e);
            None
        } else {
            Some(Arc::new(driver))
        }
    }
}

use core::time::Duration;
use alloc::string::String;
use alloc::sync::Arc;

use crate::arch;
use crate::kernel::errno::SysResult;
use crate::driver::{Device, DeviceType, DriverOps, RTCDriverOps};
use crate::driver::matcher::DriverMatcher;
use crate::kernel::mm::{MapPerm, page};

pub struct Driver {
    base: usize,
    name: String,
}

impl Driver {
    fn new(base: usize, name: String) -> Self {
        Driver { base, name }
    }

    fn read(&self, offset: usize) -> u32 {
        unsafe { arch::read_volatile((self.base + offset) as *mut u32) }
    }
}

impl DriverOps for Driver {
    fn name(&self) -> &str {
        "goldfish-rtc"
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Rtc
    }

    fn as_rtc_driver(self: Arc<Self>) -> Option<Arc<dyn RTCDriverOps>> {
        Some(self)
    }
}

impl RTCDriverOps for Driver {
    fn now(&self) -> SysResult<Duration> {
        let low = self.read(0x0) as u64;
        let high = self.read(0x4) as u64;
        Ok(Duration::from_nanos(high << 32 | low))
    }
}

pub struct Matcher;

impl DriverMatcher for Matcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(&["google,goldfish-rtc"])?;

        let (mmio_base, mmio_size) = device.mmio()?;
        let kbase = page::alloc_contiguous(arch::page_count(mmio_size));
        arch::map_kernel_addr(kbase, mmio_base, mmio_size, MapPerm::RW);
        Some(Arc::new(Driver::new(kbase, device.name().into())))
    }
}

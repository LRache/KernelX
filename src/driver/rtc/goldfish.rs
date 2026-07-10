use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use crate::arch;
use crate::driver::{Device, DeviceType, DriverOps, MMIOMatcher as MMIOMatcherTrait, RTCDriverOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::{MapPerm, page};
use crate::klib::SpinLock;

const NANOS_PER_SEC: i128 = 1_000_000_000;

#[repr(usize)]
enum Register {
    TimeLow = 0x0,
    TimeHigh = 0x4,
}

pub struct Driver {
    base: usize,
    name: String,
    offset: SpinLock<i128>,
}

impl Driver {
    fn new(base: usize, name: String) -> Self {
        Driver {
            base,
            name,
            offset: SpinLock::new(0, "GoldfishRtc::offset"),
        }
    }

    fn read(&self, register: Register) -> u32 {
        unsafe { arch::read_volatile((self.base + register as usize) as *mut u32) }
    }

    fn read_raw_nanos(&self) -> i128 {
        // TIME_LOW latches TIME_HIGH in goldfish RTC, so this read pair must
        // stay ordered and cannot be interleaved with another CPU's RTC read.
        let low = self.read(Register::TimeLow) as u64;
        let high = self.read(Register::TimeHigh) as u64;
        i128::from(high << 32 | low)
    }

    fn duration_to_nanos(time: Duration) -> SysResult<i128> {
        i128::from(time.as_secs())
            .checked_mul(NANOS_PER_SEC)
            .and_then(|nanos| nanos.checked_add(i128::from(time.subsec_nanos())))
            .ok_or(Errno::EINVAL)
    }

    fn nanos_to_duration(nanos: i128) -> SysResult<Duration> {
        if nanos < 0 {
            return Err(Errno::EINVAL);
        }

        let secs = u64::try_from(nanos / NANOS_PER_SEC).map_err(|_| Errno::EINVAL)?;
        let subsec_nanos = (nanos % NANOS_PER_SEC) as u32;
        Ok(Duration::new(secs, subsec_nanos))
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
        let offset = self.offset.lock();
        Self::nanos_to_duration(self.read_raw_nanos() + *offset)
    }

    fn set_time(&self, time: Duration) -> SysResult<()> {
        let mut offset = self.offset.lock();
        *offset = Self::duration_to_nanos(time)? - self.read_raw_nanos();
        Ok(())
    }
}

pub struct MMIOMatcher;

impl MMIOMatcherTrait for MMIOMatcher {
    fn try_match(&self, device: &Device) -> Option<Arc<dyn DriverOps>> {
        device.match_compatible(&["google,goldfish-rtc"])?;

        let (mmio_base, mmio_size) = device.mmio()?;
        let kbase = page::alloc_contiguous(arch::page_count(mmio_size));
        arch::map_kernel_addr(kbase, mmio_base, mmio_size, MapPerm::RW);
        Some(Arc::new(Driver::new(kbase, device.name().into())))
    }
}

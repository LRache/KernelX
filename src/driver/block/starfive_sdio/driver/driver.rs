use core::time::Duration;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::format;
use visionfive2_sd::{Vf2SdDriver, SDIo, SleepOps};

use crate::driver::{BlockDriverOps, DeviceType, DriverOps};
use crate::kernel::event::timer;
use crate::klib::SpinMutex;

struct SDIOImpls {
    pub base: usize
}

impl SDIo for SDIOImpls {
    fn read_reg_at(&self, offset: usize) -> u32 {
        let addr = (self.base + offset) as *const u32;
        unsafe { addr.read_volatile() }
    }

    fn write_reg_at(&mut self, offset: usize, val: u32) {
        let addr = (self.base + offset) as *mut u32;
        unsafe {
            addr.write_volatile(val);
        }
    }

    fn read_data_at(&self, offset: usize) -> u64 {
        let addr = (self.base + offset) as *const u64;
        unsafe { addr.read_volatile() }
    }

    fn write_data_at(&mut self, offset: usize, val: u64) {
        let addr = (self.base + offset) as *mut u64;
        unsafe {
            addr.write_volatile(val);
        }
    }
}

struct SleepOpsImpls;

impl SleepOps for SleepOpsImpls {
    fn sleep_ms(ms: usize) {
        timer::spin_delay(Duration::from_millis(ms as u64));
    }

    fn sleep_ms_until(ms: usize, f: impl FnMut() -> bool) {
        timer::wait_until(Duration::from_millis(ms as u64), f);
    }
}

pub struct Driver {
    base: usize,
    num: i32,
    inner: SpinMutex<Vf2SdDriver<SDIOImpls, SleepOpsImpls>>
}

impl Driver {
    pub fn new(num: i32, base: usize) -> Self {
        let inner = Vf2SdDriver::new(SDIOImpls { base });
        Driver { 
            num, 
            base, 
            inner: SpinMutex::new(inner) 
        }
    }

    pub fn init(&self) -> Result<(), ()> {
        if self.base != 0x16020000 {
            return Err(());
        }

        self.inner.lock().init();

        Ok(())
    }
}

impl DriverOps for Driver {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn name(&self) -> &str {
        "starfive_sdio"
    }

    fn device_name(&self) -> String {
        format!("sdio{}", self.num)
    }

    fn as_block_driver(self: Arc<Self>) -> Arc<dyn BlockDriverOps> {
        self
    }
}

impl BlockDriverOps for Driver {
    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        self.inner.lock().read_block(block, buf);

        Ok(())
    }

    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        self.inner.lock().write_block(block, buf);

        Ok(())
    }

    fn get_block_size(&self) -> u32 {
        512
    }

    fn get_block_count(&self) -> u64 {
        0
    }
}

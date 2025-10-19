use alloc::sync::Arc;
use alloc::boxed::Box;
use spin::Mutex;

use crate::driver::block::BlockDriver;
use crate::driver::{DriverOps, DeviceType};

use super::EMMCDeviceInner;

pub struct EMMCDriver {
    inner: Arc<Mutex<EMMCDeviceInner>>,
}

impl EMMCDriver {
    pub fn new(inner: &Arc<Mutex<EMMCDeviceInner>>) -> Self {
        Self { 
            inner: inner.clone(),
        } 
    }
}

impl DriverOps for EMMCDriver {
    fn name(&self) -> &str {
        "emmc_driver"
    }

    fn device_type(&self) -> DeviceType {
        crate::driver::DeviceType::Block
    }
}

impl BlockDriver for EMMCDriver {
    fn clone_boxed(&self) -> Box<dyn BlockDriver> {
        Box::new(EMMCDriver::new(&self.inner))
    }

    fn read_block(&self, _block: usize, _buf: &mut [u8]) -> Result<(), ()> {
        unimplemented!()
    }

    fn write_block(&self, _block: usize, _buf: &[u8]) -> Result<(), ()> {
        unimplemented!()
    }

    fn flush(&self) -> Result<(), ()> {
        Ok(())
    }

    fn get_block_size(&self) -> u32 {
        512
    }

    fn get_block_count(&self) -> u64 {
        0
    }
}
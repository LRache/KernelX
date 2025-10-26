use alloc::string::String;
use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;

use crate::driver::block::{BlockDevice, BlockDriver};

use super::driver::EMMCDriver;

const BLOCK_SIZE: usize = 512;

pub struct EMMCDeviceInner {
    base: usize,
}

impl EMMCDeviceInner {
    pub fn new(base: usize) -> Self {
        Self { base }
    }

    pub fn read_block(&mut self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        assert!(buf.len() == 512);
        Ok(())
    }
}

pub struct EMMCDevice {
    inner: Arc<Mutex<EMMCDeviceInner>>,
}

impl BlockDevice for EMMCDevice {
    fn name(&self) -> &str {
        "emmc"
    }

    fn driver(&self) -> Box<dyn BlockDriver> {
        Box::new(EMMCDriver::new(&self.inner))
    }
}

use alloc::sync::Arc;
use alloc::boxed::Box;
use spin::Mutex;

use crate::driver::block::BlockDriver;

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

impl BlockDriver for EMMCDriver {
    fn clone_boxed(&self) -> Box<dyn BlockDriver> {
        Box::new(EMMCDriver::new(&self.inner))
    }

    fn read_block(&mut self, _block: usize, _buf: &mut [u8]) -> Result<(), ()> {
        unimplemented!()
    }

    fn write_block(&mut self, _block: usize, _buf: &[u8]) -> Result<(), ()> {
        unimplemented!()
    }

    fn flush(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn get_block_size(&self) -> u32 {
        512
    }

    fn get_block_count(&self) -> u64 {
        0
    }
}
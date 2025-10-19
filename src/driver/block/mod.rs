mod virtio;
mod starfive_mmc;

use alloc::string::String;
use alloc::boxed::Box;

pub use virtio::*;
pub use starfive_mmc::*;

use crate::driver::DriverOps;

use downcast_rs::{impl_downcast, Downcast};

pub trait BlockDevice {
    fn name(&self) -> &str;
    fn driver(&self) -> Box<dyn BlockDriver>;
}

pub trait BlockDriver: DriverOps + Downcast {
    fn clone_boxed(&self) -> Box<dyn BlockDriver>;
    
    fn open(&mut self) -> Result<(), ()> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()>;
    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()>;

    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<(), ()> {
        unimplemented!()
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<(), ()> {
        unimplemented!()
    }

    fn flush(&self) -> Result<(), ()>;

    fn get_block_size(&self) -> u32;

    fn get_block_count(&self) -> u64;
}

impl_downcast!(BlockDriver);

mod virtio;
mod starfive_mmc;

use alloc::string::String;
use alloc::boxed::Box;

pub use virtio::*;
pub use starfive_mmc::*;

pub trait BlockDevice {
    fn name(&self) -> String;
    fn driver(&self) -> Box<dyn BlockDriver>;
}

pub trait BlockDriver {
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

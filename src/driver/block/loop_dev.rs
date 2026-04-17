use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::{BlockDriverOps, DeviceType, DriverOps};
use crate::fs::InodeOps;

const BLOCK_SIZE: u32 = 512;

pub struct LoopDevice {
    inode: Arc<dyn InodeOps>,
}

impl LoopDevice {
    pub fn new(inode: Arc<dyn InodeOps>) -> Arc<Self> {
        Arc::new(Self { inode })
    }
}

impl DriverOps for LoopDevice {
    fn name(&self) -> &str {
        "loop"
    }

    fn device_name(&self) -> String {
        String::from("loop")
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn as_block_driver(self: Arc<Self>) -> Option<Arc<dyn BlockDriverOps>> {
        Some(self)
    }
}

impl BlockDriverOps for LoopDevice {
    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        let offset = block * BLOCK_SIZE as usize;
        let n = self.inode.readat(buf, offset, false).map_err(|_| ())?;
        // Zero-fill if we read less than a full block
        if n < buf.len() {
            buf[n..].fill(0);
        }
        Ok(())
    }

    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        let offset = block * BLOCK_SIZE as usize;
        self.inode.writeat(buf, offset).map_err(|_| ())?;
        Ok(())
    }

    fn get_block_size(&self) -> u32 {
        BLOCK_SIZE
    }

    fn get_block_count(&self) -> u64 {
        self.inode.size().unwrap_or(0) / BLOCK_SIZE as u64
    }
}

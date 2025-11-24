use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::MmioTransport;

use crate::driver::BlockDriverOps;
use crate::driver::virtio::VirtIOHal;
use crate::driver::{DeviceType, DriverOps};
use crate::klib::SpinLock;

const BLOCK_SIZE: usize = 512;

pub struct VirtIOBlockDriver {
    num: u32,
    driver: SpinLock<VirtIOBlk<VirtIOHal, MmioTransport>>,
}

impl VirtIOBlockDriver {
    pub fn new(num: u32, transport: MmioTransport) -> Self {
        Self {
            num,
            driver: SpinLock::new(VirtIOBlk::new(transport).unwrap()),
        }
    }
}

impl DriverOps for VirtIOBlockDriver {
    fn name(&self) -> &str {
        "virtio_blk_driver"
    }

    fn device_name(&self) -> String {
        format!("virtio_block{}", self.num)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn as_block_driver(self: Arc<Self>) -> Arc<dyn BlockDriverOps> {
        self
    }
}

impl BlockDriverOps for VirtIOBlockDriver {
    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        self.driver.lock().read_blocks(block, buf).map_err(|_| ())
    }

    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        self.driver.lock().write_blocks(block, buf).map_err(|_| ())
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        let mut length = buf.len();
        let mut block = offset / BLOCK_SIZE;

        let mut block_buf = [0u8; BLOCK_SIZE];
        let mut buf_offset = 0;

        let block_offset = offset % BLOCK_SIZE;
        if block_offset != 0 {
            self.read_block(block, &mut block_buf)?;

            let read_size = core::cmp::min(BLOCK_SIZE - block_offset, length);
            buf[buf_offset..buf_offset + read_size]
                .copy_from_slice(&block_buf[block_offset..block_offset + read_size]);

            buf_offset += read_size;
            length -= read_size;
            block += 1;
        }

        while length != 0 {
            self.read_block(block, &mut block_buf)?;

            let read_size = core::cmp::min(length, BLOCK_SIZE);
            buf[buf_offset..buf_offset + read_size].copy_from_slice(&block_buf[..read_size]);

            buf_offset += read_size;
            length -= read_size;
            block += 1;
        }

        Ok(())
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<(), ()> {
        let mut length = buf.len();
        let mut block = offset / BLOCK_SIZE;

        let mut block_buf = [0u8; BLOCK_SIZE];
        let mut buf_offset = 0;

        let block_offset = offset % BLOCK_SIZE;
        if block_offset != 0 {
            self.read_block(block, &mut block_buf)?;

            let write_size = core::cmp::min(BLOCK_SIZE - block_offset, length);
            block_buf[block_offset..block_offset + write_size]
                .copy_from_slice(&buf[buf_offset..buf_offset + write_size]);
            self.write_block(block, &block_buf)?;

            buf_offset += write_size;
            length -= write_size;
            block += 1;
        }

        while length != 0 {
            self.read_block(block, &mut block_buf)?;

            let write_size = core::cmp::min(length, BLOCK_SIZE);
            block_buf[..write_size].copy_from_slice(&buf[buf_offset..buf_offset + write_size]);
            self.write_block(block, &block_buf)?;

            buf_offset += write_size;
            length -= write_size;
            block += 1;
        }

        Ok(())
    }

    fn flush(&self) -> Result<(), ()> {
        Ok(())
    }

    fn get_block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn get_block_count(&self) -> u64 {
        self.driver.lock().capacity()
    }
}

use core::ptr::NonNull;
use alloc::boxed::Box;
use alloc::string::String;
use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::mmio::{VirtIOHeader, MmioTransport};

use crate::driver::block::{BlockDevice, BlockDriver};
use crate::driver::virtio::VirtIOHal;

const BLOCK_SIZE: usize = 512;

pub struct VirtIOBlockDevice {
    addr: usize
}

impl VirtIOBlockDevice {
    pub fn new(addr: usize) -> Self {
        Self { addr }
    }
}

impl BlockDevice for VirtIOBlockDevice {
    fn name(&self) -> String {
        "virtio_blk".into()
    }

    fn driver(&self) -> Box<dyn BlockDriver> {
        Box::new(VirtIOBlockDriver::new(self.addr))
    }
}

pub struct VirtIOBlockDriver {
    driver: VirtIOBlk<VirtIOHal, MmioTransport>,
}

impl VirtIOBlockDriver {
    pub fn new(addr: usize) -> Self {
        let addr = addr as *mut usize;
        let transport = unsafe {
            MmioTransport::new(NonNull::new(addr as *mut VirtIOHeader).unwrap())
                .expect("Failed to create MMIO transport for VirtIO block driver")
        };
        Self {
            driver: VirtIOBlk::new(transport).unwrap()
        }
    }
}

impl BlockDriver for VirtIOBlockDriver {
    fn read_block(&mut self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        self.driver.read_blocks(block, buf).map_err(|_| ())
    }

    fn write_block(&mut self, block: usize, buf: &[u8]) -> Result<(), ()> {
        self.driver.write_blocks(block, buf).map_err(|_| ())
    }

    // fn read_at(&mut self, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
    //     let mut length = buf.len();
    //     let mut block = offset / BLOCK_SIZE;

    //     let mut block_buf = [0u8; BLOCK_SIZE];
    //     let mut buf_offset = 0;

    //     let block_offset = offset % BLOCK_SIZE;
    //     if block_offset != 0 {
    //         self.read_block(block, &mut block_buf)?;
            
    //         let read_size = core::cmp::min(BLOCK_SIZE - block_offset, length);
    //         buf[buf_offset..buf_offset + read_size].copy_from_slice(&block_buf[block_offset..block_offset + read_size]);
            
    //         buf_offset += read_size;
    //         length -= read_size;
    //         block += 1;
    //     }

    //     while length != 0 {
    //         self.read_block(block, &mut block_buf)?;
            
    //         let read_size = core::cmp::min(length, BLOCK_SIZE);
    //         buf[buf_offset..buf_offset + read_size].copy_from_slice(&block_buf[..read_size]);
            
    //         buf_offset += read_size;
    //         length -= read_size;
    //         block += 1;
    //     }

    //     Ok(())
    // }

    // fn write_at(&mut self, offset: usize, buf: &[u8]) -> Result<(), ()> {
    //     let mut length = buf.len();
    //     let mut block = offset / BLOCK_SIZE;
  
    //     let mut block_buf = [0u8; BLOCK_SIZE];
    //     let mut buf_offset = 0;

    //     let block_offset = offset % BLOCK_SIZE;
    //     if block_offset != 0 {
    //         self.read_block(block, &mut block_buf)?;

    //         let write_size = core::cmp::min(BLOCK_SIZE - block_offset, length);
    //         block_buf[block_offset..block_offset + write_size].copy_from_slice(&buf[buf_offset..buf_offset + write_size]);
    //         self.write_block(block, &block_buf)?;

    //         buf_offset += write_size;
    //         length -= write_size;
    //         block += 1;
    //     }

    //     while length != 0 {
    //         self.read_block(block, &mut block_buf)?;

    //         let write_size = core::cmp::min(length, BLOCK_SIZE);
    //         block_buf[..write_size].copy_from_slice(&buf[buf_offset..buf_offset + write_size]);
    //         self.write_block(block, &block_buf)?;

    //         buf_offset += write_size;
    //         length -= write_size;
    //         block += 1;
    //     }

    //     Ok(())
    // }

    fn flush(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn get_block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn get_block_count(&self) -> u64 {
        self.driver.capacity()
    }
}

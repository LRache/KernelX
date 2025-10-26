use alloc::sync::Arc;
use alloc::string::String;

use crate::kernel::errno::SysResult;
use crate::kernel::event::{PollEvent, PollEventSet};

use super::DeviceType;

pub trait DriverOps {
    fn name(&self) -> &str;

    fn device_name(&self) -> String;
    fn device_type(&self) -> DeviceType;

    fn as_block_driver(self: Arc<Self>) -> Arc<dyn BlockDriverOps> {
        unreachable!()
    }

    fn as_char_driver(self: Arc<Self>) -> Arc<dyn CharDriverOps> {
        unreachable!()
    }
}

use downcast_rs::{impl_downcast, Downcast};

pub trait BlockDriverOps: DriverOps + Downcast {
    fn open(&mut self) -> Result<(), ()> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ()> {
        Ok(())
    }

    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()>;
    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()>;

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        let block_size = self.get_block_size() as usize;
        debug_assert!(block_size <= 512);
        
        let mut length = buf.len();
        let mut block = offset / block_size;

        let mut block_buf = [0u8; 512];
        let mut buf_offset = 0;

        let block_offset = offset % block_size;
        if block_offset != 0 {
            self.read_block(block, &mut block_buf[..block_size])?;

            let read_size = core::cmp::min(block_size - block_offset, length);
            buf[buf_offset..buf_offset + read_size].copy_from_slice(&block_buf[block_offset..block_offset + read_size]);
            
            buf_offset += read_size;
            length -= read_size;
            block += 1;
        }

        while length != 0 {
            self.read_block(block, &mut block_buf)?;
            
            let read_size = core::cmp::min(length, block_size);
            buf[buf_offset..buf_offset + read_size].copy_from_slice(&block_buf[..read_size]);
            
            buf_offset += read_size;
            length -= read_size;
            block += 1;
        }

        Ok(())
    }
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<(), ()> {
        let block_size = self.get_block_size() as usize;
        debug_assert!(block_size <= 512);
        
        let mut length = buf.len();
        let mut block = offset / block_size;

        let mut block_buf = [0u8; 512];
        let mut buf_offset = 0;

        let block_offset = offset % block_size;
        if block_offset != 0 {
            self.read_block(block, &mut block_buf)?;

            let write_size = core::cmp::min(block_size - block_offset, length);
            block_buf[block_offset..block_offset + write_size].copy_from_slice(&buf[buf_offset..buf_offset + write_size]);
            self.write_block(block, &block_buf)?;

            buf_offset += write_size;
            length -= write_size;
            block += 1;
        }

        while length != 0 {
            self.read_block(block, &mut block_buf)?;

            let write_size = core::cmp::min(length, block_size);
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

    fn get_block_size(&self) -> u32;

    fn get_block_count(&self) -> u64;
}

impl_downcast!(BlockDriverOps);

pub trait CharDriverOps: DriverOps + Downcast{
    fn putchar(&self, c: u8);
    fn getchar(&self) -> Option<u8>;
    fn poll(&self, waker: usize, event: PollEventSet) -> SysResult<Option<PollEvent>>;
    fn poll_cancel(&self);
}

impl_downcast!(CharDriverOps);

pub trait PMUDriverOps : Sync + Send {
    fn shutdown(&self) -> !;
}

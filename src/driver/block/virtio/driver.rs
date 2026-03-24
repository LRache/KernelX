use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::string::String;
use virtio_drivers::device::blk::{VirtIOBlk, BlkReq, BlkResp, RespStatus};
use virtio_drivers::transport::mmio::MmioTransport;

use crate::driver::BlockDriverOps;
use crate::driver::{DeviceType, DriverOps};
use crate::driver::virtio::VirtIOHal;
use crate::kernel::event::Event;
use crate::kernel::scheduler::{self, current, Task};
use crate::klib::SpinLock;

const BLOCK_SIZE: usize = 512;

pub struct VirtIOBlockDriver {
    device_name: String,
    driver: SpinLock<VirtIOBlk<VirtIOHal, MmioTransport>>,
    inflight: SpinLock<BTreeMap<u16, Arc<dyn Task>>>,
}

impl VirtIOBlockDriver {
    pub fn new(device_name: String, transport: MmioTransport) -> Self {
        let mut blk = VirtIOBlk::new(transport).unwrap();
        blk.enable_interrupts();
        Self {
            device_name,
            driver: SpinLock::new(blk),
            inflight: SpinLock::new(BTreeMap::new()),
        }
    }

    fn wait_for_token(&self, token: u16) {
        let task = current::task().clone();
        task.block_uninterruptible("virtio_blk_io");
        self.inflight.lock().insert(token, task);
        current::schedule();
    }

    /// 任务完成 complete_* 消费掉 used ring 队头后，检查下一个是否也完成并唤醒
    fn wake_next(&self) {
        let mut driver = self.driver.lock();
        if let Some(token) = driver.peek_used() {
            if let Some(task) = self.inflight.lock().remove(&token) {
                scheduler::wakeup_task_uninterruptible(task, Event::IOComplete);
            }
        }
    }
}

impl DriverOps for VirtIOBlockDriver {
    fn name(&self) -> &str {
        "virtio_blk_driver"
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn as_block_driver(self: Arc<Self>) -> Option<Arc<dyn BlockDriverOps>> {
        Some(self)
    }

    fn handle_interrupt(&self) {
        let mut driver = self.driver.lock();
        driver.ack_interrupt();
        let mut inflight = self.inflight.lock();
        if let Some(token) = driver.peek_used() {
            if let Some(task) = inflight.remove(&token) {
                scheduler::wakeup_task_uninterruptible(task, Event::IOComplete);
            }
        }
    }
}

impl BlockDriverOps for VirtIOBlockDriver {
    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = {
            let mut driver = self.driver.lock();
            unsafe { driver.read_blocks_nb(block, &mut req, buf, &mut resp) }
                .map_err(|_| ())?
        };

        self.wait_for_token(token);

        {
            let mut driver = self.driver.lock();
            unsafe { driver.complete_read_blocks(token, &req, buf, &mut resp) }
                .map_err(|_| ())?;
        }
        self.wake_next();

        if resp.status() == RespStatus::OK {
            Ok(())
        } else {
            Err(())
        }
    }

    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = {
            let mut driver = self.driver.lock();
            unsafe { driver.write_blocks_nb(block, &mut req, buf, &mut resp) }
                .map_err(|_| ())?
        };

        self.wait_for_token(token);

        {
            let mut driver = self.driver.lock();
            unsafe { driver.complete_write_blocks(token, &req, buf, &mut resp) }
                .map_err(|_| ())?;
        }
        self.wake_next();

        if resp.status() == RespStatus::OK {
            Ok(())
        } else {
            Err(())
        }
    }

    fn read_blocks(&self, start_block: usize, buf: &mut [u8]) -> Result<(), ()> {
        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = {
            let mut driver = self.driver.lock();
            unsafe { driver.read_blocks_nb(start_block, &mut req, buf, &mut resp) }
                .map_err(|_| ())?
        };

        self.wait_for_token(token);

        {
            let mut driver = self.driver.lock();
            unsafe { driver.complete_read_blocks(token, &req, buf, &mut resp) }
                .map_err(|_| ())?;
        }
        self.wake_next();

        if resp.status() == RespStatus::OK {
            Ok(())
        } else {
            Err(())
        }
    }

    fn write_blocks(&self, start_block: usize, buf: &[u8]) -> Result<(), ()> {
        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = {
            let mut driver = self.driver.lock();
            unsafe { driver.write_blocks_nb(start_block, &mut req, buf, &mut resp) }
                .map_err(|_| ())?
        };

        self.wait_for_token(token);

        {
            let mut driver = self.driver.lock();
            unsafe { driver.complete_write_blocks(token, &req, buf, &mut resp) }
                .map_err(|_| ())?;
        }
        self.wake_next();

        if resp.status() == RespStatus::OK {
            Ok(())
        } else {
            Err(())
        }
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
            buf[buf_offset..buf_offset + read_size].copy_from_slice(&block_buf[block_offset..block_offset + read_size]);

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
            block_buf[block_offset..block_offset + write_size].copy_from_slice(&buf[buf_offset..buf_offset + write_size]);
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

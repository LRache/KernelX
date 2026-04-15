use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use core::cmp::min;
use virtio_drivers::device::blk::{BlkReq, BlkResp, RespStatus, VirtIOBlk};
use virtio_drivers::transport::mmio::MmioTransport;

use crate::arch;
use crate::driver::virtio::VirtIOHal;
use crate::driver::{BlockDriverOps, DeviceType, DriverOps};
use crate::kernel::event::Event;
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::scheduler::{self, Task, current};
use crate::klib::SpinLock;

const BLOCK_SIZE: usize = 512;
const CACHE_PAGE_SIZE: usize = arch::PGSIZE;
const CACHE_PAGE_COUNT: usize = 2048;
const BLOCKS_PER_CACHE_PAGE: usize = CACHE_PAGE_SIZE / BLOCK_SIZE;

struct CachePage {
    frame: PhysPageFrame,
    dirty: bool,
    version: u64,
}

struct CacheWriteback {
    page: usize,
    version: u64,
    data: CachePage,
}

impl CachePage {
    fn alloc_zeroed() -> Self {
        Self {
            frame: PhysPageFrame::alloc_zeroed(),
            dirty: false,
            version: 0,
        }
    }

    fn copy(&self) -> Self {
        Self {
            frame: self.frame.copy(),
            dirty: self.dirty,
            version: self.version,
        }
    }

    fn read_range(&self, page_offset: usize, buf: &mut [u8]) {
        self.frame.copy_to_slice(page_offset, buf);
    }

    fn write_range(&mut self, page_offset: usize, buf: &[u8]) {
        self.frame.copy_from_slice(page_offset, buf);
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
    }

    fn as_bytes(&self) -> &mut [u8] {
        self.frame.slice()
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn version(&self) -> u64 {
        self.version
    }

    fn mark_clean(&mut self) {
        self.dirty = false;
    }
}

struct BlockPageCache {
    pages: BTreeMap<usize, CachePage>,
    lru: VecDeque<usize>,
}

impl BlockPageCache {
    fn new() -> Self {
        Self {
            pages: BTreeMap::new(),
            lru: VecDeque::new(),
        }
    }

    fn touch(&mut self, page: usize) {
        if let Some(pos) = self.lru.iter().position(|cached| *cached == page) {
            self.lru.remove(pos);
        }
        self.lru.push_front(page);
    }

    fn lru_page(&self) -> Option<usize> {
        self.lru.back().copied()
    }

    fn read_range(&mut self, page: usize, page_offset: usize, buf: &mut [u8]) -> bool {
        if !self.pages.contains_key(&page) {
            return false;
        }

        self.touch(page);
        let cached = self.pages.get(&page).expect("cache entry must exist after touch");
        cached.read_range(page_offset, buf);
        true
    }

    fn clone_page(&mut self, page: usize) -> Option<CachePage> {
        if !self.pages.contains_key(&page) {
            return None;
        }

        self.touch(page);
        let cached = self.pages.get(&page).expect("cache entry must exist after touch");
        Some(cached.copy())
    }

    fn remove(&mut self, page: usize) -> Option<CachePage> {
        if let Some(pos) = self.lru.iter().position(|cached| *cached == page) {
            self.lru.remove(pos);
        }
        self.pages.remove(&page)
    }

    fn snapshot_writeback(&self, page: usize) -> Option<CacheWriteback> {
        let cached = self.pages.get(&page)?;
        if !cached.is_dirty() {
            return None;
        }

        Some(CacheWriteback {
            page,
            version: cached.version(),
            data: cached.copy(),
        })
    }

    fn mark_clean_if_version(&mut self, page: usize, version: u64) -> bool {
        if let Some(cached) = self.pages.get_mut(&page) {
            if cached.version() == version {
                cached.mark_clean();
                return true;
            }
        }
        false
    }
}

pub struct VirtIOBlockDriver {
    device_name: String,
    driver: SpinLock<VirtIOBlk<VirtIOHal, MmioTransport>>,
    inflight: SpinLock<BTreeMap<u16, Arc<dyn Task>>>,
    cache: SpinLock<BlockPageCache>,
}

impl VirtIOBlockDriver {
    pub fn new(device_name: String, transport: MmioTransport) -> Self {
        debug_assert_eq!(CACHE_PAGE_SIZE % BLOCK_SIZE, 0);
        let mut blk = VirtIOBlk::new(transport).unwrap();
        blk.enable_interrupts();
        Self {
            device_name,
            driver: SpinLock::new(blk, "VirtIOBlockDriver::driver"),
            inflight: SpinLock::new(BTreeMap::new(), "VirtIOBlockDriver::inflight"),
            cache: SpinLock::new(BlockPageCache::new(), "VirtIOBlockDriver::cache"),
        }
    }

    fn wait_for_token(&self, token: u16) {
        let task = current::task().clone();
        task.block_uninterruptible("virtio_blk_io");

        // Disable interrupts to make the following two steps atomic:
        //   1. Register ourselves in inflight so the interrupt handler can find us.
        //   2. Check whether the I/O already completed before we registered.
        //
        // Without this, the interrupt can fire between steps 1 and 2 (or before
        // step 1), see an empty inflight map, and skip the wakeup — leaving the
        // task blocked forever (lost-wakeup race).
        self.inflight.lock().insert(token, task.clone());

        // If the interrupt fired before we inserted (inflight was empty at that
        // point), the completion token is still sitting in the used ring.
        // Detect this and self-wake so schedule() returns promptly.
        if self.driver.lock().peek_used() == Some(token) {
            if let Some(t) = self.inflight.lock().remove(&token) {
                scheduler::wakeup_task_uninterruptible(t, Event::IOComplete);
            }
        }

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

    fn raw_read_blocks(&self, start_block: usize, buf: &mut [u8]) -> Result<(), ()> {
        if buf.is_empty() {
            return Ok(());
        }
        debug_assert_eq!(buf.len() % BLOCK_SIZE, 0);

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = {
            let mut driver = self.driver.lock();
            unsafe { driver.read_blocks_nb(start_block, &mut req, buf, &mut resp) }.map_err(|_| ())?
        };

        self.wait_for_token(token);

        {
            let mut driver = self.driver.lock();
            unsafe { driver.complete_read_blocks(token, &req, buf, &mut resp) }.map_err(|_| ())?;
        }
        self.wake_next();

        if resp.status() == RespStatus::OK {
            Ok(())
        } else {
            Err(())
        }
    }

    fn raw_write_blocks(&self, start_block: usize, buf: &[u8]) -> Result<(), ()> {
        if buf.is_empty() {
            return Ok(());
        }
        debug_assert_eq!(buf.len() % BLOCK_SIZE, 0);

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = {
            let mut driver = self.driver.lock();
            unsafe { driver.write_blocks_nb(start_block, &mut req, buf, &mut resp) }.map_err(|_| ())?
        };

        self.wait_for_token(token);

        {
            let mut driver = self.driver.lock();
            unsafe { driver.complete_write_blocks(token, &req, buf, &mut resp) }.map_err(|_| ())?;
        }
        self.wake_next();

        if resp.status() == RespStatus::OK {
            Ok(())
        } else {
            Err(())
        }
    }

    fn load_cache_page(&self, page: usize) -> Result<CachePage, ()> {
        let data = CachePage::alloc_zeroed();
        self.raw_read_blocks(page * BLOCKS_PER_CACHE_PAGE, data.as_bytes())?;
        Ok(data)
    }

    fn write_back_snapshot(&self, writeback: &CacheWriteback) -> Result<(), ()> {
        self.raw_write_blocks(writeback.page * BLOCKS_PER_CACHE_PAGE, writeback.data.as_bytes())
    }

    fn evict_one_cache_page(&self) -> Result<(), ()> {
        loop {
            let writeback = {
                let mut cache = self.cache.lock();
                let Some(page) = cache.lru_page() else {
                    return Ok(());
                };

                if let Some(writeback) = cache.snapshot_writeback(page) {
                    writeback
                } else {
                    cache.remove(page);
                    return Ok(());
                }
            };

            self.write_back_snapshot(&writeback)?;

            let mut cache = self.cache.lock();
            if cache.mark_clean_if_version(writeback.page, writeback.version) {
                cache.remove(writeback.page);
                return Ok(());
            }
        }
    }

    fn cache_page_if_absent(&self, page: usize, data: CachePage) -> Result<(), ()> {
        let mut data = Some(data);

        loop {
            let mut cache = self.cache.lock();
            if cache.pages.contains_key(&page) {
                cache.touch(page);
                return Ok(());
            }

            if cache.pages.len() < CACHE_PAGE_COUNT {
                cache.pages.insert(page, data.take().expect("cache page should exist"));
                cache.touch(page);
                return Ok(());
            }

            drop(cache);
            self.evict_one_cache_page()?;
        }
    }

    fn upsert_cache_page(&self, page: usize, data: CachePage) -> Result<(), ()> {
        let mut data = Some(data);

        loop {
            let mut cache = self.cache.lock();
            if cache.pages.contains_key(&page) {
                cache.pages.insert(page, data.take().expect("cache page should exist"));
                cache.touch(page);
                return Ok(());
            }

            if cache.pages.len() < CACHE_PAGE_COUNT {
                cache.pages.insert(page, data.take().expect("cache page should exist"));
                cache.touch(page);
                return Ok(());
            }

            drop(cache);
            self.evict_one_cache_page()?;
        }
    }

    fn read_from_cache_page(&self, page: usize, page_offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        {
            let mut cache = self.cache.lock();
            if cache.read_range(page, page_offset, buf) {
                return Ok(());
            }
        }

        let loaded = self.load_cache_page(page)?;
        self.cache_page_if_absent(page, loaded)?;

        let mut cache = self.cache.lock();
        let cached = cache.read_range(page, page_offset, buf);
        debug_assert!(cached);
        Ok(())
    }

    fn cached_page_for_write(&self, page: usize, full_page_write: bool) -> Result<CachePage, ()> {
        if full_page_write {
            return Ok(CachePage::alloc_zeroed());
        }

        if let Some(cached) = self.cache.lock().clone_page(page) {
            return Ok(cached);
        }

        self.load_cache_page(page)
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
        self.read_at(block * BLOCK_SIZE, buf)
    }

    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        self.write_at(block * BLOCK_SIZE, buf)
    }

    fn read_blocks(&self, start_block: usize, buf: &mut [u8]) -> Result<(), ()> {
        self.read_at(start_block * BLOCK_SIZE, buf)
    }

    fn write_blocks(&self, start_block: usize, buf: &[u8]) -> Result<(), ()> {
        self.write_at(start_block * BLOCK_SIZE, buf)
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        let mut buf_offset = 0;

        while buf_offset < buf.len() {
            let absolute = offset + buf_offset;
            let page = absolute / CACHE_PAGE_SIZE;
            let page_offset = absolute % CACHE_PAGE_SIZE;
            let read_size = min(CACHE_PAGE_SIZE - page_offset, buf.len() - buf_offset);

            self.read_from_cache_page(page, page_offset, &mut buf[buf_offset..buf_offset + read_size])?;
            buf_offset += read_size;
        }

        Ok(())
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<(), ()> {
        let mut buf_offset = 0;

        while buf_offset < buf.len() {
            let absolute = offset + buf_offset;
            let page = absolute / CACHE_PAGE_SIZE;
            let page_offset = absolute % CACHE_PAGE_SIZE;
            let write_size = min(CACHE_PAGE_SIZE - page_offset, buf.len() - buf_offset);
            let full_page_write = page_offset == 0 && write_size == CACHE_PAGE_SIZE;

            let mut page_data = self.cached_page_for_write(page, full_page_write)?;
            page_data.write_range(page_offset, &buf[buf_offset..buf_offset + write_size]);

            self.upsert_cache_page(page, page_data)?;
            buf_offset += write_size;
        }

        Ok(())
    }

    fn flush(&self) -> Result<(), ()> {
        loop {
            let Some(writeback) = ({
                let cache = self.cache.lock();
                cache.lru.iter().rev().find_map(|page| cache.snapshot_writeback(*page))
            }) else {
                return Ok(());
            };

            self.write_back_snapshot(&writeback)?;

            let mut cache = self.cache.lock();
            cache.mark_clean_if_version(writeback.page, writeback.version);
        }
    }

    fn get_block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn get_block_count(&self) -> u64 {
        self.driver.lock().capacity()
    }
}

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use virtio_drivers::Error;
use virtio_drivers::device::blk::{BlkReq, BlkResp, RespStatus, VirtIOBlk};
use virtio_drivers::transport::Transport;

#[cfg(feature = "virtio-block-page-cache")]
use crate::arch;
use crate::driver::virtio::VirtIOHal;
use crate::driver::{BlockDriverOps, DeviceType, DriverOps};
use crate::kernel::event::Event;
#[cfg(feature = "virtio-block-page-cache")]
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::scheduler::{self, Task, current};
use crate::klib::SpinLock;
#[cfg(feature = "virtio-block-page-cache")]
use crate::klib::{SleepLock, lru::LRUCache};

const BLOCK_SIZE: usize = 512;

#[cfg(feature = "virtio-block-page-cache")]
struct PageCacheEntry {
    page: PhysPageFrame,
    dirty: bool,
}

#[cfg(feature = "virtio-block-page-cache")]
impl PageCacheEntry {
    fn clean(page: PhysPageFrame) -> Self {
        Self { page, dirty: false }
    }
}

#[cfg(feature = "virtio-block-page-cache")]
struct PageCache {
    pages: LRUCache<usize, PageCacheEntry>,
}

#[cfg(feature = "virtio-block-page-cache")]
impl PageCache {
    const CAPACITY: usize = 64;

    fn new() -> Self {
        Self { pages: LRUCache::new() }
    }

    fn contains_page(&self, page_index: usize) -> bool {
        self.pages.contains_key(&page_index)
    }

    fn len(&self) -> usize {
        self.pages.len()
    }

    fn insert(&mut self, page_index: usize, entry: PageCacheEntry) {
        self.pages.put(page_index, entry);
    }

    fn lru_page_mut(&mut self) -> Option<(usize, &mut PageCacheEntry)> {
        self.pages.tail()
    }

    fn pop_lru(&mut self) -> Option<usize> {
        self.pages.pop_lru()
    }

    fn write_dirty_pages<F>(&mut self, mut write_page: F) -> Result<(), ()>
    where
        F: FnMut(usize, &PhysPageFrame) -> Result<(), ()>,
    {
        self.pages.try_for_each_mut(|page_index, entry| {
            if entry.dirty {
                write_page(page_index, &entry.page)?;
                entry.dirty = false;
            }
            Ok(())
        })
    }

    fn copy_to_slice(&mut self, page_index: usize, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        self.pages.get(&page_index).ok_or(())?.page.copy_to_slice(offset, buf);
        Ok(())
    }

    fn copy_from_slice(&mut self, page_index: usize, offset: usize, buf: &[u8]) -> Result<(), ()> {
        let entry = self.pages.get_mut(&page_index).ok_or(())?;
        entry.page.copy_from_slice(offset, buf);
        entry.dirty = true;
        Ok(())
    }
}

pub struct VirtIOBlockDriver<T: Transport + Send + 'static> {
    device_name: String,
    #[cfg(feature = "virtio-block-page-cache")]
    cache: SleepLock<PageCache>,
    driver: SpinLock<VirtIOBlk<VirtIOHal, T>>,
    inflight: SpinLock<BTreeMap<u16, Arc<dyn Task>>>,
    queue_waiters: SpinLock<VecDeque<Arc<dyn Task>>>,
    read_only: bool,
    readahead: AtomicUsize,
}

impl<T: Transport + Send + 'static> VirtIOBlockDriver<T> {
    pub fn new(device_name: String, transport: T) -> Self {
        let mut blk = VirtIOBlk::new(transport).unwrap();
        blk.enable_interrupts();
        let read_only = blk.readonly();
        Self {
            device_name,
            #[cfg(feature = "virtio-block-page-cache")]
            cache: SleepLock::new(PageCache::new(), "VirtIOBlockDriver::cache"),
            driver: SpinLock::new(blk, "VirtIOBlockDriver::driver"),
            inflight: SpinLock::new(BTreeMap::new(), "VirtIOBlockDriver::inflight"),
            queue_waiters: SpinLock::new(VecDeque::new(), "VirtIOBlockDriver::queue_waiters"),
            read_only,
            readahead: AtomicUsize::new(0),
        }
    }

    fn wait_for_token(&self, token: u16) {
        let task = current::task();
        scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_io");
        self.inflight.lock().insert(token, task.clone());

        if self.driver.lock().peek_used() == Some(token) {
            self.wake_token(token);
        }

        current::schedule();
    }

    fn wake_token(&self, token: u16) {
        if let Some(task) = self.inflight.lock().remove(&token) {
            scheduler::wakeup_task_uninterruptible(task, Event::IOComplete);
        }
    }

    fn wake_next(&self) {
        let token = self.driver.lock().peek_used();
        if let Some(token) = token {
            self.wake_token(token);
        }
    }

    fn wake_queue_waiter(&self) {
        // Callers hold `driver`, matching the submit-side driver -> queue_waiters
        // lock order and publishing reclaimed descriptors after waiter registration.
        if let Some(task) = self.queue_waiters.lock().pop_front() {
            scheduler::wakeup_task_uninterruptible(task, Event::IOComplete);
        }
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn device_size(&self) -> Result<usize, ()> {
        let block_count = usize::try_from(self.get_block_count()).map_err(|_| ())?;
        block_count.checked_mul(BLOCK_SIZE).ok_or(())
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn check_range(&self, offset: usize, length: usize) -> Result<(), ()> {
        if length == 0 {
            return Ok(());
        }

        let device_size = self.device_size()?;
        let end = offset.checked_add(length).ok_or(())?;
        if offset >= device_size || end > device_size {
            return Err(());
        }
        Ok(())
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn page_len(&self, page_index: usize) -> Result<usize, ()> {
        let page_offset = page_index.checked_mul(arch::PGSIZE).ok_or(())?;
        let device_size = self.device_size()?;
        if page_offset >= device_size {
            return Err(());
        }
        Ok(core::cmp::min(arch::PGSIZE, device_size - page_offset))
    }

    fn raw_read_blocks(&self, start_block: usize, buf: &mut [u8]) -> Result<(), ()> {
        if buf.is_empty() {
            return Ok(());
        }

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = loop {
            let mut driver = self.driver.lock();
            // SAFETY: `req`, `buf`, and `resp` remain valid and untouched until
            // the matching `complete_read_blocks` call after a successful
            // submission. `QueueFull` is returned before they are published.
            match unsafe { driver.read_blocks_nb(start_block, &mut req, buf, &mut resp) } {
                Ok(token) => break token,
                Err(Error::QueueFull) => {
                    let task = current::task().clone();
                    scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_queue_full");
                    // Keep `driver` locked until this waiter is visible so a
                    // concurrent completion cannot release space and miss it.
                    self.queue_waiters.lock().push_back(task);
                    drop(driver);
                    current::schedule();
                }
                Err(err) => {
                    crate::kdebug!(
                        "virtio-blk read submit failed: device={}, start_block={}, len={}, err={:?}",
                        self.device_name,
                        start_block,
                        buf.len(),
                        err
                    );
                    return Err(());
                }
            }
        };

        self.wait_for_token(token);

        let complete_result = {
            let mut driver = self.driver.lock();
            // SAFETY: These are the same buffers and token passed to the
            // corresponding successful `read_blocks_nb` submission.
            let result = unsafe { driver.complete_read_blocks(token, &req, buf, &mut resp) };
            self.wake_queue_waiter();
            result
        };
        self.wake_next();
        complete_result.map_err(|err| {
            crate::kdebug!(
                "virtio-blk read completion failed: device={}, start_block={}, len={}, token={}, status={:?}, err={:?}",
                self.device_name,
                start_block,
                buf.len(),
                token,
                resp.status(),
                err
            );
        })?;

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

        if self.is_readonly() {
            return Err(());
        }

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let token = loop {
            let mut driver = self.driver.lock();
            // SAFETY: `req`, `buf`, and `resp` remain valid and untouched until
            // the matching `complete_write_blocks` call after a successful
            // submission. `QueueFull` is returned before they are published.
            match unsafe { driver.write_blocks_nb(start_block, &mut req, buf, &mut resp) } {
                Ok(token) => break token,
                Err(Error::QueueFull) => {
                    let task = current::task().clone();
                    scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_queue_full");
                    // Keep `driver` locked until this waiter is visible so a
                    // concurrent completion cannot release space and miss it.
                    self.queue_waiters.lock().push_back(task);
                    drop(driver);
                    current::schedule();
                }
                Err(err) => {
                    crate::kdebug!(
                        "virtio-blk write submit failed: device={}, start_block={}, len={}, err={:?}",
                        self.device_name,
                        start_block,
                        buf.len(),
                        err
                    );
                    return Err(());
                }
            }
        };

        self.wait_for_token(token);

        let complete_result = {
            let mut driver = self.driver.lock();
            // SAFETY: These are the same buffers and token passed to the
            // corresponding successful `write_blocks_nb` submission.
            let result = unsafe { driver.complete_write_blocks(token, &req, buf, &mut resp) };
            self.wake_queue_waiter();
            result
        };
        self.wake_next();
        complete_result.map_err(|err| {
            crate::kdebug!(
                "virtio-blk write completion failed: device={}, start_block={}, len={}, token={}, status={:?}, err={:?}",
                self.device_name,
                start_block,
                buf.len(),
                token,
                resp.status(),
                err
            );
        })?;

        if resp.status() == RespStatus::OK {
            Ok(())
        } else {
            Err(())
        }
    }

    fn raw_read_block(&self, block: usize, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), ()> {
        self.raw_read_blocks(block, buf)
    }

    fn raw_write_block(&self, block: usize, buf: &[u8; BLOCK_SIZE]) -> Result<(), ()> {
        self.raw_write_blocks(block, buf)
    }

    fn raw_read_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        let mut length = buf.len();
        let mut block = offset / BLOCK_SIZE;
        let mut buf_offset = 0;

        let block_offset = offset % BLOCK_SIZE;
        if block_offset != 0 {
            let mut block_buf = [0u8; BLOCK_SIZE];
            self.raw_read_block(block, &mut block_buf)?;

            let read_size = core::cmp::min(BLOCK_SIZE - block_offset, length);
            buf[buf_offset..buf_offset + read_size].copy_from_slice(&block_buf[block_offset..block_offset + read_size]);

            buf_offset += read_size;
            length -= read_size;
            block += 1;
        }

        let full_block_len = length / BLOCK_SIZE * BLOCK_SIZE;
        if full_block_len != 0 {
            self.raw_read_blocks(block, &mut buf[buf_offset..buf_offset + full_block_len])?;
            buf_offset += full_block_len;
            length -= full_block_len;
            block += full_block_len / BLOCK_SIZE;
        }

        if length != 0 {
            let mut block_buf = [0u8; BLOCK_SIZE];
            self.raw_read_block(block, &mut block_buf)?;
            buf[buf_offset..buf_offset + length].copy_from_slice(&block_buf[..length]);
        }

        Ok(())
    }

    fn raw_write_at(&self, offset: usize, buf: &[u8]) -> Result<(), ()> {
        if buf.is_empty() {
            return Ok(());
        }

        if self.is_readonly() {
            return Err(());
        }

        let mut length = buf.len();
        let mut block = offset / BLOCK_SIZE;
        let mut buf_offset = 0;

        let mut block_buf = [0u8; BLOCK_SIZE];
        let block_offset = offset % BLOCK_SIZE;
        if block_offset != 0 {
            self.raw_read_block(block, &mut block_buf)?;

            let write_size = core::cmp::min(BLOCK_SIZE - block_offset, length);
            block_buf[block_offset..block_offset + write_size]
                .copy_from_slice(&buf[buf_offset..buf_offset + write_size]);
            self.raw_write_block(block, &block_buf)?;

            buf_offset += write_size;
            length -= write_size;
            block += 1;
        }

        let full_block_len = length / BLOCK_SIZE * BLOCK_SIZE;
        if full_block_len != 0 {
            self.raw_write_blocks(block, &buf[buf_offset..buf_offset + full_block_len])?;
            buf_offset += full_block_len;
            length -= full_block_len;
            block += full_block_len / BLOCK_SIZE;
        }

        if length != 0 {
            self.raw_read_block(block, &mut block_buf)?;
            block_buf[..length].copy_from_slice(&buf[buf_offset..buf_offset + length]);
            self.raw_write_block(block, &block_buf)?;
        }

        Ok(())
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn read_page_from_device(&self, page_index: usize) -> Result<PhysPageFrame, ()> {
        let page_offset = page_index.checked_mul(arch::PGSIZE).ok_or(())?;
        let page_len = self.page_len(page_index)?;
        let page = PhysPageFrame::alloc_zeroed();
        self.raw_read_at(page_offset, &mut page.slice()[..page_len])?;
        Ok(page)
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn write_page_to_device(&self, page_index: usize, page: &PhysPageFrame) -> Result<(), ()> {
        let page_offset = page_index.checked_mul(arch::PGSIZE).ok_or(())?;
        let page_len = self.page_len(page_index)?;
        self.raw_write_at(page_offset, &page.slice()[..page_len])
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn insert_cache_page(&self, cache: &mut PageCache, page_index: usize, entry: PageCacheEntry) -> Result<(), ()> {
        cache.insert(page_index, entry);
        self.evict_cache_pages(cache)
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn evict_cache_pages(&self, cache: &mut PageCache) -> Result<(), ()> {
        while cache.len() > PageCache::CAPACITY {
            {
                let Some((page_index, entry)) = cache.lru_page_mut() else {
                    break;
                };
                if entry.dirty {
                    self.write_page_to_device(page_index, &entry.page)?;
                    entry.dirty = false;
                }
            }

            if cache.pop_lru().is_none() {
                break;
            }
        }

        Ok(())
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn readahead_page_count(&self) -> usize {
        let readahead_bytes = self.get_readahead().saturating_mul(BLOCK_SIZE);
        if readahead_bytes == 0 {
            return 0;
        }

        let pages = readahead_bytes.saturating_add(arch::PGSIZE - 1) / arch::PGSIZE;
        core::cmp::min(pages, PageCache::CAPACITY.saturating_sub(1))
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn readahead_pages(&self, cache: &mut PageCache, start_page: usize) -> Result<(), ()> {
        for page_index in start_page..start_page.saturating_add(self.readahead_page_count()) {
            if cache.contains_page(page_index) {
                continue;
            }

            let Ok(page) = self.read_page_from_device(page_index) else {
                break;
            };
            self.insert_cache_page(cache, page_index, PageCacheEntry::clean(page))?;
        }

        Ok(())
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn read_cached_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        self.check_range(offset, buf.len())?;
        let mut cache = self.cache.lock();
        let mut buf_offset = 0;

        while buf_offset < buf.len() {
            let current_offset = offset + buf_offset;
            let page_index = current_offset / arch::PGSIZE;
            let page_offset = current_offset % arch::PGSIZE;
            let copy_len = core::cmp::min(buf.len() - buf_offset, arch::PGSIZE - page_offset);

            if !cache.contains_page(page_index) {
                let page = self.read_page_from_device(page_index)?;
                self.insert_cache_page(&mut cache, page_index, PageCacheEntry::clean(page))?;
                self.readahead_pages(&mut cache, page_index + 1)?;
            }

            cache.copy_to_slice(page_index, page_offset, &mut buf[buf_offset..buf_offset + copy_len])?;
            buf_offset += copy_len;
        }

        Ok(())
    }

    #[cfg(feature = "virtio-block-page-cache")]
    fn write_cached_at(&self, offset: usize, buf: &[u8]) -> Result<(), ()> {
        if buf.is_empty() {
            return Ok(());
        }

        if self.is_readonly() {
            return Err(());
        }

        self.check_range(offset, buf.len())?;
        let mut cache = self.cache.lock();
        let mut buf_offset = 0;

        while buf_offset < buf.len() {
            let current_offset = offset + buf_offset;
            let page_index = current_offset / arch::PGSIZE;
            let page_offset = current_offset % arch::PGSIZE;
            let write_len = core::cmp::min(buf.len() - buf_offset, arch::PGSIZE - page_offset);
            let buf_start = buf_offset;
            let buf_end = buf_offset + write_len;

            if !cache.contains_page(page_index) {
                let page_len = self.page_len(page_index)?;
                let page = if page_offset == 0 && write_len == page_len {
                    PhysPageFrame::alloc_zeroed()
                } else {
                    self.read_page_from_device(page_index)?
                };
                self.insert_cache_page(&mut cache, page_index, PageCacheEntry::clean(page))?;
            }

            cache.copy_from_slice(page_index, page_offset, &buf[buf_start..buf_end])?;
            buf_offset += write_len;
        }

        Ok(())
    }
}

impl<T: Transport + Send + 'static> DriverOps for VirtIOBlockDriver<T> {
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
        let token = driver.peek_used();
        drop(driver);

        if let Some(token) = token {
            self.wake_token(token);
        }
    }
}

impl<T: Transport + Send + 'static> BlockDriverOps for VirtIOBlockDriver<T> {
    fn read_block(&self, block: usize, buf: &mut [u8]) -> Result<(), ()> {
        #[cfg(feature = "virtio-block-page-cache")]
        {
            let offset = block.checked_mul(BLOCK_SIZE).ok_or(())?;
            self.read_cached_at(offset, buf)
        }
        #[cfg(not(feature = "virtio-block-page-cache"))]
        {
            self.raw_read_blocks(block, buf)
        }
    }

    fn write_block(&self, block: usize, buf: &[u8]) -> Result<(), ()> {
        #[cfg(feature = "virtio-block-page-cache")]
        {
            let offset = block.checked_mul(BLOCK_SIZE).ok_or(())?;
            self.write_cached_at(offset, buf)
        }
        #[cfg(not(feature = "virtio-block-page-cache"))]
        {
            self.raw_write_blocks(block, buf)
        }
    }

    fn read_blocks(&self, start_block: usize, buf: &mut [u8]) -> Result<(), ()> {
        #[cfg(feature = "virtio-block-page-cache")]
        {
            let offset = start_block.checked_mul(BLOCK_SIZE).ok_or(())?;
            self.read_cached_at(offset, buf)
        }
        #[cfg(not(feature = "virtio-block-page-cache"))]
        {
            self.raw_read_blocks(start_block, buf)
        }
    }

    fn write_blocks(&self, start_block: usize, buf: &[u8]) -> Result<(), ()> {
        #[cfg(feature = "virtio-block-page-cache")]
        {
            let offset = start_block.checked_mul(BLOCK_SIZE).ok_or(())?;
            self.write_cached_at(offset, buf)
        }
        #[cfg(not(feature = "virtio-block-page-cache"))]
        {
            self.raw_write_blocks(start_block, buf)
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), ()> {
        #[cfg(feature = "virtio-block-page-cache")]
        {
            self.read_cached_at(offset, buf)
        }
        #[cfg(not(feature = "virtio-block-page-cache"))]
        {
            self.raw_read_at(offset, buf)
        }
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<(), ()> {
        #[cfg(feature = "virtio-block-page-cache")]
        {
            self.write_cached_at(offset, buf)
        }
        #[cfg(not(feature = "virtio-block-page-cache"))]
        {
            self.raw_write_at(offset, buf)
        }
    }

    fn flush(&self) -> Result<(), ()> {
        #[cfg(feature = "virtio-block-page-cache")]
        {
            let mut cache = self.cache.lock();
            cache.write_dirty_pages(|page_index, page| self.write_page_to_device(page_index, page))
        }

        #[cfg(not(feature = "virtio-block-page-cache"))]
        {
            Ok(())
        }
    }

    fn is_readonly(&self) -> bool {
        self.read_only
    }

    fn get_readahead(&self) -> usize {
        self.readahead.load(Ordering::Relaxed)
    }

    fn set_readahead(&self, readahead: usize) {
        self.readahead.store(readahead, Ordering::Relaxed);
    }

    fn get_block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn get_block_count(&self) -> u64 {
        self.driver.lock().capacity()
    }
}

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomPinned;
use core::mem::{self, size_of};
use core::pin::{Pin, pin};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use virtio_drivers::Error;
use virtio_drivers::device::blk::{BlkReq, BlkResp, VirtIOBlk};
use virtio_drivers::transport::Transport;

use crate::arch;
use crate::driver::virtio::VirtIOHal;
use crate::driver::{BlockDriverOps, DeviceType, DriverOps};
use crate::kernel::event::{Event, timer};
use crate::kernel::mm::ContiguousPhysPageFrame;
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

#[repr(C)]
struct DmaRequest {
    req: BlkReq,
    resp: BlkResp,
}

impl Default for DmaRequest {
    fn default() -> Self {
        Self {
            req: BlkReq::default(),
            resp: BlkResp::default(),
        }
    }
}

#[derive(Clone, Copy)]
enum RequestOperation {
    Read,
    Write,
}

struct InflightRequest {
    token: u16,
    operation: RequestOperation,
    start_block: usize,
    buffer_addr: usize,
    buffer_len: usize,
    task: Arc<dyn Task>,
    completion_addr: usize,
    deadline: Duration,
}

enum RequestSlot {
    Free,
    Reserved,
    Inflight(InflightRequest),
}

#[derive(Clone, Copy)]
struct RequestIo {
    slot: usize,
    operation: RequestOperation,
    start_block: usize,
    buffer_addr: usize,
    buffer_len: usize,
    completion_addr: usize,
}

struct RequestPool {
    dma: ContiguousPhysPageFrame,
    slots: Vec<RequestSlot>,
    token_slots: Vec<Option<usize>>,
}

impl RequestPool {
    fn new(queue_size: usize) -> Self {
        assert_ne!(queue_size, 0, "virtio-blk queue must not be empty");
        let bytes = queue_size
            .checked_mul(size_of::<DmaRequest>())
            .expect("virtio-blk request pool size overflow");
        let dma = ContiguousPhysPageFrame::alloc(arch::page_count(bytes));
        assert!(
            arch::dma_direct_paddr(dma.get_page(), bytes).is_some(),
            "virtio-blk request pool must be directly DMA-addressable"
        );

        for slot in 0..queue_size {
            // SAFETY: The DMA allocation owns enough aligned, contiguous space
            // for `queue_size` non-overlapping `DmaRequest` objects. Each slot
            // is initialized exactly once here and contains no drop fields.
            unsafe { dma.ptr().cast::<DmaRequest>().add(slot).write(DmaRequest::default()) };
        }

        Self {
            dma,
            slots: core::iter::repeat_with(|| RequestSlot::Free).take(queue_size).collect(),
            token_slots: core::iter::repeat_with(|| None).take(queue_size).collect(),
        }
    }

    fn reserve(&mut self) -> Option<usize> {
        let slot = self.slots.iter().position(|slot| matches!(slot, RequestSlot::Free))?;
        self.slots[slot] = RequestSlot::Reserved;
        Some(slot)
    }

    fn release_reserved(&mut self, slot: usize) {
        debug_assert!(matches!(self.slots[slot], RequestSlot::Reserved));
        self.slots[slot] = RequestSlot::Free;
    }

    fn dma_request(&mut self, slot: usize) -> &mut DmaRequest {
        debug_assert!(slot < self.slots.len());
        // SAFETY: The pool initialized every slot in `new`, the backing DMA
        // allocation outlives this borrow, and the pool lock serializes all
        // accesses to a slot's request and response headers.
        unsafe { &mut *self.dma.ptr().cast::<DmaRequest>().add(slot) }
    }

    fn publish(&mut self, slot: usize, request: InflightRequest) {
        let token = usize::from(request.token);
        assert!(
            token < self.token_slots.len(),
            "virtio-blk returned an out-of-range token"
        );
        assert!(self.token_slots[token].is_none(), "virtio-blk reused an inflight token");
        debug_assert!(matches!(self.slots[slot], RequestSlot::Reserved));
        self.token_slots[token] = Some(slot);
        self.slots[slot] = RequestSlot::Inflight(request);
    }

    fn io(&self, token: u16) -> Option<RequestIo> {
        let slot = *self.token_slots.get(usize::from(token))?.as_ref()?;
        let RequestSlot::Inflight(request) = &self.slots[slot] else {
            return None;
        };
        Some(RequestIo {
            slot,
            operation: request.operation,
            start_block: request.start_block,
            buffer_addr: request.buffer_addr,
            buffer_len: request.buffer_len,
            completion_addr: request.completion_addr,
        })
    }

    fn finish(&mut self, token: u16) -> Option<Arc<dyn Task>> {
        let token_index = usize::from(token);
        let slot = self.token_slots.get_mut(token_index)?.take()?;
        let RequestSlot::Inflight(request) = mem::replace(&mut self.slots[slot], RequestSlot::Free) else {
            return None;
        };
        debug_assert_eq!(request.token, token);
        Some(request.task)
    }

    fn refresh_deadline(&mut self, token: u16, deadline: Duration) {
        let Some(slot) = self
            .token_slots
            .get(usize::from(token))
            .and_then(|slot| slot.as_ref())
            .copied()
        else {
            return;
        };
        if let RequestSlot::Inflight(request) = &mut self.slots[slot] {
            request.deadline = deadline;
        }
    }

    fn expired_tasks(&self, now: Duration) -> Vec<Arc<dyn Task>> {
        self.slots
            .iter()
            .filter_map(|slot| match slot {
                RequestSlot::Inflight(request) if request.deadline <= now => Some(request.task.clone()),
                _ => None,
            })
            .collect()
    }

    fn inflight_count(&self) -> usize {
        self.token_slots.iter().filter(|slot| slot.is_some()).count()
    }
}

struct RequestCompletion {
    result: SpinLock<Option<Result<(), ()>>>,
    _pin: PhantomPinned,
}

impl RequestCompletion {
    fn new() -> Self {
        Self {
            result: SpinLock::new(None, "RequestCompletion::result"),
            _pin: PhantomPinned,
        }
    }

    fn finish(&self, result: Result<(), ()>) {
        let old = self.result.lock().replace(result);
        debug_assert!(old.is_none(), "virtio-blk request completed twice");
    }

    fn take_result_or_block(&self, task: Arc<dyn Task>) -> Option<Result<(), ()>> {
        let mut result = self.result.lock();
        if result.is_some() {
            return result.take();
        }
        scheduler::block_task_uninterruptible(task, "virtio_blk_io");
        None
    }
}

pub struct VirtIOBlockDriver<T: Transport + Send + 'static> {
    device_name: String,
    #[cfg(feature = "virtio-block-page-cache")]
    cache: SleepLock<PageCache>,
    driver: SpinLock<VirtIOBlk<VirtIOHal, T>>,
    requests: SpinLock<RequestPool>,
    queue_waiters: SpinLock<VecDeque<Arc<dyn Task>>>,
    read_only: bool,
    readahead: AtomicUsize,
}

impl<T: Transport + Send + 'static> VirtIOBlockDriver<T> {
    const IO_TIMEOUT: Duration = Duration::from_secs(1);
    const TIMEOUT_SCAN_INTERVAL: Duration = Duration::from_millis(250);

    pub fn new(device_name: String, transport: T) -> Arc<Self> {
        let mut blk = VirtIOBlk::new(transport).unwrap();
        blk.enable_interrupts();
        let read_only = blk.readonly();
        let request_pool = RequestPool::new(usize::from(blk.virt_queue_size()));
        let driver = Arc::new(Self {
            device_name,
            #[cfg(feature = "virtio-block-page-cache")]
            cache: SleepLock::new(PageCache::new(), "VirtIOBlockDriver::cache"),
            driver: SpinLock::new(blk, "VirtIOBlockDriver::driver"),
            requests: SpinLock::new(request_pool, "VirtIOBlockDriver::requests"),
            queue_waiters: SpinLock::new(VecDeque::new(), "VirtIOBlockDriver::queue_waiters"),
            read_only,
            readahead: AtomicUsize::new(0),
        });
        Self::arm_timeout(&driver);
        driver
    }

    fn arm_timeout(driver: &Arc<Self>) {
        let driver = Arc::downgrade(driver);
        timer::add_timer_with_callback(
            Self::TIMEOUT_SCAN_INTERVAL,
            Box::new(move || {
                let Some(driver) = driver.upgrade() else {
                    return;
                };
                driver.handle_timeout();
                Self::arm_timeout(&driver);
            }),
        );
    }

    fn handle_timeout(&self) {
        self.drain_completions(false);

        let now = timer::now();
        let (expired, inflight_count) = {
            let requests = self.requests.lock();
            (requests.expired_tasks(now), requests.inflight_count())
        };
        if expired.is_empty() {
            return;
        }

        let used_head = self.driver.lock().peek_used();
        crate::kwarn!(
            "virtio-blk I/O wait timed out: device={}, expired={}, inflight={}, used_head={:?}",
            self.device_name,
            expired.len(),
            inflight_count,
            used_head,
        );
        for task in expired {
            scheduler::wakeup_task_uninterruptible(task, Event::Timeout);
        }
    }

    fn wait_for_completion(&self, token: u16, completion: Pin<&RequestCompletion>) -> Result<(), ()> {
        loop {
            current::schedule();
            let event = current::task().take_wakeup_event().unwrap();
            debug_assert!(matches!(event, Event::IOComplete | Event::Timeout));
            if let Some(result) = completion.get_ref().take_result_or_block(current::task().clone()) {
                return result;
            }
            self.requests
                .lock()
                .refresh_deadline(token, timer::now() + Self::IO_TIMEOUT);
        }
    }

    fn wake_queue_waiters(&self, count: usize) {
        let waiters: Vec<_> = {
            let mut queue_waiters = self.queue_waiters.lock();
            (0..count).filter_map(|_| queue_waiters.pop_front()).collect()
        };
        for task in waiters {
            scheduler::wakeup_task_uninterruptible(task, Event::IOComplete);
        }
    }

    fn drain_completions(&self, acknowledge: bool) -> usize {
        let mut completed_tasks = Vec::new();
        {
            let mut driver = self.driver.lock();
            if acknowledge {
                driver.ack_interrupt();
            }

            while let Some(token) = driver.peek_used() {
                let mut requests = self.requests.lock();
                let Some(io) = requests.io(token) else {
                    crate::kwarn!(
                        "virtio-blk used token has no request: device={}, token={}",
                        self.device_name,
                        token
                    );
                    break;
                };

                let dma_request = requests.dma_request(io.slot);
                // SAFETY: `io` records the exact request header, response
                // header, and data buffer published for `token`. The owning
                // task remains blocked until this completion is recorded, and
                // the driver lock serializes used-ring consumption.
                let result = unsafe {
                    match io.operation {
                        RequestOperation::Read => driver.complete_read_blocks(
                            token,
                            &dma_request.req,
                            core::slice::from_raw_parts_mut(io.buffer_addr as *mut u8, io.buffer_len),
                            &mut dma_request.resp,
                        ),
                        RequestOperation::Write => driver.complete_write_blocks(
                            token,
                            &dma_request.req,
                            core::slice::from_raw_parts(io.buffer_addr as *const u8, io.buffer_len),
                            &mut dma_request.resp,
                        ),
                    }
                };
                let status = dma_request.resp.status();

                if driver.peek_used() == Some(token) {
                    crate::kwarn!(
                        "virtio-blk failed to consume used token: device={}, token={}, err={:?}",
                        self.device_name,
                        token,
                        result
                    );
                    break;
                }

                if let Err(err) = &result {
                    crate::kdebug!(
                        "virtio-blk completion failed: device={}, start_block={}, len={}, token={}, status={:?}, err={:?}",
                        self.device_name,
                        io.start_block,
                        io.buffer_len,
                        token,
                        status,
                        err
                    );
                }

                // SAFETY: `completion_addr` points to the pinned completion
                // object owned by the blocked submitter. The submitter cannot
                // return until a result is stored here, and this token has not
                // previously been removed from the request pool.
                unsafe { &*(io.completion_addr as *const RequestCompletion) }.finish(result.map_err(|_| ()));
                let task = requests
                    .finish(token)
                    .expect("virtio-blk request disappeared during completion");
                completed_tasks.push(task);
            }
        }

        let completed = completed_tasks.len();
        for task in completed_tasks {
            scheduler::wakeup_task_uninterruptible(task, Event::IOComplete);
        }
        self.wake_queue_waiters(completed);
        completed
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

        let completion = pin!(RequestCompletion::new());

        let token = loop {
            let mut driver = self.driver.lock();
            let mut requests = self.requests.lock();
            let Some(slot) = requests.reserve() else {
                let task = current::task().clone();
                scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_queue_full");
                self.queue_waiters.lock().push_back(task);
                drop(requests);
                drop(driver);
                current::schedule();
                let event = current::task().take_wakeup_event().unwrap();
                debug_assert_eq!(event, Event::IOComplete);
                continue;
            };

            let dma_request = requests.dma_request(slot);
            dma_request.resp = BlkResp::default();
            // SAFETY: This pool slot and `buf` remain valid and untouched until
            // `drain_completions` consumes the matching token. `QueueFull` is
            // returned before the buffers are published to the device.
            match unsafe { driver.read_blocks_nb(start_block, &mut dma_request.req, buf, &mut dma_request.resp) } {
                Ok(token) => {
                    let task = current::task().clone();
                    scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_io");
                    requests.publish(
                        slot,
                        InflightRequest {
                            token,
                            operation: RequestOperation::Read,
                            start_block,
                            buffer_addr: buf.as_mut_ptr() as usize,
                            buffer_len: buf.len(),
                            task,
                            completion_addr: completion.as_ref().get_ref() as *const RequestCompletion as usize,
                            deadline: timer::now() + Self::IO_TIMEOUT,
                        },
                    );
                    break token;
                }
                Err(Error::QueueFull) => {
                    requests.release_reserved(slot);
                    let task = current::task().clone();
                    scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_queue_full");
                    self.queue_waiters.lock().push_back(task);
                    drop(requests);
                    drop(driver);
                    current::schedule();
                    let event = current::task().take_wakeup_event().unwrap();
                    debug_assert_eq!(event, Event::IOComplete);
                }
                Err(err) => {
                    requests.release_reserved(slot);
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

        self.wait_for_completion(token, completion.as_ref())
    }

    fn raw_write_blocks(&self, start_block: usize, buf: &[u8]) -> Result<(), ()> {
        if buf.is_empty() {
            return Ok(());
        }

        if self.is_readonly() {
            return Err(());
        }

        let completion = pin!(RequestCompletion::new());

        let token = loop {
            let mut driver = self.driver.lock();
            let mut requests = self.requests.lock();
            let Some(slot) = requests.reserve() else {
                let task = current::task().clone();
                scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_queue_full");
                self.queue_waiters.lock().push_back(task);
                drop(requests);
                drop(driver);
                current::schedule();
                let event = current::task().take_wakeup_event().unwrap();
                debug_assert_eq!(event, Event::IOComplete);
                continue;
            };

            let dma_request = requests.dma_request(slot);
            dma_request.resp = BlkResp::default();
            // SAFETY: This pool slot and `buf` remain valid and untouched until
            // `drain_completions` consumes the matching token. `QueueFull` is
            // returned before the buffers are published to the device.
            match unsafe { driver.write_blocks_nb(start_block, &mut dma_request.req, buf, &mut dma_request.resp) } {
                Ok(token) => {
                    let task = current::task().clone();
                    scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_io");
                    requests.publish(
                        slot,
                        InflightRequest {
                            token,
                            operation: RequestOperation::Write,
                            start_block,
                            buffer_addr: buf.as_ptr() as usize,
                            buffer_len: buf.len(),
                            task,
                            completion_addr: completion.as_ref().get_ref() as *const RequestCompletion as usize,
                            deadline: timer::now() + Self::IO_TIMEOUT,
                        },
                    );
                    break token;
                }
                Err(Error::QueueFull) => {
                    requests.release_reserved(slot);
                    let task = current::task().clone();
                    scheduler::block_task_uninterruptible(task.clone(), "virtio_blk_queue_full");
                    self.queue_waiters.lock().push_back(task);
                    drop(requests);
                    drop(driver);
                    current::schedule();
                    let event = current::task().take_wakeup_event().unwrap();
                    debug_assert_eq!(event, Event::IOComplete);
                }
                Err(err) => {
                    requests.release_reserved(slot);
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

        self.wait_for_completion(token, completion.as_ref())
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
        self.drain_completions(true);
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

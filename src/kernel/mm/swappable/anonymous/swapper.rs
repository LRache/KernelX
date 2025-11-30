use core::time::Duration;
use alloc::sync::Arc;
use bitvec::vec::BitVec;

use crate::driver::BlockDriverOps;
use crate::arch;
use crate::kernel::errno::SysResult;
use crate::kernel::mm::swappable::LRUCache;
use crate::kernel::mm::swappable::anonymous::frame::NO_DISK_BLOCK;
use crate::kernel::mm::swappable::anonymous::frame::{AnonymousFrameInner, State};
use crate::kernel::mm::PhysPageFrame;
use crate::klib::{InitedCell, SpinLock};

struct Counter {
    swap_in_count: usize,
    swap_out_count: usize,
    swap_in_time: Duration,
    swap_out_time: Duration,
    shrink_count: usize,
    shrink_time: Duration,
}

static COUNTER: SpinLock<Counter> = SpinLock::new(Counter {
    swap_in_count: 0,
    swap_out_count: 0,
    swap_in_time: Duration::ZERO,
    swap_out_time: Duration::ZERO,
    shrink_count: 0,
    shrink_time: Duration::ZERO,
});

pub fn print_perf_info() {
    let counter = COUNTER.lock();
    crate::kinfo!("Anonymous Swapper Performance Info:");
    crate::kinfo!("  Swap In: {} times, total time: {:?}", counter.swap_in_count, counter.swap_in_time);
    crate::kinfo!("  Swap Out: {} times, total time: {:?}", counter.swap_out_count, counter.swap_out_time);
    crate::kinfo!("  Shrink: {} times, total time: {:?}", counter.shrink_count, counter.shrink_time);
}

struct BitMap {
    allocated: BitVec,
    cached: BitVec, // `1` means this block has a cached page in memory `0` means this block only has a swapped out page on disk
}

struct Swapper {
    bitmap: SpinLock<BitMap>,
    driver: Arc<dyn BlockDriverOps>,
    lru: SpinLock<LRUCache<usize, Arc<AnonymousFrameInner>>>,
    block_per_page: usize,
}

impl Swapper {
    fn new(driver: Arc<dyn BlockDriverOps>) -> Self {
        let block_size = driver.get_block_size() as usize;
        let len = block_size * driver.get_block_count() as usize / arch::PGSIZE;
        let bitmap = SpinLock::new(BitMap { allocated: BitVec::repeat(false, len), cached: BitVec::repeat(false, len) });
        let lru = LRUCache::new();
        let block_per_page = arch::PGSIZE / block_size;
        
        debug_assert!(block_per_page * block_size == arch::PGSIZE);
        crate::kinfo!("Anonymous swapper: {} pages ({} KB) available.", len, len * arch::PGSIZE / 1024);
        
        Self {
            bitmap, 
            driver, 
            lru: SpinLock::new(lru),
            block_per_page,
        }
    }

    fn free(&self, inner: &AnonymousFrameInner) {
        let state = inner.state.lock();
        match &state.state {
            State::Allocated(allocated) => {
                let key = allocated.frame.get_page();
                self.lru.lock().remove(&key);
            }
            State::SwappedOut => {
                let mut bitmap = self.bitmap.lock();
                let pos = state.disk_block / arch::PGSIZE;
                bitmap.allocated.set(pos, false);
                bitmap.cached.set(pos, false);
            }
        }
    }

    fn read_swapped_out(&self, diskpos: usize, frame: &PhysPageFrame) {
        let start = crate::kernel::event::timer::now();
        self.driver.read_blocks(
            diskpos, 
            frame.slice()
        ).expect("Failed to read swapped out page from block device");
        let mut counter = COUNTER.lock();
        counter.swap_in_count += 1;
        counter.swap_in_time += crate::kernel::event::timer::now() - start;
    }

    fn swap_in(&self, block_start: usize, inner: &Arc<AnonymousFrameInner>) -> SysResult<PhysPageFrame> {
        let swap_start = crate::kernel::event::timer::now();
        
        let frame = PhysPageFrame::alloc_with_shrink_zeroed();
        self.read_swapped_out(block_start, &frame);

        let page = frame.get_page();
        self.lru.lock().put(page, inner.clone());

        // Cached the swap page in disk
        let mut bitmap = self.bitmap.lock();
        let pos = block_start / self.block_per_page;
        bitmap.cached.set(pos, true);
        debug_assert!(
            bitmap.allocated.get(pos).map(|b| *b).unwrap_or(false), 
            "Swapped in page must be allocated in bitmap, pos={}, uaddr={:#x}", pos, inner.uaddr
        );

        let mut counter = COUNTER.lock();
        counter.swap_in_count += 1;
        counter.swap_in_time += crate::kernel::event::timer::now() - swap_start;

        Ok(frame)
    }

    fn shrink(&self, page_count: &mut usize, min_to_shrink: usize) {
        let shrink_start = crate::kernel::event::timer::now();
        let mut lru = self.lru.lock();
        let mut bitmap = self.bitmap.lock();
        
        let mut swapped_count = 0;
        for _ in 0..(*page_count * 2) {
            if let Some((kpage, frame)) = lru.tail() {
                let mut state = frame.state.lock();
                let disk_block = state.disk_block;
                
                let allocated = match &mut state.state {
                    State::Allocated(allocated) => { allocated },
                    State::SwappedOut => { unreachable!() }
                };

                let mut page_accessed = false;
                let mut page_dirty = allocated.dirty;
                for member in &*frame.family_chain.lock() {
                    let addrspace = match member.upgrade() {
                        Some(addrspace) => addrspace,
                        None => { continue; }
                    };

                    let (accessed, dirty) = addrspace.take_page_access_dirty_bit(frame.uaddr).unwrap_or((false, false));
                    page_accessed |= accessed;
                    page_dirty |= dirty;
                }

                if page_accessed {
                    allocated.dirty = page_dirty;
                    drop(state);
                    lru.access(&kpage);
                    continue;
                } 
                    
                let pos = if disk_block != NO_DISK_BLOCK {
                    disk_block / self.block_per_page
                } else if let Some(pos) = bitmap.allocated.first_zero() {
                    pos
                } else if let Some(pos) = bitmap.cached.first_one() {
                    pos
                } else {
                    panic!("No space available in swapper during shrinking");
                };
                    
                // Swap out this frame
                let block_start = pos * self.block_per_page;
                
                // Page is dirty or the block does not have a cached page in memory
                if page_dirty || !bitmap.cached.get(pos).map(|b| *b).unwrap() {
                    self.driver.write_blocks(
                        block_start, 
                        allocated.frame.slice()
                    ).expect("Failed to write swapped out page to block device");
                    // kinfo!("Write back swapped out page to block device at block {}", block_start);
                }

                bitmap.allocated.set(pos, true);
                bitmap.cached.set(pos, false); // This block does not saved a page cache but a swapped out page

                frame.family_chain.lock().retain(|member| {
                    if let Some(addrspace) = member.upgrade() {
                        addrspace.unmap_swap_page(frame.uaddr, kpage);
                        true
                    } else {
                        false
                    }
                });

                state.state = State::SwappedOut;
                state.disk_block = block_start;

                drop(state);
                // debug_assert!(lru.pop_lru().unwrap() == kpage);
                lru.pop_lru();

                swapped_count += 1;

                if swapped_count >= *page_count {
                    break;
                }
            } else {
                break;
            }
        }

        if swapped_count < min_to_shrink {
            for _ in swapped_count..min_to_shrink {
                if let Some((kpage, frame)) = lru.tail() {
                    let mut state = frame.state.lock();
                    let disk_block = state.disk_block;
                    
                    let allocated = match &mut state.state {
                        State::Allocated(allocated) => { allocated },
                        State::SwappedOut => { unreachable!() }
                    };

                    let pos = if disk_block != NO_DISK_BLOCK {
                        disk_block / self.block_per_page
                    } else if let Some(pos) = bitmap.allocated.first_zero() {
                        pos
                    } else if let Some(pos) = bitmap.cached.first_zero() {
                        pos
                    } else {
                        panic!("No space available in swapper during shrinking");
                    };
                    
                    // Swap out this frame
                    let block_start = pos * self.block_per_page;

                    let page_dirty = if allocated.dirty {
                        true
                    } else {
                        let mut dirty = false;
                        for member in &*frame.family_chain.lock() {
                            let addrspace = match member.upgrade() {
                                Some(addrspace) => addrspace,
                                None => { continue; }
                            };

                            let (_accessed, d) = addrspace.take_page_access_dirty_bit(frame.uaddr)
                                                                      .unwrap_or((false, false));
                            if d {
                                dirty = true;
                                break;
                            }
                        }
                        dirty
                    };
                    
                    if page_dirty || !bitmap.cached.get(pos).map(|b| *b).unwrap() {
                        self.driver.write_blocks(
                            block_start, 
                            allocated.frame.slice()
                        ).expect("Failed to write swapped out page to block device");
                    }

                    bitmap.allocated.set(pos, true);
                    bitmap.cached.set(pos, false); // This block does not saved a page cache but a swapped out page

                    frame.family_chain.lock().retain(|member| {
                        if let Some(addrspace) = member.upgrade() {
                            addrspace.unmap_swap_page(frame.uaddr, kpage);
                            true
                        } else {
                            false
                        }
                    });

                    state.state = State::SwappedOut;
                    state.disk_block = block_start;

                    drop(state);
                    // debug_assert!(lru.pop_lru().unwrap() == kpage);
                    lru.pop_lru();

                    swapped_count += 1;
                } else {
                    break;
                }
            }
        }

        *page_count -= swapped_count;

        let mut counter = COUNTER.lock();
        counter.shrink_count += 1;
        counter.shrink_time += crate::kernel::event::timer::now() - shrink_start;
    }
}

static SWAPPER: InitedCell<Swapper> = InitedCell::uninit();

pub fn init_swapper(driver: Arc<dyn BlockDriverOps>) {
    SWAPPER.init(Swapper::new(driver));
}

pub fn push_lru(kpage: usize, frame: Arc<AnonymousFrameInner>) {
    SWAPPER.lru.lock().put(kpage, frame);
}

pub fn dealloc(frame: &AnonymousFrameInner) {
    SWAPPER.free(frame);
}

pub(super) fn swap_in_page(block_start: usize, inner: &Arc<AnonymousFrameInner>) -> PhysPageFrame {
    SWAPPER.swap_in(block_start, inner).expect("swap in page failed")
}

pub(super) fn read_swapped_page(diskpos: usize, frame: &PhysPageFrame) {
    SWAPPER.read_swapped_out(diskpos, frame);
}

pub fn shrink(page_count: &mut usize, min_to_shrink: usize) {
    SWAPPER.shrink(page_count, min_to_shrink);
}

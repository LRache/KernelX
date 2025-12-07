use core::time::Duration;
use alloc::sync::Arc;

use crate::kernel::mm::swappable::LRUCache;
use crate::klib::{InitedCell, SpinLock};

use super::SwappableFrame;

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

pub fn counter_swap_in(time: Duration) {
    let mut counter = COUNTER.lock();
    counter.swap_in_count += 1;
    counter.swap_in_time += time;
}

pub fn counter_swap_out(time: Duration) {
    let mut counter = COUNTER.lock();
    counter.swap_out_count += 1;
    counter.swap_out_time += time;
}

#[derive(Clone)]
struct SwapEntry {
    frame: Arc<dyn SwappableFrame>,
    dirty: bool,
}

impl SwapEntry {
    fn new(frame: Arc<dyn SwappableFrame>) -> Self {
        Self {
            frame,
            dirty: false,
        }
    }
}

struct Swapper {
    lru: SpinLock<LRUCache<usize, SwapEntry>>,
}

impl Swapper {
    fn new() -> Self {
        Self {
            lru: SpinLock::new(LRUCache::new()),
        }
    }

    fn put(&self, kpage: usize, frame: Arc<dyn SwappableFrame>) {
        self.lru.lock().put(kpage, SwapEntry::new(frame));
    }

    fn remove_lru(&self, kpage: usize) {
        self.lru.lock().remove(&kpage);
    }

    fn shrink(&self, page_count: usize, min_to_shrink: usize) {
        let shrink_start = crate::kernel::event::timer::now();
        let mut lru = self.lru.lock();
        
        let mut swapped_count = 0;
        for _ in 0..(page_count * 2) {
            if let Some((key, entry)) = lru.tail() {
                // Remove the tail entry so we can own and modify it
                let frame = &entry.frame;
                let (accessed, dirty) = frame.take_access_dirty_bit().unwrap_or((false, false));

                if accessed {
                    entry.dirty |= dirty;
                    // Recently accessed, move it to the head of LRU by reinserting
                    lru.access(&key);
                    continue;
                }

                frame.swap_out(dirty);
                lru.pop_lru();

                swapped_count += 1;

                if swapped_count >= page_count {
                    break;
                }
            } else {
                break;
            }
        }

        if swapped_count < min_to_shrink {
            for _ in swapped_count..min_to_shrink {
                if let Some((_, entry)) = lru.tail() {
                    let frame = &entry.frame;
                    
                    // Swap out this frame
                    let mut dirty = entry.dirty;
                    if !dirty {
                        let (_, d) = frame.take_access_dirty_bit().unwrap_or((false, false));
                        dirty |= d;
                    }
                    
                    let start = crate::kernel::event::timer::now();
                    frame.swap_out(dirty);
                    lru.pop_lru();
                    let end = crate::kernel::event::timer::now();
                    counter_swap_out(end - start);
                } else {
                    break;
                }
            }
        }

        let mut counter = COUNTER.lock();
        counter.shrink_count += 1;
        counter.shrink_time += crate::kernel::event::timer::now() - shrink_start;
    }
}

static SWAPPER: InitedCell<Swapper> = InitedCell::uninit();

pub fn init_swapper() {
    SWAPPER.init(Swapper::new());
}

pub fn push_lru(kpage: usize, frame: Arc<dyn SwappableFrame>) {
    SWAPPER.put(kpage, frame);
}

pub fn remove_lru(kpage: usize) {
    SWAPPER.remove_lru(kpage);
}

pub fn shrink(page_count: usize, min_to_shrink: usize) {
    SWAPPER.shrink(page_count, min_to_shrink);
}

use alloc::sync::Arc;
use core::time::Duration;

use crate::kernel::event::timer;
use crate::klib::intrusive_lru::PriorityIntrusiveLru;
use crate::klib::{InitedCell, SpinLock};

use super::swappable::{ReclaimClass, SwapOutResult, SwappableFrameOps};

type SwapperLru = PriorityIntrusiveLru<dyn SwappableFrameOps, { ReclaimClass::COUNT }>;

struct Counter {
    #[cfg(feature = "swap-memory")]
    swap_in_count: usize,
    #[cfg(feature = "swap-memory")]
    swap_out_count: usize,
    #[cfg(feature = "swap-memory")]
    swap_in_time: Duration,
    #[cfg(feature = "swap-memory")]
    swap_out_time: Duration,
    shrink_count: usize,
    shrink_time: Duration,
    shrink_attempts: usize,
    reclaimed_pages: usize,
    shrink_shortfalls: usize,
}

static COUNTER: SpinLock<Counter> = SpinLock::new(
    Counter {
        #[cfg(feature = "swap-memory")]
        swap_in_count: 0,
        #[cfg(feature = "swap-memory")]
        swap_out_count: 0,
        #[cfg(feature = "swap-memory")]
        swap_in_time: Duration::ZERO,
        #[cfg(feature = "swap-memory")]
        swap_out_time: Duration::ZERO,
        shrink_count: 0,
        shrink_time: Duration::ZERO,
        shrink_attempts: 0,
        reclaimed_pages: 0,
        shrink_shortfalls: 0,
    },
    "swapper::COUNTER",
);

struct Swapper {
    lru: SwapperLru,
}

impl Swapper {
    fn new() -> Self {
        Self {
            lru: SwapperLru::new("Swapper::lru"),
        }
    }

    fn shrink(&self, page_count: usize, min_to_shrink: usize) {
        let start = timer::now();
        let target = page_count.max(min_to_shrink);
        let initial_len = self.lru.len();
        let attempt_limit = initial_len.saturating_add(target);
        let mut attempts = 0;
        let mut reclaimed = 0;

        while attempts < attempt_limit && reclaimed < target {
            let Some(victim) = self.lru.pop_back() else {
                break;
            };
            attempts += 1;

            match victim.clone().try_swap_out() {
                Ok(SwapOutResult::Evicted | SwapOutResult::Released) => reclaimed += 1,
                Ok(SwapOutResult::Referenced | SwapOutResult::Failed) => {
                    let class = victim.reclaim_class();
                    self.lru.move_to_front(victim, class.index());
                }
                Ok(SwapOutResult::Busy) | Err(_) => {}
            }
        }

        let mut counter = COUNTER.lock();
        counter.shrink_count += 1;
        counter.shrink_time += timer::now() - start;
        counter.shrink_attempts += attempts;
        counter.reclaimed_pages += reclaimed;
        counter.shrink_shortfalls += usize::from(reclaimed < target);
    }
}

static SWAPPER: InitedCell<Swapper> = InitedCell::uninit();

pub fn init_swapper() {
    SWAPPER.init(Swapper::new());
}

pub fn link(page: Arc<dyn SwappableFrameOps>) {
    let class = page.reclaim_class();
    SWAPPER.lru.link_front(page, class.index());
}

pub fn relink(page: Arc<dyn SwappableFrameOps>, class: ReclaimClass) {
    SWAPPER.lru.move_to_front(page, class.index());
}

pub fn unlink(page: &Arc<dyn SwappableFrameOps>) -> bool {
    SWAPPER.lru.unlink(page)
}

pub fn shrink(page_count: usize, min_to_shrink: usize) {
    SWAPPER.shrink(page_count, min_to_shrink);
}

#[cfg(feature = "swap-memory")]
pub fn counter_swap_in(time: Duration) {
    let mut counter = COUNTER.lock();
    counter.swap_in_count += 1;
    counter.swap_in_time += time;
}

#[cfg(feature = "swap-memory")]
pub fn counter_swap_out(time: Duration) {
    let mut counter = COUNTER.lock();
    counter.swap_out_count += 1;
    counter.swap_out_time += time;
}

pub fn print_perf_info() {
    let counter = COUNTER.lock();
    #[cfg(feature = "swap-memory")]
    {
        crate::kinfo!("Anonymous Swapper Performance Info:");
        crate::kinfo!(
            "  Swap In: {} times, total time: {:?}",
            counter.swap_in_count,
            counter.swap_in_time
        );
        crate::kinfo!(
            "  Swap Out: {} times, total time: {:?}",
            counter.swap_out_count,
            counter.swap_out_time
        );
    }
    crate::kinfo!("Page Reclaimer Performance Info:");
    crate::kinfo!(
        "  Shrink: {} times, total time: {:?}",
        counter.shrink_count,
        counter.shrink_time
    );
    crate::kinfo!(
        "  Attempts: {}, reclaimed pages: {}, shortfalls: {}, current LRU: {}",
        counter.shrink_attempts,
        counter.reclaimed_pages,
        counter.shrink_shortfalls,
        SWAPPER.lru.len()
    );
}

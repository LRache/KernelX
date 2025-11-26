use core::time::Duration;

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::vec;
use bitvec::vec::BitVec;

use crate::driver::BlockDriverOps;
use crate::arch;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::swappable::AddrSpaceFamilyChain;
use crate::kernel::mm::swappable::LRUCache;
use crate::kernel::mm::swappable::anonymous::frame::{AnonymousFrameInner, AnonymousFrameStatus};
use crate::kernel::mm::PhysPageFrame;
use crate::klib::{InitedCell, SpinLock};

use super::AnonymousFrame;

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

struct Swapper {
    bitmap: SpinLock<BitVec>,
    driver: Arc<dyn BlockDriverOps>,
    lru: SpinLock<LRUCache<usize, Arc<AnonymousFrameInner>>>,
    // frames: SpinLock<BTreeMap<usize, Arc<AnonymousFrameInner>>>,
    block_per_page: usize,
    block_size: usize,
}

impl Swapper {
    fn new(driver: Arc<dyn BlockDriverOps>) -> Self {
        let block_size = driver.get_block_size() as usize;
        let len = block_size * driver.get_block_count() as usize / arch::PGSIZE;
        let bitmap = SpinLock::new(BitVec::repeat(false, len));
        let lru = LRUCache::new();
        let block_per_page = arch::PGSIZE / block_size;
        debug_assert!(block_per_page * block_size == arch::PGSIZE);
        crate::kinfo!("Anonymous swapper: {} pages ({} KB) available.", len, len * arch::PGSIZE / 1024);
        
        Self {
            bitmap, 
            driver, 
            lru: SpinLock::new(lru),
            block_per_page,
            block_size
        }
    }

    fn alloc(&self, family_chain: AddrSpaceFamilyChain, uaddr: usize) -> AnonymousFrame {
        let frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let key = frame.get_page();
        let inner = Arc::new(AnonymousFrameInner::allocated(frame, family_chain, uaddr));
        self.lru.lock().put(key, inner.clone());
        AnonymousFrame::new(inner)
    }

    fn free(&self, inner: &AnonymousFrameInner) {
        match &*inner.status.lock() {
            AnonymousFrameStatus::Allocated(allocated) => {
                let key = allocated.get_page();
                self.lru.lock().remove(&key);
            }
            AnonymousFrameStatus::SwappedOut(disk_offset) => {
                self.bitmap.lock().set(disk_offset / arch::PGSIZE, false);
            }
        }
    }

    fn swap_out(&self, inner: &AnonymousFrameInner) -> SysResult<()> {
        let swap_start = crate::kernel::event::timer::now();
        let mut bitmap = self.bitmap.lock();
        if let Some(pos) = bitmap.first_zero() {
            let mut status = inner.status.lock();
            bitmap.set(pos, true);
            let allocated = match &*status {
                AnonymousFrameStatus::Allocated(alloc) => alloc,
                AnonymousFrameStatus::SwappedOut(_) => panic!("Trying to swap out an already swapped-out frame"),
            };

            let block_start = pos * self.block_per_page;
            for i in 0..self.block_per_page {
                let block_index = block_start + i;
                self.driver.write_block(
                    block_index, 
                    &allocated.slice()[i * self.block_size..(i + 1) * self.block_size]
                ).map_err(|_| Errno::ENOMEM)?;
            }

            let page = allocated.get_page();

            let mut family_chain = inner.family_chain.lock();
            family_chain.retain(|member| {
                if let Some(addrspace) = member.upgrade() {
                    addrspace.unmap_swap_page(inner.uaddr, page);
                    true
                } else {
                    false
                }
            });
            
            // The physical page will be freed here because of the drop of `allocated`
            *status = AnonymousFrameStatus::SwappedOut(block_start);
            
            let mut counter = COUNTER.lock();
            counter.swap_out_count += 1;
            counter.swap_out_time += crate::kernel::event::timer::now() - swap_start;

            Ok(())
        } else {
            Err(Errno::ENOMEM)
        }
    }

    fn read_swapped_out(&self, diskpos: usize, frame: &PhysPageFrame) {
        for i in 0..self.block_per_page {
            let block_index = diskpos + i;
            self.driver.read_block(
                block_index, 
                &mut frame.slice()[i * self.block_size..(i + 1) * self.block_size]
            ).expect("Failed to read swapped out page from block device");
        }
    }

    fn swap_in(&self, inner: &Arc<AnonymousFrameInner>) -> SysResult<usize> {
        let swap_start = crate::kernel::event::timer::now();
        let mut status = inner.status.lock();
        let diskpos = match &*status {
            AnonymousFrameStatus::SwappedOut(offset) => *offset,
            AnonymousFrameStatus::Allocated(_) => panic!("Trying to swap in an already allocated frame"),
        };

        let frame = PhysPageFrame::alloc_with_shrink_zeroed();
        self.read_swapped_out(diskpos, &frame);

        let page = frame.get_page();
        self.lru.lock().put(page, inner.clone());

        *status = AnonymousFrameStatus::Allocated(frame);

        let mut bitmap = self.bitmap.lock();
        bitmap.set(diskpos / self.block_per_page, false);

        let mut counter = COUNTER.lock();
        counter.swap_in_count += 1;
        counter.swap_in_time += crate::kernel::event::timer::now() - swap_start;

        Ok(page)
    }

    fn shrink(&self, page_count: &mut usize) {
        let shrink_start = crate::kernel::event::timer::now();
        let mut lru = self.lru.lock();
        let mut to_swap_out: Vec<Option<Arc<AnonymousFrameInner>>> = vec![None; *page_count];
        for i in 0..*page_count {
            if let Some((_, frame)) = lru.pop_lru() {
                let status = frame.status.lock();
                let page = match &*status {
                    AnonymousFrameStatus::Allocated(allocated) => { allocated.get_page() },
                    AnonymousFrameStatus::SwappedOut(_) => { unreachable!() }
                };

                let mut tag = false;
                for member in &*frame.family_chain.lock() {
                    let addrspace = match member.upgrade() {
                        Some(addrspace) => addrspace,
                        None => { continue; }
                    };

                    if addrspace.take_page_access_bit(page).unwrap_or(false) {
                        tag = true;
                        lru.access(&page);
                        break;
                    }
                }
                if !tag {
                    to_swap_out[i] = Some(frame.clone());
                    continue;
                }
            } else {
                break;
            }
        }

        let mut swapped_count = 0;
        to_swap_out.iter().for_each(|i| {
            if let Some(frame) = i {
                if let Err(e) = self.swap_out(frame) {
                    crate::kwarn!("Failed to swap out anonymous frame during shrinking: {:?}", e);
                } else {
                    swapped_count += 1;
                }
            }
        });
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

pub fn alloc(family_chain: AddrSpaceFamilyChain, uaddr: usize) -> AnonymousFrame {
    SWAPPER.alloc(family_chain, uaddr)
}

pub fn dealloc(frame: &AnonymousFrameInner) {
    SWAPPER.free(frame);
}

pub(super) fn swap_in_page(inner: &Arc<AnonymousFrameInner>) -> usize {
    SWAPPER.swap_in(inner).expect("swap in page failed")
}

pub(super) fn read_swapped_page(diskpos: usize, frame: &PhysPageFrame) {
    SWAPPER.read_swapped_out(diskpos, frame);
}

pub fn shrink(page_count: &mut usize) {
    SWAPPER.shrink(page_count);
}

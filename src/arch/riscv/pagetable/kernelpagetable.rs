use crate::arch::flush_tlb_all;
use crate::arch::riscv::{KERNEL_MMIO_START, KERNEL_STACK_ARENA_END, PGSIZE};
use crate::kernel::mm::MapPerm;
use crate::kernel::scheduler::current;
use crate::klib::{InitedCell, SpinLock};

use super::super::cpu::core_count;
use super::pagetable::{
    ENTRIES_PER_TABLE, FLUSH_RANGE_MAX_PAGES, PageTable, local_flush_all, local_flush_range, remote_flush_range,
};
use super::pte::PTETable;

unsafe extern "C" {
    static __riscv_kpgtable_root: usize;
}

static KERNEL_PAGETABLE: InitedCell<SpinLock<PageTable>> = InitedCell::uninit();
static KERNEL_PAGETABLE_ROOT: InitedCell<usize> = InitedCell::uninit();
const USER_ROOT_ENTRIES: usize = ENTRIES_PER_TABLE / 2;

pub(super) fn is_shared_kernel_root(index: usize) -> bool {
    index >= USER_ROOT_ENTRIES
}

fn prepare_shared_kernel_root_entries(pagetable: &mut PageTable) {
    const ROOT_ENTRY_SIZE: usize = 1 << 30;

    let mut addr = KERNEL_MMIO_START;
    while addr < KERNEL_STACK_ARENA_END {
        let index = (addr / ROOT_ENTRY_SIZE) & (ENTRIES_PER_TABLE - 1);
        pagetable.ensure_root_entry(index);
        addr += ROOT_ENTRY_SIZE;
    }
}

#[unsafe(link_section = ".text.init")]
pub fn init() {
    KERNEL_PAGETABLE_ROOT.init(unsafe { __riscv_kpgtable_root });

    let mut pagetable = PageTable::from_root(*KERNEL_PAGETABLE_ROOT);

    prepare_shared_kernel_root_entries(&mut pagetable);

    KERNEL_PAGETABLE.init(SpinLock::new(pagetable, "KERNEL_PAGETABLE"));
}

pub(super) fn install_shared_kernel_mappings(pagetable: &mut PageTable) {
    let kernel_pagetable = KERNEL_PAGETABLE.lock();
    let kernel_root = PTETable::new(kernel_pagetable.root as *mut usize);
    let mut user_root = PTETable::new(pagetable.root as *mut usize);

    for index in USER_ROOT_ENTRIES..ENTRIES_PER_TABLE {
        let pte = kernel_root.get(index);
        if !pte.is_valid() {
            continue;
        }

        debug_assert!(
            !user_root.get(index).is_valid(),
            "user page table should not map shared kernel root entry {index}"
        );
        user_root.set(index, pte);
    }
}

/// Every hart other than the current one, as an SBI hart mask with base 0.
/// Hart IDs are contiguous from 0 (see `setup_all_cores`).
fn other_harts_mask() -> usize {
    let all = 1usize
        .checked_shl(core_count().try_into().expect("hart count does not fit in u32"))
        .expect("hart count exceeds TLB CPU mask width")
        .wrapping_sub(1);
    all & !(1usize << current::hart_id())
}

/// Ranged local invalidation that degrades to a full local flush when the
/// range is large, so the per-page fence count stays bounded.
fn local_flush_range_or_all(kstart: usize, page_count: usize) {
    if page_count <= FLUSH_RANGE_MAX_PAGES {
        local_flush_range(kstart, page_count);
    } else {
        local_flush_all();
    }
}

/// Write the mappings into the kernel page table; returns the page count.
/// TLB invalidation is the caller's responsibility.
fn map_kernel_pages_no_flush(kstart: usize, pstarts: impl IntoIterator<Item = usize>, perm: MapPerm) -> usize {
    let mut pagetable = KERNEL_PAGETABLE.lock();
    let mut kaddr = kstart;
    let mut page_count = 0;
    for paddr in pstarts {
        pagetable.mmap_kernel(kaddr, paddr, perm);
        kaddr += PGSIZE;
        page_count += 1;
    }
    page_count
}

pub fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) {
    map_kernel_pages(kstart, (0..size).step_by(PGSIZE).map(|offset| pstart + offset), perm);
}

/// Map kernel pages and perform a full global TLB shootdown.
///
/// Required for callers that change existing translations (e.g. the kmodule
/// loader flips permissions on direct-map pages, and MMIO drivers repoint
/// direct-map VAs at device memory): every hart may hold the old translation.
/// Callers mapping previously-unmapped VAs should use
/// `map_fresh_kernel_pages` instead.
pub fn map_kernel_pages(kstart: usize, pstarts: impl IntoIterator<Item = usize>, perm: MapPerm) {
    if map_kernel_pages_no_flush(kstart, pstarts, perm) > 0 {
        // Outside the KERNEL_PAGETABLE lock: the fence inside flush_tlb_all
        // publishes the PTE writes, and the flush only needs to cover this
        // caller's own update.
        flush_tlb_all();
    }
}

/// Map kernel pages into a virtual address range that is guaranteed to be
/// unmapped on every hart, skipping the remote TLB shootdown.
///
/// The caller must ensure no hart can hold a translation for the range. This
/// holds for VAs that were never mapped, and for recycled VAs whose previous
/// unmapping completed its global shootdown before the range was reused
/// (`unmap_kernel_addr` guarantees this before it returns). As with user
/// invalid->valid transitions (`mmap_raw`), no remote fence is needed; the
/// local ranged fence orders the PTE writes before this hart's first access
/// and drops any cached invalid entry.
pub fn map_fresh_kernel_pages(kstart: usize, pstarts: impl IntoIterator<Item = usize>, perm: MapPerm) {
    let page_count = map_kernel_pages_no_flush(kstart, pstarts, perm);
    if page_count > 0 {
        local_flush_range_or_all(kstart, page_count);
    }
}

pub fn get_kernel_satp() -> usize {
    KERNEL_PAGETABLE.lock().get_satp()
}

/// Unmap `[kstart, kstart + size)` from the kernel page table and invalidate
/// the range on every hart.
///
/// The PTEs are cleared under the KERNEL_PAGETABLE lock, but the TLB
/// shootdown runs after the lock is dropped so the synchronous SBI round trip
/// does not serialize unrelated kernel mappings. The shootdown is complete on
/// all harts when this function returns: only then may the caller recycle the
/// backing frames or reuse the virtual address range.
pub unsafe fn unmap_kernel_addr(kstart: usize, size: usize) {
    let kend = kstart + size;
    let page_count = size / PGSIZE;

    {
        let mut pagetable = KERNEL_PAGETABLE.lock();
        let mut kaddr = kstart;
        while kaddr < kend {
            pagetable.munmap_no_flush(kaddr).expect("page must be mapped");
            kaddr += PGSIZE;
        }
    }

    // Kernel mappings are global (G bit), and both the ranged local fence
    // (sfence.vma vaddr, x0) and the SBI ranged remote fence invalidate
    // global entries because no ASID is specified. The remote side is a
    // single SBI call regardless of range size (the SBI implementation
    // widens large ranges to a full flush itself).
    local_flush_range_or_all(kstart, page_count);
    remote_flush_range(other_harts_mask(), kstart, page_count);
}

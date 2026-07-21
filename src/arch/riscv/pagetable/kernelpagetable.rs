use crate::arch::flush_tlb_all;
use crate::arch::riscv::{KERNEL_MMIO_START, KERNEL_STACK_ARENA_END, PGSIZE};
use crate::kernel::mm::MapPerm;
use crate::klib::{InitedCell, SpinLock};

use super::pagetable::{ENTRIES_PER_TABLE, PageTable};
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

pub fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) {
    map_kernel_pages(kstart, (0..size).step_by(PGSIZE).map(|offset| pstart + offset), perm);
}

pub fn map_kernel_pages(kstart: usize, pstarts: impl IntoIterator<Item = usize>, perm: MapPerm) {
    let mut pagetable = KERNEL_PAGETABLE.lock();
    let mut kaddr = kstart;
    let mut mapped = false;
    for paddr in pstarts {
        pagetable.mmap_kernel(kaddr, paddr, perm);
        kaddr += PGSIZE;
        mapped = true;
    }

    if mapped {
        flush_tlb_all();
    }
}

pub fn get_kernel_satp() -> usize {
    KERNEL_PAGETABLE.lock().get_satp()
}

pub unsafe fn unmap_kernel_addr(kstart: usize, size: usize) {
    let mut kaddr = kstart;
    let kend = kstart + size;

    let mut pagetable = KERNEL_PAGETABLE.lock();
    while kaddr < kend {
        pagetable.munmap_no_flush(kaddr).expect("page must be mapped");
        kaddr += PGSIZE;
    }

    flush_tlb_all();
}

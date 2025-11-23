use crate::kernel::mm::MapPerm;
use crate::arch::riscv::{PGSIZE, TRAMPOLINE_BASE};
use crate::arch::PageTableTrait;
use crate::klib::{InitedCell, SpinLock};
use crate::kinfo;

use super::pagetable::PageTable;

unsafe extern "C" {
    static __trampoline_start: u8;
    static __riscv_kpgtable_root: usize;
    static __riscv_kaddr_offset: usize;
}

static KERNEL_PAGETABLE: InitedCell<SpinLock<PageTable>> = InitedCell::uninit();
static KERNEL_SATP: InitedCell<usize> = InitedCell::uninit();

#[unsafe(link_section = ".text.init")]
pub fn init() {
    kinfo!("root=0x{:x}, offset=0x{:x}", unsafe { __riscv_kpgtable_root }, core::ptr::addr_of!(__riscv_kaddr_offset) as usize);
    let mut pagetable = PageTable::from_root(unsafe { __riscv_kpgtable_root });

    pagetable.mmap(
        TRAMPOLINE_BASE,
        core::ptr::addr_of!(__trampoline_start) as usize,
        MapPerm::R | MapPerm::X
    );

    KERNEL_SATP.init(pagetable.get_satp());
    KERNEL_PAGETABLE.init(SpinLock::new(pagetable));
}

pub fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) {
    let mut kaddr = kstart;
    let kend = kstart + size;
    
    let mut pagetable = KERNEL_PAGETABLE.lock();
    let mut paddr = pstart;
    while kaddr < kend {
        pagetable.mmap_paddr(kaddr, paddr, perm);
        kaddr += PGSIZE;
        paddr += PGSIZE;
    }

    unsafe {
        core::arch::asm!(
            "sfence.vma zero, zero"
        )
    }
}

pub unsafe fn unmap_kernel_addr(kstart: usize, size: usize) {
    let mut kaddr = kstart;
    let kend = kstart + size;
    
    let mut pagetable = KERNEL_PAGETABLE.lock();
    while kaddr < kend {
        pagetable.munmap(kaddr);
        kaddr += PGSIZE;
    }

    unsafe {
        core::arch::asm!(
            "sfence.vma zero, zero"
        )
    }
}

pub fn get_kernel_satp() -> usize {
    *KERNEL_SATP
}
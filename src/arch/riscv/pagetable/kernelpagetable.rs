use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use crate::kernel::mm::MapPerm;
use crate::arch::riscv::PGSIZE;
use crate::arch::PageTableTrait;
use crate::platform::config::TRAMPOLINE_BASE;
use crate::kinfo;

use super::pagetable::PageTable;

unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
    static __stack_start: u8;
    static __stack_end: u8;
    static __trampoline_start: u8;
}

unsafe extern "C" {
    static __riscv_kpgtable_root: usize;
    static __riscv_kaddr_offset: usize;
}

struct KernelPageTable(UnsafeCell<MaybeUninit<PageTable>>);

impl KernelPageTable {
    const fn new() -> Self {
        KernelPageTable(UnsafeCell::new(MaybeUninit::zeroed()))
    }

    fn init(&self, pagetable: PageTable) {
        unsafe { (*self.0.get()).write(pagetable); }
    }
    
    fn get_mut(&self) -> &mut PageTable {
        unsafe { (*self.0.get()).assume_init_mut() }
    }
}

unsafe impl Sync for KernelPageTable {}

struct KernelPageTableSatp(UnsafeCell<usize>);

unsafe impl Send for KernelPageTableSatp {}
unsafe impl Sync for KernelPageTableSatp {}

impl KernelPageTableSatp {
    const fn new() -> Self {
        KernelPageTableSatp(UnsafeCell::new(0))
    }
    
    fn set(&self, satp: usize) {
        unsafe { *self.0.get() = satp; }
    }
}

static KERNEL_PAGETABLE: KernelPageTable = KernelPageTable::new();
static KERNEL_SATP: KernelPageTableSatp = KernelPageTableSatp::new();

#[unsafe(link_section = ".text.init")]
pub fn init() {
    kinfo!("root=0x{:x}, offset=0x{:x}", unsafe { __riscv_kpgtable_root }, core::ptr::addr_of!(__riscv_kaddr_offset) as usize);
    let mut pagetable = PageTable::from_root(unsafe { __riscv_kpgtable_root });

    pagetable.mmap(
        TRAMPOLINE_BASE,
        core::ptr::addr_of!(__trampoline_start) as usize,
        MapPerm::R | MapPerm::X
    );

    KERNEL_SATP.set(pagetable.get_satp());
    KERNEL_PAGETABLE.init(pagetable);
}

pub fn map_kernel_addr(kstart: usize, pstart: usize, size: usize, perm: MapPerm) {
    let mut kaddr = kstart;
    let kend = kstart + size;
    
    let pagetable = KERNEL_PAGETABLE.get_mut();
    let mut paddr = pstart;
    while kaddr < kend {
        pagetable.mmap_paddr(kaddr, paddr, perm);
        kaddr += PGSIZE;
        paddr += PGSIZE;
    }

    unsafe {
        asm!(
            "sfence.vma zero, zero"
        )
    }
}

pub fn get_kernel_satp() -> usize {
    KERNEL_PAGETABLE.get_mut().get_satp()
}
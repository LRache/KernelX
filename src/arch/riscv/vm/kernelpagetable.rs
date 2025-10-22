use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use crate::kernel::mm::MapPerm;
use crate::arch::riscv::{PageTable, PGSIZE};
use crate::arch::{self, kaddr_to_paddr, PageTableTrait};
use crate::platform::config::TRAMPOLINE_BASE;
use crate::{kinfo, println};

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

fn map_kernel_range(start: *const u8, end: *const u8, perm: MapPerm) {
    println!("Mapping kernel range: 0x{:x} - 0x{:x} with perm: {:?}", 
             start as usize, end as usize, perm);
    let mut vaddr = start as usize;
    let end = end as usize;
    
    let pagetable = KERNEL_PAGETABLE.get_mut();
    while vaddr < end {
        let paddr = kaddr_to_paddr(vaddr);
        pagetable.mmap_paddr(vaddr, paddr, perm);
        vaddr += PGSIZE;
    }
}

pub fn init() {
    kinfo!("root=0x{:x}, offset=0x{:x}", unsafe { __riscv_kpgtable_root }, core::ptr::addr_of!(__riscv_kaddr_offset) as usize);
    let mut pagetable = PageTable::from_root(unsafe { __riscv_kpgtable_root });

    // map_kernel_range(
    //     core::ptr::addr_of!(__text_start), 
    //     core::ptr::addr_of!(__text_end), 
    //     MapPerm::R | MapPerm::X
    // );

    // map_kernel_range(
    //     core::ptr::addr_of!(__rodata_start), 
    //     core::ptr::addr_of!(__rodata_end), 
    //     MapPerm::R
    // );

    // map_kernel_range(
    //     core::ptr::addr_of!(__data_start), 
    //     core::ptr::addr_of!(__bss_end), 
    //     MapPerm::R | MapPerm::W
    // );

    // map_kernel_range(
    //     core::ptr::addr_of!(__stack_start), 
    //     core::ptr::addr_of!(__stack_end), 
    //     MapPerm::R | MapPerm::W
    // );

    // map_kernel_range(
    //     core::ptr::addr_of!(__stack_end), 
    //     (KERNEL_PMEM_TOP + KADDR_OFFSET) as *const u8, 
    //     MapPerm::R | MapPerm::W
    // );

    pagetable.mmap(
        TRAMPOLINE_BASE,
        core::ptr::addr_of!(__trampoline_start) as usize,
        MapPerm::R | MapPerm::X
    );

    pagetable.mmap_paddr(
        0x10001000, 
        0x10001000, 
        MapPerm::R | MapPerm::W
    );

    // pagetable.apply(); 

    KERNEL_SATP.set(pagetable.get_satp());
    KERNEL_PAGETABLE.init(pagetable);
}

pub fn get_kernel_satp() -> usize {
    KERNEL_PAGETABLE.get_mut().get_satp()
}
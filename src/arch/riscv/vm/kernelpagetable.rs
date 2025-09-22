use crate::arch::pagetable::PageTable;
use crate::arch::PageTableTrait;
use crate::arch;
use crate::kernel::mm::MapPerm;
use crate::platform::config::{KERNEL_PMEM_TOP, KERNEL_VADDR_OFFSET, TRAMPOLINE_BASE};
use crate::println;
use core::cell::UnsafeCell;

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
    static __heap_start: u8;
    static __heap_end: u8;
    static __trampoline_start: u8;
}

struct KernelPageTable(UnsafeCell<PageTable>);


unsafe impl Send for KernelPageTable {}
unsafe impl Sync for KernelPageTable {}

impl KernelPageTable {
    const fn new() -> Self {
        KernelPageTable(UnsafeCell::new(PageTable::new()))
    }
    
    fn get_mut(&self) -> &mut PageTable {
        unsafe { &mut *self.0.get() }
    }
    
    fn get(&self) -> &PageTable {
        unsafe { &*self.0.get() }
    }
}

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
        let paddr = vaddr;
        pagetable.mmap(vaddr, paddr, perm);
        vaddr += arch::PGSIZE;
    }
}


pub fn init() {
    let pagetable = KERNEL_PAGETABLE.get_mut();
    pagetable.create();

    map_kernel_range(
        core::ptr::addr_of!(__text_start), 
        core::ptr::addr_of!(__text_end), 
        MapPerm::R | MapPerm::X
    );

    map_kernel_range(
        core::ptr::addr_of!(__rodata_start), 
        core::ptr::addr_of!(__rodata_end), 
        MapPerm::R
    );

    map_kernel_range(
        core::ptr::addr_of!(__data_start), 
        core::ptr::addr_of!(__bss_end), 
        MapPerm::R | MapPerm::W
    );

    map_kernel_range(
        core::ptr::addr_of!(__stack_start), 
        core::ptr::addr_of!(__stack_end), 
        MapPerm::R | MapPerm::W
    );

    map_kernel_range(
        core::ptr::addr_of!(__heap_start), 
        core::ptr::addr_of!(__heap_end), 
        MapPerm::R | MapPerm::W
    );

    map_kernel_range(
        core::ptr::addr_of!(__heap_end), 
        (KERNEL_PMEM_TOP + KERNEL_VADDR_OFFSET) as *const u8, 
        MapPerm::R | MapPerm::W
    );

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

    pagetable.apply(); 

    KERNEL_SATP.set(pagetable.get_satp());
}

pub fn get_kernel_satp() -> usize {
    KERNEL_PAGETABLE.get().get_satp()
}
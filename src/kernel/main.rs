use crate::kernel::event::timer;
use crate::kernel::{config, mm, scheduler, task};
use crate::driver;
use crate::fs;
use crate::arch;
use crate::klib::kalloc;
use crate::kinfo;

pub fn fini() {
    kinfo!("Deinitializing KernelX...");
    
    fs::fini();
    
    kinfo!("KernelX deinitialized successfully!");
}

fn free_init() {
    unsafe extern "C" {
        static __init_start: u8;
        static __init_end:   u8;
    }

    let kstart = core::ptr::addr_of!(__init_start) as usize;
    let kend = core::ptr::addr_of!(__init_end) as usize;
    debug_assert!(kstart % arch::PGSIZE == 0);
    debug_assert!(kend % arch::PGSIZE == 0);

    let mut kaddr = core::ptr::addr_of!(__init_start) as usize;
    while kaddr < kend {
        mm::page::free(kaddr);
        kaddr += arch::PGSIZE;
    }

    kinfo!("Freed init section memory {:#x} bytes", kend - kstart);
}

#[unsafe(no_mangle)]
pub extern "C" fn main(hartid: usize, heap_start: usize) -> ! {
    kinfo!("Welcome to KernelX!");
    
    kinfo!("Initializing KernelX...");
    
    kalloc::init(heap_start, config::KERNEL_HEAP_SIZE);
    mm::init(heap_start + config::KERNEL_HEAP_SIZE);
    driver::init();
    arch::init();
    arch::scan_device();

    kinfo!("Welcome to KernelX!");

    fs::init();

    task::init();
    timer::init();

    free_init();
    
    kinfo!("KernelX initialized successfully!");
    
    scheduler::run_tasks(hartid as u8);
}

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

#[unsafe(no_mangle)]
pub extern "C" fn main(hartid: usize, heap_start: usize) -> ! {
    kinfo!("Welcome to KernelX!");
    
    kinfo!("Initializing KernelX...");
    
    kalloc::init(heap_start, config::KERNEL_HEAP_SIZE);
    mm::init(heap_start + config::KERNEL_HEAP_SIZE);
    driver::init();
    arch::init();
    arch::scan_device();
    fs::init();

    task::init();
    timer::init();
    
    kinfo!("KernelX initialized successfully!");
    
    scheduler::run_tasks(hartid as u8);
}

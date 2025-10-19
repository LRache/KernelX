use crate::kernel::event::timer;
use crate::kernel::{mm, task, scheduler};
use crate::driver;
use crate::fs;
use crate::arch;
use crate::klib::kalloc;
use crate::{kdebug, kinfo};

unsafe extern "C"{
    static __bss_start: u8;
    static __bss_end: u8;
}

fn clean_bss() -> () {
    let bss_start = core::ptr::addr_of!(__bss_start) as usize;
    let bss_end = core::ptr::addr_of!(__bss_end) as usize;
    let bss_size = bss_end - bss_start;

    unsafe { core::slice::from_raw_parts_mut(bss_start as *mut u8, bss_size) }.fill(0);

    kdebug!("Clean BSS: 0x{:x} - 0x{:x} (size: {} bytes) successfully initialized", 
             bss_start, bss_end, bss_size);
}

pub fn fini() {
    kinfo!("Deinitializing KernelX...");
    
    fs::fini();
    
    kinfo!("KernelX deinitialized successfully!");
}

#[unsafe(no_mangle)]
pub extern "C" fn main(hartid: usize) -> ! {
    kinfo!("Welcome to KernelX!");
    
    kinfo!("Initializing KernelX...");
    
    clean_bss();
    
    kalloc::init();
    mm::init();
    arch::init();
    driver::init();
    fs::init();

    panic!();

    task::init();
    timer::init();
    
    kinfo!("KernelX initialized successfully!");
    
    scheduler::run_tasks(hartid as u8);
}

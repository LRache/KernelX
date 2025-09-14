use crate::driver;
use crate::fs;
use crate::arch;
use crate::klib::kalloc;
use crate::kdebug;
use crate::kinfo;
use crate::kernel::{mm, task, scheduler};
use crate::println;

unsafe extern "C"{
    static __bss_start: u8;
    static __bss_end: u8;
}

fn clean_bss() -> () {
    let bss_start = core::ptr::addr_of!(__bss_start) as usize;
    let bss_end = core::ptr::addr_of!(__bss_end) as usize;
    let bss_size = bss_end - bss_start;
    
    unsafe {
        let bss_slice = core::slice::from_raw_parts_mut(
            bss_start as *mut u8, 
            bss_size
        );
        bss_slice.fill(0);
    }

    kdebug!("Clean BSS: 0x{:x} - 0x{:x} (size: {} bytes) successfully initialized", 
             bss_start, bss_end, bss_size);
}

fn init() {
    kinfo!("Initializing KernelX...");
    
    clean_bss();
    
    kalloc::init();
    mm::page::init();
    arch::init();
    driver::init();
    fs::init();

    task::init();
    
    kinfo!("KernelX initialized successfully!");
}

#[unsafe(no_mangle)]
pub extern "C" fn main(_hartid: usize) -> ! {
    println!("Welcome to KernelX!");
    init();
    
    scheduler::run_tasks();
}

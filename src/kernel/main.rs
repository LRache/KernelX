use alloc::collections::btree_map::BTreeMap;

use crate::kernel::event::timer;
use crate::kernel::config;
use crate::kernel::kthread;
use crate::kernel::mm;
use crate::kernel::scheduler;
use crate::kernel::scheduler::current;
use crate::kernel::task;
use crate::fs::vfs;
use crate::arch;
use crate::fs;
use crate::driver;
use crate::klib::{kalloc, InitedCell};
use crate::{kinfo, println};

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

static BOOT_ARGS: InitedCell<BTreeMap<&'static str, &'static str>> = InitedCell::uninit();

#[unsafe(link_section = ".text.init")]
pub fn parse_boot_args(bootargs: &'static str) {
    let mut bootargs_map = BTreeMap::new();
    for arg in bootargs.split_whitespace() {
        if let Some((key, value)) = arg.split_once('=') {
            bootargs_map.insert(key, value);
            kinfo!("bootarg: {}={}", key, value);
        } else {
            bootargs_map.insert(arg, "");
            kinfo!("bootarg: {}", arg);
        }
    }

    BOOT_ARGS.init(bootargs_map);
}

#[unsafe(no_mangle)]
extern "C" fn main(hartid: usize, heap_start: usize, memory_top: usize) -> ! {
    kinfo!("Welcome to KernelX!");
    
    kinfo!("Initializing KernelX...");
    
    kalloc::init(heap_start, config::KERNEL_HEAP_SIZE);
    mm::init(heap_start + config::KERNEL_HEAP_SIZE, memory_top);
    driver::init();
    arch::init();
    arch::scan_device();

    kinfo!("Welcome to KernelX!");

    kinfo!("Frame space: {:#x} - {:#x}, total {:#x}", heap_start + config::KERNEL_HEAP_SIZE, memory_top, memory_top - (heap_start + config::KERNEL_HEAP_SIZE));

    fs::init();
    fs::mount_init_fs(
        BOOT_ARGS.get("root").unwrap_or(&config::DEFAULT_BOOT_ROOT), 
        BOOT_ARGS.get("rootfstype").unwrap_or(&config::DEFAULT_BOOT_ROOT_FSTYPE)
    );

    task::create_initprocess(
        BOOT_ARGS.get("init").unwrap_or(&config::DEFAULT_INITPATH),
        BOOT_ARGS.get("initcwd").unwrap_or(&config::DEFAULT_INITCWD)
    );
    
    timer::init();

    // kthread::spawn(kthread_test);

    // free_init();
    
    kinfo!("KernelX initialized successfully!");
    
    scheduler::run_tasks(hartid as u8);
}

fn kthread_test() {
    let mut sec = 0;
    loop {
        timer::add_timer(current::task().clone(), core::time::Duration::from_secs(1));
        current::block("sleep");
        sec += 1;
        kinfo!("kthread_test: {} seconds elapsed", sec);
    }
}

pub fn exit() -> ! {
    vfs::sync_all().unwrap_or_else(|e| {
        println!("Failed to sync filesystem: {:?}", e);
    });
    
    driver::chosen::kpmu::shutdown();
}

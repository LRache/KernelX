use alloc::collections::BTreeMap;

use crate::kernel::event::timer;
use crate::kernel::config;
use crate::kernel::mm;
use crate::kernel::scheduler;
use crate::kernel::task;
use crate::arch;
use crate::fs;
use crate::driver;
use crate::klib::{kalloc, InitedCell};
use crate::kinfo;
use crate::print;

#[allow(dead_code)]
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

const LOGO: &str = r#"
  _  __                               _  __  __
 | |/ /   ___   _ __   _ __     ___  | | \ \/ /
 | ' /   / _ \ | '__| | '_ \   / _ \ | |  \  / 
 | . \  |  __/ | |    | | | | |  __/ | |  /  \ 
 |_|\_\  \___| |_|    |_| |_|  \___| |_| /_/\_\
"#;

#[unsafe(no_mangle)]
extern "C" fn main(hartid: usize, heap_start: usize, memory_top: usize) {
    kinfo!("Welcome to KernelX!");
    
    kinfo!("Initializing KernelX...");
    
    kalloc::init(heap_start, config::KERNEL_HEAP_SIZE);
    mm::init(heap_start + config::KERNEL_HEAP_SIZE, memory_top);
    driver::init();
    arch::init();
    
    fs::init();
    arch::scan_device();
    
    #[cfg(feature = "swap-memory")]
    mm::swappable::init();
    
    fs::mount_init_fs(
        BOOT_ARGS.get("root").unwrap_or(&config::DEFAULT_BOOT_ROOT), 
        BOOT_ARGS.get("rootfstype").unwrap_or(&config::DEFAULT_BOOT_ROOT_FSTYPE)
    );

    task::create_initprocess(
        BOOT_ARGS.get("init").unwrap_or(&config::DEFAULT_INITPATH),
        BOOT_ARGS.get("initcwd").unwrap_or(&config::DEFAULT_INITCWD),
        BOOT_ARGS.get("initargs").unwrap_or(&""),
        BOOT_ARGS.get("tty").unwrap_or(&config::DEFAULT_INITTTY),
    );

    driver::chosen::init(&BOOT_ARGS);
    
    timer::init();

    #[cfg(feature = "swap-memory")]
    {
        crate::kernel::mm::swappable::spawn_kswapd();
    }
    
    kinfo!("KernelX initialized successfully!");

    print!("{}{}{}\n", "\x1b[94m", LOGO, "\x1b[0m");

    kinfo!("Welcome to use KernelX!");

    arch::setup_all_cores(hartid);

    kentry(hartid);
}

#[unsafe(no_mangle)]
extern "C" fn kentry(hartid: usize) -> ! {
    kinfo!("Hart {} booted.", hartid);
    arch::set_next_time_event_us(10000);
    arch::enable_timer_interrupt();
    arch::enable_device_interrupt(hartid);
    
    scheduler::run_tasks(hartid);
}

pub fn exit() -> ! {
    fs::fini();

    #[cfg(feature = "swap-memory")]
    crate::kernel::mm::swappable::fini();
    
    driver::chosen::kpmu::shutdown();
}

use core::sync::atomic::{AtomicBool, Ordering};

use alloc::collections::BTreeMap;

use crate::fs::vfs;
use crate::kernel::event::timer;
use crate::kernel::scheduler::{Processor, Task};
use crate::kernel::{config, kthread, mm, scheduler, task};
use crate::klib::{InitedCell, kalloc};
use crate::{arch, driver, fs, kinfo, net, print};

#[allow(dead_code)]
fn free_init() {
    unsafe extern "C" {
        static __init_start: u8;
        static __init_end: u8;
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

fn kinit() {
    timer::init();

    mm::vdso::init();

    fs::mount_init_fs(
        BOOT_ARGS.get("root").unwrap_or(&config::DEFAULT_BOOT_ROOT_DEVICE),
        BOOT_ARGS.get("rootfstype").unwrap_or(&config::DEFAULT_BOOT_ROOT_FSTYPE),
    );

    driver::chosen::init(&BOOT_ARGS);

    net::configure();

    task::create_initprocess(
        BOOT_ARGS.get("init").unwrap_or(&config::DEFAULT_INITPATH),
        BOOT_ARGS.get("initcwd").unwrap_or(&config::DEFAULT_INITCWD),
        BOOT_ARGS.get("initargs").unwrap_or(&""),
        BOOT_ARGS.get("tty").unwrap_or(&config::DEFAULT_INITTTY),
    );

    #[cfg(feature = "watchdog")]
    kthread::spawn(scheduler::watchdog::kwatchdog);
    kthread::spawn(vfs::inode_cache_reclaimer);

    kinfo!("KernelX initialized successfully!");

    print!("{}{}{}\n", "\x1b[94m", LOGO, "\x1b[0m");

    kinfo!("Welcome to use KernelX!");

    crate::kernel::mm::swappable::spawn_kswapd();
}

#[unsafe(link_section = ".text.init")]
pub fn parse_boot_args(bootargs: &'static str) {
    if bootargs.is_empty() {
        return parse_boot_args(config::DEFAULT_BOOTARGS);
    }
    let mut bootargs_map = BTreeMap::new();

    let mut insert_arg = |arg: &'static str| {
        let arg = arg.trim_matches('"');
        if arg.is_empty() {
            return;
        }
        if let Some((key, value)) = arg.split_once('=') {
            let value = value.trim_matches('"');
            bootargs_map.insert(key, value);
            kinfo!("bootarg: {}={}", key, value);
        } else {
            let arg = arg.trim_matches('"');
            if arg.is_empty() {
                return;
            }
            bootargs_map.insert(arg, "");
            kinfo!("bootarg: {}", arg);
        }
    };

    let mut start = None;
    let mut in_quotes = false;
    for (i, c) in bootargs.char_indices() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                if start.is_none() {
                    start = Some(i);
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if let Some(begin) = start.take() {
                    insert_arg(&bootargs[begin..i]);
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(i);
                }
            }
        }
    }

    if let Some(begin) = start {
        insert_arg(&bootargs[begin..]);
    }

    BOOT_ARGS.init(bootargs_map);
}

static FIRST_BOOTED: AtomicBool = AtomicBool::new(true);

const LOGO: &str = r#"
  _  __                               _  __  __
 | |/ /   ___   _ __   _ __     ___  | | \ \/ /
 | ' /   / _ \ | '__| | '_ \   / _ \ | |  \  / 
 | . \  |  __/ | |    | | | | |  __/ | |  /  \ 
 |_|\_\  \___| |_|    |_| |_|  \___| |_| /_/\_\
"#;

#[unsafe(no_mangle)]
extern "C" fn main(hartid: usize, bootstrap_end: usize, mem_regions: *const mm::MemRegion, mem_region_count: usize) {
    let processor = Processor::new(hartid);
    arch::set_percpu_data(&processor as *const Processor as usize);

    if FIRST_BOOTED.swap(false, Ordering::SeqCst) {
        // SAFETY: The architecture entry code reserves one page for the memory
        // region array and passes the exact number of initialized entries.
        unsafe { mm::init(bootstrap_end, mem_regions, mem_region_count) };
        kalloc::init();
        mm::swappable::init();
        arch::init();
        fs::init();
        driver::init();
        arch::scan_device();
        #[cfg(feature = "swap-memory")]
        {
            let swap_driver = BOOT_ARGS.get("swap").and_then(|name| {
                let driver = driver::get_block_driver(name);
                if driver.is_none() {
                    crate::kwarn!("Anonymous swap driver '{}' not found; swap is disabled.", name);
                }
                driver
            });
            mm::swappable::init_anonymous_swap(swap_driver);
        }
        #[cfg(not(feature = "swap-memory"))]
        if let Some(name) = BOOT_ARGS.get("swap") {
            crate::kwarn!(
                "Anonymous swap driver '{}' was specified, but swap-memory support is disabled.",
                name
            );
        }
        net::init();

        let inittask = kthread::spawn(kinit);
        debug_assert!(inittask.tid() == 0);

        arch::setup_all_cores(hartid);
    }

    // kinfo!("Welcome to KernelX!");
    // kinfo!("Initializing KernelX...");

    arch::set_next_time_event_us(10000);
    arch::enable_timer_interrupt();
    arch::enable_device_interrupt(hartid);

    scheduler::run_tasks(hartid);
}

pub fn deinit() {
    fs::fini();

    crate::kernel::mm::swappable::fini();
}

pub fn exit() -> ! {
    driver::chosen::kpmu::shutdown();
}

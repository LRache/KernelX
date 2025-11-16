use core::panic::PanicInfo;

use crate::driver;
use crate::fs::vfs;
use crate::println;

pub const COLOR_RESET: &str = "\x1b[0m";
pub const COLOR_RED: &str = "\x1b[31m";
pub const COLOR_YELLOW: &str = "\x1b[33m";
pub const COLOR_BLUE: &str = "\x1b[34m";
pub const COLOR_GREEN: &str = "\x1b[32m";
pub const COLOR_CYAN: &str = "\x1b[36m";
pub const COLOR_BOLD: &str = "\x1b[1m";

#[cfg(feature = "log-warn")]
#[macro_export]
macro_rules! kwarn {
    ($($arg:tt)*) => {
        $crate::println!(
            "{}{}[{}]{} {} (tid={}) @ {}:{}:{}{}",
            $crate::klib::klog::COLOR_BOLD,
            $crate::klib::klog::COLOR_YELLOW,
            "WARN",
            $crate::klib::klog::COLOR_RESET,
            format_args!($($arg)*),
            $crate::kernel::scheduler::current::tid(),
            file!(),
            line!(),
            column!(),
            $crate::klib::klog::COLOR_RESET
        )
    };
}

#[cfg(not(feature = "log-warn"))]
#[macro_export]
macro_rules! kwarn {
    () => {};
    ($($arg:tt)*) => {};
}

#[cfg(feature = "log-info")]
#[macro_export]
macro_rules! kinfo {
    ($($arg:tt)*) => {
        $crate::println!(
            "{}{}[{}]{} {} (tid={}) @ {}:{}:{}{}",
            $crate::klib::klog::COLOR_BOLD,
            $crate::klib::klog::COLOR_BLUE,
            "INFO",
            $crate::klib::klog::COLOR_RESET,
            format_args!($($arg)*),
            $crate::kernel::scheduler::current::tid(),
            file!(),
            line!(),
            column!(),
            $crate::klib::klog::COLOR_RESET
        )
    }
}

#[cfg(not(feature = "log-info"))]
#[macro_export]
macro_rules! kinfo {
    () => {};
    ($($arg:tt)*) => {};
}

#[cfg(feature = "log-debug")]
#[macro_export]
macro_rules! kdebug {
    ($($arg:tt)*) => {
        $crate::println!(
            "{}{}[{}]{} {} (tid={}) @ {}:{}:{}{}",
            $crate::klib::klog::COLOR_BOLD,
            $crate::klib::klog::COLOR_CYAN,
            "DEBUG",
            $crate::klib::klog::COLOR_RESET,
            format_args!($($arg)*),
            $crate::kernel::scheduler::current::tid(),
            file!(),
            line!(),
            column!(),
            $crate::klib::klog::COLOR_RESET
        );
    }
}

#[cfg(not(feature = "log-debug"))]
#[macro_export]
macro_rules! kdebug {
    ($($arg:tt)*) => {};
}

#[cfg(feature = "log-trace")]
#[macro_export]
macro_rules! ktrace {
    ($($arg:tt)*) => {
        $crate::println!(
            "{}{}[{}]{} {} (tid={}) @ {}:{}:{}{}",
            $crate::klib::klog::COLOR_BOLD,
            $crate::klib::klog::COLOR_GREEN,
            "TRACE",
            $crate::klib::klog::COLOR_RESET,
            format_args!($($arg)*),
            $crate::kernel::scheduler::current::tid(),
            file!(),
            line!(),
            column!(),
            $crate::klib::klog::COLOR_RESET
        );
    };
}
#[cfg(not(feature = "log-trace"))]
#[macro_export]
macro_rules! ktrace {
    () => {};
    ($($arg:tt)*) => {};
}

#[panic_handler]
pub fn panic_handler(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        println!(
            "{}{}[{}]{} {} (tid={}) @ {}:{}{}",
            COLOR_BOLD,
            COLOR_RED,
            "PANIC",
            COLOR_RESET,
            info.message(),
            crate::kernel::scheduler::current::tid(),
            location.file(),
            location.line(),
            COLOR_RESET
        );
    } else {
        println!(
            "{}{}[{}]{} Unknown location - {}{}",
            COLOR_BOLD,
            COLOR_RED,
            "PANIC",
            COLOR_RESET,
            info.message(),
            COLOR_RESET
        );
    }

    vfs::sync_all().unwrap_or_else(|e| {
        println!("Failed to sync filesystem: {:?}", e);
    });

    driver::chosen::kpmu::shutdown();
}
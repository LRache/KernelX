use core::panic::PanicInfo;

use crate::kernel::exit;
use crate::klib::backtrace;
use crate::println;

pub const COLOR_RESET: &str = "\x1b[0m";
pub const COLOR_RED: &str = "\x1b[31m";
pub const COLOR_YELLOW: &str = "\x1b[33m";
pub const COLOR_BLUE: &str = "\x1b[34m";
pub const COLOR_GREEN: &str = "\x1b[32m";
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

    // SAFETY: panic 时已无法恢复，栈帧在 force-frame-pointers=yes 下合法
    backtrace::print_backtrace();

    exit();
}

/// C 代码通过 kpanic() / kassert() 触发内核 panic（见 clib/include/klib/klib.h）
#[unsafe(no_mangle)]
pub extern "C" fn rust_kpanic(file: *const u8, line: u32, msg: *const u8) -> ! {
    // SAFETY: C 传入的字符串在 panic 路径上保证有效
    let (file_str, msg_str) = unsafe {
        let file_len = (0..).take_while(|&i| *file.add(i) != 0).count();
        let msg_len  = (0..).take_while(|&i| *msg.add(i) != 0).count();
        (
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(file, file_len)),
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(msg,  msg_len)),
        )
    };

    println!(
        "{}{}[{}]{} {} @ {}:{}{}",
        COLOR_BOLD, COLOR_RED, "PANIC", COLOR_RESET,
        msg_str, file_str, line, COLOR_RESET
    );

    backtrace::print_backtrace();

    exit();
}
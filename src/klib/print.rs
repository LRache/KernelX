use core::fmt::Write;

use spin::{Mutex, MutexGuard};

use crate::arch;
use crate::driver::chosen::{kconsole, kdebug_console};
use crate::klib::dmesg;

static KLOG_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn try_lock_klog() -> Option<MutexGuard<'static, ()>> {
    KLOG_LOCK.try_lock()
}

pub struct Writer;
impl Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        kconsole::kputs(s);
        Ok(())
    }
}

/// 同时输出到控制台和 dmesg 缓冲区，仅供 kinfo!/kwarn!/kdebug!/ktrace! 使用
pub struct KlogWriter;
impl Write for KlogWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        kconsole::kputs(s);
        dmesg::dmesg_write(s);
        Ok(())
    }
}

pub fn _klog_print(args: core::fmt::Arguments) {
    let _guard = KLOG_LOCK.lock();
    let mut writer = KlogWriter;
    write_uptime_prefix(&mut writer);
    writer.write_fmt(args).unwrap();
}

fn write_uptime_prefix(writer: &mut impl Write) {
    let uptime = arch::try_uptime().unwrap_or_else(|| core::time::Duration::from_micros(0));
    writer
        .write_fmt(format_args!("[{:>5}.{:06}] ", uptime.as_secs(), uptime.subsec_micros()))
        .unwrap();
}

pub fn _kprint_debug(args: core::fmt::Arguments) {
    struct KPrintDebugWriter;
    impl Write for KPrintDebugWriter {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            kdebug_console::kputs(s);
            Ok(())
        }
    }

    let mut writer = KPrintDebugWriter;
    write_uptime_prefix(&mut writer);
    writer.write_fmt(args).unwrap();
}

pub fn _print(args: core::fmt::Arguments) {
    let mut writer = Writer;
    writer.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::klib::print::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kprint_debug {
    ($($arg:tt)*) => {
        $crate::klib::print::_kprint_debug(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! klogln {
    ($($arg:tt)*) => {
        $crate::klib::print::_klog_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

use core::fmt::Write;

use crate::driver::chosen::kconsole::kputs;
use crate::klib::dmesg;

pub struct Writer;
impl Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        kputs(s);
        Ok(())
    }
}

/// 同时输出到控制台和 dmesg 缓冲区，仅供 kinfo!/kwarn!/kdebug!/ktrace! 使用
pub struct KlogWriter;
impl Write for KlogWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        kputs(s);
        dmesg::dmesg_write(s);
        Ok(())
    }
}

pub fn _klog_print(args: core::fmt::Arguments) {
    let mut writer = KlogWriter;
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
macro_rules! klogln {
    ($($arg:tt)*) => {
        $crate::klib::print::_klog_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

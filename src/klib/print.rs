use core::fmt::Write;

use crate::driver::chosen::kconsole::kputs;

pub struct Writer;
impl Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        kputs(s);
        Ok(())
    }
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
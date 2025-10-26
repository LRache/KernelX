use alloc::string::String;
use alloc::sync::Arc;

use crate::kernel::errno::SysResult;
use crate::kernel::event::{PollEvent, PollEventSet};
use crate::driver::{CharDriverOps, DeviceType, DriverOps};
use crate::driver::chosen::kconsole::KConsole;

use super::sbi;

pub struct SBIConsoleDriver;

impl DriverOps for SBIConsoleDriver {
    fn name(&self) -> &str {
        "sbi-console"
    }

    fn device_name(&self) -> String {
        "sbi-console".into()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn as_char_driver(self: Arc<Self>) -> alloc::sync::Arc<dyn CharDriverOps> {
        self
    }
}

impl CharDriverOps for SBIConsoleDriver {
    fn putchar(&self, c: u8) {
        sbi::putchar(c);
    }
    
    fn getchar(&self) -> Option<u8> {
        None
    }

    fn poll(&self, _waker: usize, _event: PollEventSet) -> SysResult<Option<PollEvent>> {
        unimplemented!()
    }

    fn poll_cancel(&self) {
        unimplemented!()
    }
}

pub struct SBIKConsole;

impl KConsole for SBIKConsole {
    fn kputs(&self, s: &str) {
        s.bytes().for_each(sbi::putchar);
    }
}

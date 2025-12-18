use alloc::string::String;
use alloc::sync::Arc;

use crate::kernel::errno::SysResult;
use crate::kernel::event::{FileEvent, PollEventSet};
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

    fn as_char_driver(self: Arc<Self>) -> Option<alloc::sync::Arc<dyn CharDriverOps>> {
        Some(self)
    }
}

impl CharDriverOps for SBIConsoleDriver {
    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        for &c in buf {
            sbi::putchar(c);
        }
        Ok(buf.len())
    }

    fn read(&self, _: &mut [u8]) -> SysResult<usize> {
        Ok(0)
    }

    fn wait_event(&self, _waker: usize, _event: PollEventSet) -> SysResult<Option<FileEvent>> {
        // unimplemented!()
        Ok(None)
    }

    fn wait_event_cancel(&self) {
        unimplemented!()
    }
}

pub struct SBIKConsole;

impl KConsole for SBIKConsole {
    fn kputs(&self, s: &str) {
        s.bytes().for_each(sbi::putchar);
    }
}

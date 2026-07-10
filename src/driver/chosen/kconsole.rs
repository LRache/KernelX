use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::driver::CharDriverOps;
use crate::klib::RWLock;

pub trait KConsole: Send + Sync {
    fn kputs(&self, s: &str);
}

struct CharDriverKConsole {
    driver: Arc<dyn CharDriverOps>,
}

impl CharDriverKConsole {
    fn new(driver: Arc<dyn CharDriverOps>) -> Self {
        Self { driver }
    }
}

impl KConsole for CharDriverKConsole {
    fn kputs(&self, s: &str) {
        let _ = self.driver.write(s.as_bytes());
    }
}

pub(super) fn new_char_driver_console(driver: Arc<dyn CharDriverOps>) -> Box<dyn KConsole> {
    Box::new(CharDriverKConsole::new(driver))
}

static KCONSOLE: RWLock<Option<Box<dyn KConsole>>> = RWLock::new(None, "static::KCONSOLE");

pub fn register(console: Box<dyn KConsole>) {
    *KCONSOLE.write() = Some(console);
}

pub fn register_driver(driver: Arc<dyn CharDriverOps>) {
    register(new_char_driver_console(driver));
}

pub fn kputs(s: &str) {
    let console = KCONSOLE.read();
    if let Some(console) = console.as_ref() {
        console.kputs(s);
    }
}

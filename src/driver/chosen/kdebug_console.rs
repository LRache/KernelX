use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::driver::CharDriverOps;
use crate::klib::RWLock;

use super::kconsole::{KConsole, new_char_driver_console};

static KDEBUG_CONSOLE: RWLock<Option<Box<dyn KConsole>>> = RWLock::new(None, "static::KDEBUG_CONSOLE");

pub fn register(console: Box<dyn KConsole>) {
    *KDEBUG_CONSOLE.write() = Some(console);
}

pub fn register_driver(driver: Arc<dyn CharDriverOps>) {
    register(new_char_driver_console(driver));
}

#[allow(dead_code)]
pub fn kputs(s: &str) -> bool {
    let console = KDEBUG_CONSOLE.read();
    if let Some(console) = console.as_ref() {
        console.kputs(s);
        true
    } else {
        false
    }
}

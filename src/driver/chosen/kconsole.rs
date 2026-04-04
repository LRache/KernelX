use spin::RwLock;

pub trait KConsole: Sync {
    fn kputs(&self, s: &str);
}

struct EmptyKConsole;

impl KConsole for EmptyKConsole {
    fn kputs(&self, _s: &str) {
        // Do nothing
    }
}

static KCONSOLE: RwLock<&'static dyn KConsole> = RwLock::new(&EmptyKConsole);

pub fn register(console: &'static dyn KConsole) {
    *KCONSOLE.write() = console;
}

pub fn kputs(s: &str) {
    KCONSOLE.read().kputs(s);
}

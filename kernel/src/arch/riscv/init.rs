use crate::arch::kernelpagetable;
use super::process;

pub fn init() {
    process::init();
    kernelpagetable::init();
}

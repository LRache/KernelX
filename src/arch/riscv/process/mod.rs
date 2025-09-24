pub mod context;
pub mod percpu;
pub mod switch;
pub mod traphandle;

pub use percpu::*;
pub use traphandle::*;

use traphandle::kerneltrap_handler;

use crate::arch::riscv::csr::stvec;

pub fn init() {
    stvec::write(kerneltrap_handler as usize);
}

//! Early ns16550a UART driver for the QEMU `-M virt` LoongArch machine.
//!
//! This is the minimum piece of the driver stack that has to work before the
//! full `driver::*` framework comes up — `kinfo!` / `print!` / the panic
//! handler all funnel through `driver::chosen::kconsole::kputs`, so somebody
//! needs to register a `KConsole` right at the top of `Arch::init`.
//!
//! Hardware facts (verified against `-machine dumpdtb`):
//!   - PA        0x1fe0_01e0    ns16550a, reg-shift=0, reg-io-width=1
//!   - CC attr:  through DMW0 (VSEG 0x8, MAT=SUC, set in clib/.../entry.S)
//!              → KernelX VA 0x8000_0000_1fe0_01e0
//!   - Clock:    100 MHz (0x5f5e100 Hz per DTS)
//!
//! We don't reprogram the UART — QEMU's already left it in a usable state
//! (8N1, FIFO enabled, 115200 baud).  All we do is poll LSR.THRE before
//! each byte and write THR.

use crate::driver::chosen::kconsole::KConsole;

/// UART THR / RBR (offset 0, DLAB=0)
const UART_BASE: usize = 0x8000_0000_1fe0_01e0;
/// Line Status Register
const UART_LSR: usize = UART_BASE + 5;
/// LSR bit 5 = Transmit Holding Register empty
const LSR_THRE: u8 = 1 << 5;

#[inline(always)]
fn poll_tx_ready() {
    // Spin until THR is empty. QEMU's model never blocks the host, so this
    // is essentially a one-read loop in practice.
    while unsafe { core::ptr::read_volatile(UART_LSR as *const u8) } & LSR_THRE == 0 {}
}

#[inline(always)]
fn putc(c: u8) {
    poll_tx_ready();
    unsafe { core::ptr::write_volatile(UART_BASE as *mut u8, c) };
}

/// `KConsole` impl. Owned by `Arch::init`, which registers an eternal
/// `&'static` reference with `driver::chosen::kconsole::register`.
pub struct EarlyUart;

impl KConsole for EarlyUart {
    fn kputs(&self, s: &str) {
        for b in s.bytes() {
            // Translate lone '\n' to CR-LF so we look sane on a raw serial
            // line and in Qemu's `-nographic` mode.
            if b == b'\n' {
                putc(b'\r');
            }
            putc(b);
        }
    }
}

pub static EARLY_UART: EarlyUart = EarlyUart;

use crate::driver::chosen::kconsole::KConsole;

const UART_BASE: usize = 0x8000_0000_1fe0_01e0;
const UART_LSR: usize = UART_BASE + 5;
const LSR_THRE: u8 = 1 << 5;

#[inline(always)]
fn poll_tx_ready() {
    while unsafe { core::ptr::read_volatile(UART_LSR as *const u8) } & LSR_THRE == 0 {}
}

#[inline(always)]
fn putc(c: u8) {
    poll_tx_ready();
    unsafe { core::ptr::write_volatile(UART_BASE as *mut u8, c) };
}

pub struct EarlyUart;

impl KConsole for EarlyUart {
    fn kputs(&self, s: &str) {
        for b in s.bytes() {
            // Translate lone '\n' to CR-LF for raw serial / `-nographic`.
            if b == b'\n' {
                putc(b'\r');
            }
            putc(b);
        }
    }
}

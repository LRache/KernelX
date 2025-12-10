// Reference: https://github.com/mit-pdos/xv6-riscv/blob/riscv/kernel/uart.c

use alloc::string::String;
use alloc::sync::Arc;

use crate::driver::{DriverOps, CharDriverOps, DeviceType};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{PollEventSet, FileEvent};
use crate::klib::SpinLock;

mod regs {
    pub const RHR: usize = 0; // receive holding register (for input bytes)
    pub const THR: usize = 0; // transmit holding register (for output bytes)
    pub const IER: usize = 1; // interrupt enable register
    pub const IER_RX_ENABLE: u8 = 1 << 0;
    pub const IER_TX_ENABLE: u8 = 1 << 1;
    pub const FCR: usize = 2; // FIFO control register
    pub const FCR_FIFO_ENABLE: u8 = 1 << 0;
    pub const FCR_FIFO_CLEAR: u8 = 3 << 1; // clear the content of the two FIFOs
    pub const ISR: usize = 2; // interrupt status register
    pub const LCR: usize = 3; // line control register
    pub const LCR_EIGHT_BITS: u8 = 3 << 0;
    pub const LCR_BAUD_LATCH: u8 = 1 << 7; // special mode to set baud rate
    pub const LSR: usize = 5; // line status register
    pub const LSR_RX_READY: u8 = 1 << 0; // input is waiting to be read from RHR
    pub const LSR_TX_IDLE: u8 = 1 << 5; // THR can accept another character to send
}

struct Inner {
    base: usize,
}

impl Inner {
    #[inline(always)]
    fn read_reg(&mut self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u8) }
    }

    #[inline(always)]
    fn write_reg(&mut self, offset: usize, value: u8) {
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u8, value); }
    }
}

pub struct Driver {
    inner: SpinLock<Inner>,
}

impl Driver {
    pub fn new(base: usize) -> Self {
        Driver {
            inner: SpinLock::new(Inner { base })
        }
    }

    pub fn init(&self) {
        let mut inner = self.inner.lock();

        // Disable interrupts while configuring the UART.
        inner.write_reg(regs::IER, 0x00);

        // Enter baud latch mode to configure baud rate.
        inner.write_reg(regs::LCR, regs::LCR_BAUD_LATCH);

        // Configure baud rate divisor for 38.4K (LSB then MSB).
        inner.write_reg(0, 0x03);
        inner.write_reg(1, 0x00);

        // Leave baud latch mode and configure: 8 bits, no parity.
        inner.write_reg(regs::LCR, regs::LCR_EIGHT_BITS);

        // Reset and enable FIFOs.
        inner.write_reg(regs::FCR, regs::FCR_FIFO_ENABLE | regs::FCR_FIFO_CLEAR);

        // Enable transmit and receive interrupts.
        inner.write_reg(regs::IER, regs::IER_TX_ENABLE | regs::IER_RX_ENABLE);
    }
}

impl DriverOps for Driver {
    fn name(&self) -> &str {
        "uart16650"
    }

    fn device_name(&self) -> String {
        "serial".into()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn as_char_driver(self: Arc<Self>) -> Arc<dyn CharDriverOps> {
        self
    }
}

impl CharDriverOps for Driver {
    fn putchar(&self, c: u8) {
        let mut inner = self.inner.lock();
        // Wait for UART to be ready to transmit.
        while (inner.read_reg(regs::LSR) & regs::LSR_TX_IDLE) == 0 {}
        
        inner.write_reg(regs::THR, c);
    }

    fn getchar(&self) -> Option<u8> {
        let mut inner = self.inner.lock();
        // Check if a character is available to read.
        if (inner.read_reg(regs::LSR) & regs::LSR_RX_READY) == 0 {
            return None;
        }
        
        Some(inner.read_reg(regs::RHR))
    }

    fn wait_event(&self, _waker: usize, _event: PollEventSet) -> SysResult<Option<FileEvent>> {
        Err(Errno::ENOSYS)
    }

    fn wait_event_cancel(&self) {}
}
